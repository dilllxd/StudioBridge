use anyhow::Context;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};
use studiobridge_beacn::{BeacnStudioBackend, DspWriteGate};
use studiobridge_core::{
    AppSnapshot, BridgeError, CreateMixerSourceRequest, DspWriteModule, MicrophoneDspUpdate,
    MixerBackend, MixerProfileRequest, MixerProfileSummary, MixerProfilesResponse, MixerSnapshot,
    MockMixerBackend, MockStudioBackend, RemoveMixerSourceRequest, ReorderMixerSourceRequest,
    SetDefaultDeviceRequest, SetLinkAssignmentRequest, SetLinkOutputAssignmentRequest,
    SetMicrophoneRequest, SetMixerApplicationRequest, SetMixerSourceColourRequest,
    SetMixerSourceNameRequest, SetMuteRequest, SetRouteRequest, SetSourceDeviceRequest,
    SetTargetDeviceRequest, SetTargetMuteRequest, SetTargetVolumeRequest, SetVolumeLinkedRequest,
    SetVolumeRequest, StudioBackend, StudioBridgeService,
};
use studiobridge_pipeweaver::PipeweaverBackend;
use tokio::sync::RwLock;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(about = "Unofficial Linux control daemon for BEACN Studio")]
struct Args {
    #[arg(long, value_enum, default_value_t = StudioMode::Mock)]
    studio: StudioMode,

    #[arg(long, value_enum, default_value_t = MixerMode::Mock)]
    mixer: MixerMode,

    #[arg(long, default_value = "http://127.0.0.1:14565")]
    pipeweaver_url: String,

    #[arg(
        long,
        help = "Enable BEACN gain, phantom-power, and Link assignment writes"
    )]
    allow_hardware_writes: bool,

    #[arg(
        long,
        help = "Enable the zero-payload USB1 Link host heartbeat and Link assignments"
    )]
    enable_link_host: bool,

    #[arg(
        long = "enable-dsp-write",
        value_enum,
        action = clap::ArgAction::Append,
        help = "Enable writes for exactly one validated DSP module; repeat for additional modules"
    )]
    enabled_dsp_writes: Vec<DspWriteModuleArg>,

    #[arg(
        long,
        hide = true,
        help = "Legacy alias; mock mode is already the default"
    )]
    _mock: bool,

    #[arg(long, default_value = "127.0.0.1")]
    bind: String,

    #[arg(long, default_value_t = 17840)]
    port: u16,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StudioMode {
    Mock,
    Beacn,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MixerMode {
    Mock,
    Pipeweaver,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DspWriteModuleArg {
    Equalizer,
    Compressor,
    Expander,
    NoiseSuppression,
    EnhancementSuite,
    HeadphoneEqualizer,
}

impl From<DspWriteModuleArg> for DspWriteModule {
    fn from(value: DspWriteModuleArg) -> Self {
        match value {
            DspWriteModuleArg::Equalizer => Self::Equalizer,
            DspWriteModuleArg::Compressor => Self::Compressor,
            DspWriteModuleArg::Expander => Self::Expander,
            DspWriteModuleArg::NoiseSuppression => Self::NoiseSuppression,
            DspWriteModuleArg::EnhancementSuite => Self::EnhancementSuite,
            DspWriteModuleArg::HeadphoneEqualizer => Self::HeadphoneEqualizer,
        }
    }
}

#[derive(Clone)]
struct AppState {
    service: StudioBridgeService,
    runtime: RuntimeInfo,
    static_dsp_writes: HashSet<DspWriteModule>,
    dsp_write_gate: DspWriteGate,
    profiles: ProfileStore,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedMixerProfile {
    name: String,
    mixer: MixerSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProfileFile {
    active: Option<String>,
    profiles: Vec<SavedMixerProfile>,
}

#[cfg(test)]
struct ProfilePersistenceControl {
    fail_next: std::sync::atomic::AtomicBool,
    pause_next: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Semaphore,
    released: (std::sync::Mutex<bool>, std::sync::Condvar),
}

#[cfg(test)]
impl ProfilePersistenceControl {
    fn new() -> Self {
        Self {
            fail_next: std::sync::atomic::AtomicBool::new(false),
            pause_next: std::sync::atomic::AtomicBool::new(false),
            entered: tokio::sync::Semaphore::new(0),
            released: (std::sync::Mutex::new(false), std::sync::Condvar::new()),
        }
    }

    fn fail_once(&self) {
        self.fail_next
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn pause_once(&self) {
        *self.released.0.lock().expect("persistence release lock") = false;
        self.pause_next
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    async fn wait_until_paused(&self) {
        self.entered
            .acquire()
            .await
            .expect("persistence pause semaphore")
            .forget();
    }

    fn release(&self) {
        *self.released.0.lock().expect("persistence release lock") = true;
        self.released.1.notify_all();
    }

    fn before_persist(&self) -> Result<(), BridgeError> {
        if self
            .fail_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(BridgeError::Backend(
                "injected profile persistence failure".into(),
            ));
        }
        if self
            .pause_next
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            self.entered.add_permits(1);
            let mut released = self.released.0.lock().expect("persistence release lock");
            while !*released {
                released = self
                    .released
                    .1
                    .wait(released)
                    .expect("persistence release condition");
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ProfileStore {
    path: PathBuf,
    state: Arc<RwLock<ProfileFile>>,
    transaction: Arc<tokio::sync::Mutex<()>>,
    #[cfg(test)]
    persistence_control: Option<Arc<ProfilePersistenceControl>>,
}

impl ProfileStore {
    fn load() -> Self {
        let path = daemon_config_root().join("mixer-profiles.json");
        let state = read_profile_file(&path).unwrap_or_default();
        Self {
            path,
            state: Arc::new(RwLock::new(state)),
            transaction: Arc::new(tokio::sync::Mutex::new(())),
            #[cfg(test)]
            persistence_control: None,
        }
    }

    async fn list(&self) -> MixerProfilesResponse {
        let state = self.state.read().await;
        MixerProfilesResponse {
            profiles: state
                .profiles
                .iter()
                .map(|profile| MixerProfileSummary {
                    name: profile.name.clone(),
                    active: state.active.as_deref() == Some(profile.name.as_str()),
                })
                .collect(),
        }
    }

    async fn get(&self, name: &str) -> Option<MixerSnapshot> {
        self.state
            .read()
            .await
            .profiles
            .iter()
            .find(|profile| profile.name == name)
            .map(|profile| profile.mixer.clone())
    }

    async fn save(&self, name: String, mixer: MixerSnapshot) -> Result<(), BridgeError> {
        validate_profile_name(&name)?;
        self.transact(move |next| {
            if let Some(profile) = next
                .profiles
                .iter_mut()
                .find(|profile| profile.name == name)
            {
                profile.mixer = mixer;
            } else {
                next.profiles.push(SavedMixerProfile {
                    name: name.clone(),
                    mixer,
                });
            }
            next.active = Some(name);
            Ok(())
        })
        .await
    }

    async fn create(&self, mixer: MixerSnapshot) -> Result<String, BridgeError> {
        self.transact(move |next| {
            let name = next_profile_name(
                "New Profile",
                next.profiles.iter().map(|profile| profile.name.as_str()),
                false,
            );
            next.profiles.push(SavedMixerProfile {
                name: name.clone(),
                mixer,
            });
            next.active = Some(name.clone());
            Ok(name)
        })
        .await
    }

    async fn duplicate(&self, name: &str) -> Result<String, BridgeError> {
        let name = name.to_owned();
        self.transact(move |next| {
            let mixer = next
                .profiles
                .iter()
                .find(|profile| profile.name == name)
                .map(|profile| profile.mixer.clone())
                .ok_or_else(|| {
                    BridgeError::InvalidValue(format!("unknown mixer profile: {name}"))
                })?;
            let duplicate_name = next_profile_name(
                &name,
                next.profiles.iter().map(|profile| profile.name.as_str()),
                true,
            );
            next.profiles.push(SavedMixerProfile {
                name: duplicate_name.clone(),
                mixer,
            });
            Ok(duplicate_name)
        })
        .await
    }

    async fn set_active(&self, name: &str) -> Result<(), BridgeError> {
        let name = name.to_owned();
        self.transact(move |next| {
            if !next.profiles.iter().any(|profile| profile.name == name) {
                return Err(BridgeError::InvalidValue(format!(
                    "unknown mixer profile: {name}"
                )));
            }
            next.active = Some(name);
            Ok(())
        })
        .await
    }

    async fn delete(&self, name: &str) -> Result<(), BridgeError> {
        let name = name.to_owned();
        self.transact(move |next| {
            let before = next.profiles.len();
            next.profiles.retain(|profile| profile.name != name);
            if next.profiles.len() == before {
                return Err(BridgeError::InvalidValue(format!(
                    "unknown mixer profile: {name}"
                )));
            }
            if next.active.as_deref() == Some(name.as_str()) {
                next.active = None;
            }
            Ok(())
        })
        .await
    }

    async fn transact<T, F>(&self, mutation: F) -> Result<T, BridgeError>
    where
        T: Send + 'static,
        F: FnOnce(&mut ProfileFile) -> Result<T, BridgeError> + Send + 'static,
    {
        // A canceled waiter leaves the queue. Once acquired, the owned task
        // finishes disk persistence and memory publication even if its caller
        // disconnects, keeping both committed representations aligned.
        let transaction = self.transaction.clone().lock_owned().await;
        let store = self.clone();
        tokio::spawn(async move {
            let _transaction = transaction;
            let mut next = store.state.read().await.clone();
            let result = mutation(&mut next)?;
            let path = store.path.clone();
            let durable = next.clone();
            #[cfg(test)]
            let persistence_control = store.persistence_control.clone();
            tokio::task::spawn_blocking(move || {
                #[cfg(test)]
                if let Some(control) = persistence_control {
                    control.before_persist()?;
                }
                persist_profile_file(&path, &durable)
            })
            .await
            .map_err(|error| {
                BridgeError::Backend(format!("profile persistence task failed: {error}"))
            })??;
            *store.state.write().await = next;
            Ok(result)
        })
        .await
        .map_err(|error| {
            BridgeError::Backend(format!("profile transaction task failed: {error}"))
        })?
    }
}

fn persist_profile_file(path: &std::path::Path, state: &ProfileFile) -> Result<(), BridgeError> {
    let parent = path
        .parent()
        .ok_or_else(|| BridgeError::Backend("mixer profile path has no parent directory".into()))?;
    std::fs::create_dir_all(parent).map_err(|error| BridgeError::Backend(error.to_string()))?;
    let json = serde_json::to_vec_pretty(state)
        .map_err(|error| BridgeError::Backend(error.to_string()))?;
    let temp_path = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let write_result = (|| {
        let mut temp = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        std::io::Write::write_all(&mut temp, &json)
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        temp.sync_all()
            .map_err(|error| BridgeError::Backend(error.to_string()))
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    replace_file(&temp_path, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp_path);
    })
}

fn profile_backup_path(path: &std::path::Path) -> PathBuf {
    path.with_extension("json.backup")
}

fn read_profile_file(path: &std::path::Path) -> Option<ProfileFile> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .or_else(|| {
            let backup = profile_backup_path(path);
            let state = std::fs::read(&backup)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())?;
            let _ = std::fs::rename(&backup, path);
            Some(state)
        })
}

fn replace_file(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), BridgeError> {
    #[cfg(not(windows))]
    {
        std::fs::rename(source, destination)
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        if let Some(parent) = destination.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| BridgeError::Backend(error.to_string()))?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        if !destination.exists() {
            return std::fs::rename(source, destination)
                .map_err(|error| BridgeError::Backend(error.to_string()));
        }
        let backup = profile_backup_path(destination);
        if backup.exists() {
            std::fs::remove_file(&backup)
                .map_err(|error| BridgeError::Backend(error.to_string()))?;
        }
        std::fs::rename(destination, &backup)
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        if let Err(error) = std::fs::rename(source, destination) {
            let _ = std::fs::rename(&backup, destination);
            return Err(BridgeError::Backend(error.to_string()));
        }
        let _ = std::fs::remove_file(backup);
        Ok(())
    }
}

fn validate_profile_name(name: &str) -> Result<(), BridgeError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 64 {
        return Err(BridgeError::InvalidValue(
            "profile name must contain 1 to 64 characters".into(),
        ));
    }
    Ok(())
}

fn next_profile_name<'a>(
    base: &str,
    existing: impl Iterator<Item = &'a str>,
    parenthesized: bool,
) -> String {
    let existing = existing.collect::<HashSet<_>>();
    if !parenthesized && !existing.contains(base) {
        return base.to_owned();
    }
    for index in 1..=10_000 {
        let suffix = if parenthesized {
            format!(" ({index})")
        } else {
            format!(" {index}")
        };
        let max_base_chars = 64_usize.saturating_sub(suffix.chars().count());
        let candidate = format!(
            "{}{}",
            base.chars().take(max_base_chars).collect::<String>(),
            suffix
        );
        if !existing.contains(candidate.as_str()) {
            return candidate;
        }
    }
    unreachable!("profile name space exhausted")
}

fn daemon_config_root() -> PathBuf {
    #[cfg(windows)]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return PathBuf::from(app_data).join("StudioBridge");
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".config"))
        })
        .unwrap_or_else(std::env::temp_dir)
        .join("studiobridge")
}

#[derive(Debug, Serialize)]
struct ApiMessage {
    ok: bool,
    message: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct RuntimeInfo {
    studio_mode: &'static str,
    mixer_mode: &'static str,
    hardware_writes_enabled: bool,
    link_control_enabled: bool,
    dsp_write_modules: Vec<&'static str>,
    dsp_write_active_module: Option<&'static str>,
    dsp_write_lease_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ArmDspRequest {
    module: DspWriteModule,
    acknowledgement: String,
    lease_seconds: u64,
}

#[derive(Debug, Serialize)]
struct DspGateResponse {
    armed: bool,
    module: Option<&'static str>,
    lease_seconds: Option<u64>,
}

const DSP_WRITE_ACKNOWLEDGEMENT: &str = "I_UNDERSTAND_THIS_CHANGES_AUDIO";
const MIN_DSP_LEASE_SECONDS: u64 = 30;
const MAX_DSP_LEASE_SECONDS: u64 = 300;

fn dsp_editing_backends_supported(studio_mode: &str, mixer_mode: &str) -> bool {
    matches!(
        (studio_mode, mixer_mode),
        ("beacn", "pipeweaver") | ("mock", "mock")
    )
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    ok: bool,
    message: &'static str,
    #[serde(flatten)]
    runtime: RuntimeInfo,
}

#[derive(Debug)]
struct ApiError(BridgeError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0 {
            BridgeError::InvalidValue(_) => StatusCode::BAD_REQUEST,
            BridgeError::BackendUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            BridgeError::Backend(_) => StatusCode::BAD_GATEWAY,
        };
        (
            status,
            Json(serde_json::json!({ "error": self.0.to_string() })),
        )
            .into_response()
    }
}

impl From<BridgeError> for ApiError {
    fn from(value: BridgeError) -> Self {
        Self(value)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let args = Args::parse();
    let enabled_dsp_writes: HashSet<DspWriteModule> = args
        .enabled_dsp_writes
        .iter()
        .copied()
        .map(Into::into)
        .collect();
    let dsp_write_gate = DspWriteGate::default();
    let studio: Arc<dyn StudioBackend> = match args.studio {
        StudioMode::Mock => Arc::new(MockStudioBackend::default()),
        StudioMode::Beacn => Arc::new(BeacnStudioBackend::spawn(
            args.allow_hardware_writes,
            args.enable_link_host,
            enabled_dsp_writes.clone(),
            dsp_write_gate.clone(),
        )?),
    };
    let mixer: Arc<dyn MixerBackend> = match args.mixer {
        MixerMode::Mock => Arc::new(MockMixerBackend::default()),
        MixerMode::Pipeweaver => Arc::new(PipeweaverBackend::new(&args.pipeweaver_url)),
    };
    let service = StudioBridgeService::new(studio, mixer);
    let state = AppState {
        service,
        runtime: RuntimeInfo {
            studio_mode: match args.studio {
                StudioMode::Mock => "mock",
                StudioMode::Beacn => "beacn",
            },
            mixer_mode: match args.mixer {
                MixerMode::Mock => "mock",
                MixerMode::Pipeweaver => "pipeweaver",
            },
            hardware_writes_enabled: args.allow_hardware_writes,
            link_control_enabled: args.enable_link_host || args.allow_hardware_writes,
            dsp_write_modules: DspWriteModule::ALL
                .into_iter()
                .filter(|module| enabled_dsp_writes.contains(module))
                .map(DspWriteModule::as_str)
                .collect(),
            dsp_write_active_module: None,
            dsp_write_lease_seconds: None,
        },
        static_dsp_writes: enabled_dsp_writes,
        dsp_write_gate,
        profiles: ProfileStore::load(),
    };

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/state", get(snapshot))
        .route("/api/studio/microphone", post(set_microphone))
        .route(
            "/api/studio/microphone-dsp",
            get(microphone_dsp).post(set_microphone_dsp),
        )
        .route("/api/studio/dsp-arm", post(arm_dsp_write))
        .route("/api/studio/dsp-disarm", post(disarm_dsp_write))
        .route("/api/studio/link-assignment", post(set_link_assignment))
        .route("/api/mixer/volume", post(set_volume))
        .route("/api/mixer/target-volume", post(set_target_volume))
        .route("/api/mixer/target-mute", post(set_target_mute))
        .route("/api/mixer/target-device", post(set_target_device))
        .route("/api/mixer/source-device", post(set_source_device))
        .route(
            "/api/mixer/link-output-assignment",
            post(set_link_output_assignment),
        )
        .route("/api/mixer/default-input", post(set_default_input))
        .route("/api/mixer/default-output", post(set_default_output))
        .route("/api/mixer/volume-link", post(set_volume_linked))
        .route("/api/mixer/mute", post(set_mute))
        .route("/api/mixer/route", post(set_route))
        .route("/api/mixer/source", post(create_mixer_source))
        .route("/api/mixer/source/name", post(set_mixer_source_name))
        .route("/api/mixer/source/colour", post(set_mixer_source_colour))
        .route("/api/mixer/source/remove", post(remove_mixer_source))
        .route("/api/mixer/source/reorder", post(reorder_mixer_source))
        .route("/api/mixer/profiles", get(list_mixer_profiles))
        .route("/api/mixer/profile/save", post(save_mixer_profile))
        .route("/api/mixer/profile/create", post(create_mixer_profile))
        .route(
            "/api/mixer/profile/duplicate",
            post(duplicate_mixer_profile),
        )
        .route("/api/mixer/profile/load", post(load_mixer_profile))
        .route("/api/mixer/profile/delete", post(delete_mixer_profile))
        .route("/api/mixer/application", post(set_mixer_application))
        .fallback_service(ServeDir::new("web/dist").append_index_html_on_directories(true))
        .layer(
            CorsLayer::new()
                .allow_origin([
                    HeaderValue::from_static("http://127.0.0.1:5173"),
                    HeaderValue::from_static("http://localhost:5173"),
                ])
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE]),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let address: SocketAddr = format!("{}:{}", args.bind, args.port)
        .parse()
        .context("invalid bind address")?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    info!(
        %address,
        studio = ?args.studio,
        mixer = ?args.mixer,
        hardware_writes = args.allow_hardware_writes,
        link_control = args.enable_link_host || args.allow_hardware_writes,
        "StudioBridge daemon listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let mut runtime = state.runtime.clone();
    if let Ok(Some((module, remaining))) = state.dsp_write_gate.active_lease() {
        if !state.static_dsp_writes.contains(&module) {
            runtime.dsp_write_modules.push(module.as_str());
        }
        runtime.dsp_write_active_module = Some(module.as_str());
        runtime.dsp_write_lease_seconds = Some(remaining.as_secs().max(1));
    }
    Json(HealthResponse {
        ok: true,
        message: "studiobridge is ready",
        runtime,
    })
}

async fn snapshot(State(state): State<AppState>) -> Result<Json<AppSnapshot>, ApiError> {
    Ok(Json(state.service.snapshot().await?))
}

async fn microphone_dsp(
    State(state): State<AppState>,
) -> Result<Json<studiobridge_core::MicrophoneDspSnapshot>, ApiError> {
    Ok(Json(state.service.microphone_dsp_snapshot().await?))
}

async fn set_microphone_dsp(
    State(state): State<AppState>,
    Json(value): Json<serde_json::Value>,
) -> Result<Json<studiobridge_core::MicrophoneDspWriteResult>, ApiError> {
    let update = parse_microphone_dsp_update(value)?;
    Ok(Json(state.service.set_microphone_dsp(update).await?))
}

fn parse_microphone_dsp_update(value: serde_json::Value) -> Result<MicrophoneDspUpdate, ApiError> {
    if value.get("module").and_then(serde_json::Value::as_str) == Some("headphone_equalizer")
        && value
            .get("state")
            .and_then(|state| state.get("subwoofer"))
            .is_none()
    {
        return Err(ApiError(BridgeError::InvalidValue(
            "headphone equalizer writes require explicit subwoofer state".into(),
        )));
    }
    serde_json::from_value(value).map_err(|error| {
        ApiError(BridgeError::InvalidValue(format!(
            "invalid microphone DSP update: {error}"
        )))
    })
}

async fn arm_dsp_write(
    State(state): State<AppState>,
    Json(request): Json<ArmDspRequest>,
) -> Result<Json<DspGateResponse>, ApiError> {
    if request.acknowledgement != DSP_WRITE_ACKNOWLEDGEMENT {
        return Err(ApiError(BridgeError::InvalidValue(
            "DSP editing acknowledgement did not match".into(),
        )));
    }
    if !(MIN_DSP_LEASE_SECONDS..=MAX_DSP_LEASE_SECONDS).contains(&request.lease_seconds) {
        return Err(ApiError(BridgeError::InvalidValue(format!(
            "DSP lease must be between {MIN_DSP_LEASE_SECONDS} and {MAX_DSP_LEASE_SECONDS} seconds"
        ))));
    }
    if state.runtime.hardware_writes_enabled {
        return Err(ApiError(BridgeError::InvalidValue(
            "in-app DSP leases require general hardware writes to remain disabled".into(),
        )));
    }
    if !dsp_editing_backends_supported(state.runtime.studio_mode, state.runtime.mixer_mode) {
        return Err(ApiError(BridgeError::BackendUnavailable(
            "DSP editing requires either the real BEACN/PipeWeaver pair or the isolated mock/mock simulation".into(),
        )));
    }

    // Require both backends and every known DSP reader to succeed immediately before arming.
    state.service.snapshot().await?;
    state.service.microphone_dsp_snapshot().await?;
    state
        .dsp_write_gate
        .arm_exclusive(request.module, Duration::from_secs(request.lease_seconds))?;

    Ok(Json(DspGateResponse {
        armed: true,
        module: Some(request.module.as_str()),
        lease_seconds: Some(request.lease_seconds),
    }))
}

async fn disarm_dsp_write(
    State(state): State<AppState>,
) -> Result<Json<DspGateResponse>, ApiError> {
    state.dsp_write_gate.disarm()?;
    Ok(Json(DspGateResponse {
        armed: false,
        module: None,
        lease_seconds: None,
    }))
}

async fn set_microphone(
    State(state): State<AppState>,
    Json(request): Json<SetMicrophoneRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.set_microphone(request).await?;
    Ok(ok())
}

async fn set_link_assignment(
    State(state): State<AppState>,
    Json(request): Json<SetLinkAssignmentRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_link_assignment(&request.application, request.channel)
        .await?;
    Ok(ok())
}

async fn set_volume(
    State(state): State<AppState>,
    Json(request): Json<SetVolumeRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_volume(&request.channel_id, request.mix, request.volume)
        .await?;
    Ok(ok())
}

async fn set_target_volume(
    State(state): State<AppState>,
    Json(request): Json<SetTargetVolumeRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_target_volume(&request.target_id, request.volume)
        .await?;
    Ok(ok())
}

async fn set_target_mute(
    State(state): State<AppState>,
    Json(request): Json<SetTargetMuteRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_target_mute(&request.target_id, request.muted)
        .await?;
    Ok(ok())
}

async fn set_target_device(
    State(state): State<AppState>,
    Json(request): Json<SetTargetDeviceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_target_device(&request.target_id, request.device_node_id)
        .await?;
    Ok(ok())
}

async fn set_source_device(
    State(state): State<AppState>,
    Json(request): Json<SetSourceDeviceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_source_device(&request.channel_id, request.device_node_id)
        .await?;
    Ok(ok())
}

async fn set_link_output_assignment(
    State(state): State<AppState>,
    Json(request): Json<SetLinkOutputAssignmentRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_link_output_assignment(request.output_node_id, request.target_id.as_deref())
        .await?;
    Ok(ok())
}

async fn set_default_input(
    State(state): State<AppState>,
    Json(request): Json<SetDefaultDeviceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.set_default_input(&request.device_id).await?;
    Ok(ok())
}

async fn set_default_output(
    State(state): State<AppState>,
    Json(request): Json<SetDefaultDeviceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.set_default_output(&request.device_id).await?;
    Ok(ok())
}

async fn set_volume_linked(
    State(state): State<AppState>,
    Json(request): Json<SetVolumeLinkedRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_volume_linked(&request.channel_id, request.linked)
        .await?;
    Ok(ok())
}

async fn set_mute(
    State(state): State<AppState>,
    Json(request): Json<SetMuteRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_mute(&request.channel_id, request.state)
        .await?;
    Ok(ok())
}

async fn set_route(
    State(state): State<AppState>,
    Json(request): Json<SetRouteRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_route(&request.source_id, &request.target_id, request.enabled)
        .await?;
    Ok(ok())
}

async fn create_mixer_source(
    State(state): State<AppState>,
    Json(request): Json<CreateMixerSourceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.create_source(&request.name).await?;
    Ok(ok())
}

async fn set_mixer_source_name(
    State(state): State<AppState>,
    Json(request): Json<SetMixerSourceNameRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_source_name(&request.source_id, &request.name)
        .await?;
    Ok(ok())
}

async fn set_mixer_source_colour(
    State(state): State<AppState>,
    Json(request): Json<SetMixerSourceColourRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_source_colour(&request.source_id, &request.colour)
        .await?;
    Ok(ok())
}

async fn remove_mixer_source(
    State(state): State<AppState>,
    Json(request): Json<RemoveMixerSourceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.remove_source(&request.source_id).await?;
    Ok(ok())
}

async fn reorder_mixer_source(
    State(state): State<AppState>,
    Json(request): Json<ReorderMixerSourceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_source_order(&request.source_id, request.position)
        .await?;
    Ok(ok())
}

async fn list_mixer_profiles(State(state): State<AppState>) -> Json<MixerProfilesResponse> {
    Json(state.profiles.list().await)
}

async fn save_mixer_profile(
    State(state): State<AppState>,
    Json(request): Json<MixerProfileRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    validate_profile_name(&request.name)?;
    let mixer = state.service.snapshot().await?.mixer;
    state.profiles.save(request.name, mixer).await?;
    Ok(ok())
}

async fn create_mixer_profile(State(state): State<AppState>) -> Result<Json<ApiMessage>, ApiError> {
    let mixer = state.service.snapshot().await?.mixer;
    state.profiles.create(mixer).await?;
    Ok(ok())
}

async fn duplicate_mixer_profile(
    State(state): State<AppState>,
    Json(request): Json<MixerProfileRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.profiles.duplicate(&request.name).await?;
    Ok(ok())
}

async fn load_mixer_profile(
    State(state): State<AppState>,
    Json(request): Json<MixerProfileRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    let mixer = state.profiles.get(&request.name).await.ok_or_else(|| {
        ApiError(BridgeError::InvalidValue(format!(
            "unknown mixer profile: {}",
            request.name
        )))
    })?;
    state.service.apply_mixer_profile(&mixer).await?;
    state.profiles.set_active(&request.name).await?;
    Ok(ok())
}

async fn delete_mixer_profile(
    State(state): State<AppState>,
    Json(request): Json<MixerProfileRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.profiles.delete(&request.name).await?;
    Ok(ok())
}

async fn set_mixer_application(
    State(state): State<AppState>,
    Json(request): Json<SetMixerApplicationRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state
        .service
        .set_application_route(
            &request.process,
            &request.name,
            request.channel_id.as_deref(),
        )
        .await?;
    Ok(ok())
}

fn ok() -> Json<ApiMessage> {
    Json(ApiMessage {
        ok: true,
        message: "updated",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_test_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "studiobridge-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn test_profile_store(
        root: &std::path::Path,
    ) -> (ProfileStore, Arc<ProfilePersistenceControl>) {
        let persistence_control = Arc::new(ProfilePersistenceControl::new());
        (
            ProfileStore {
                path: root.join("mixer-profiles.json"),
                state: Arc::new(RwLock::new(ProfileFile::default())),
                transaction: Arc::new(tokio::sync::Mutex::new(())),
                persistence_control: Some(persistence_control.clone()),
            },
            persistence_control,
        )
    }

    fn read_persisted_profiles(store: &ProfileStore) -> ProfileFile {
        serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn profile_store_persists_complete_snapshot_without_false_in_memory_success() {
        let root = temporary_test_path("profile-store");
        let (store, _) = test_profile_store(&root);
        let mixer = MockMixerBackend::default().snapshot().await.unwrap();
        store
            .save("Default Profile".into(), mixer.clone())
            .await
            .unwrap();

        let persisted: ProfileFile =
            serde_json::from_slice(&std::fs::read(&store.path).unwrap()).unwrap();
        assert_eq!(persisted.active.as_deref(), Some("Default Profile"));
        assert_eq!(persisted.profiles[0].mixer, mixer);

        let backup = profile_backup_path(&store.path);
        std::fs::rename(&store.path, &backup).unwrap();
        let recovered = read_profile_file(&store.path).unwrap();
        assert_eq!(recovered.active.as_deref(), Some("Default Profile"));
        assert!(store.path.exists());
        assert!(!backup.exists());

        let blocked_parent = root.join("not-a-directory");
        std::fs::write(&blocked_parent, b"file").unwrap();
        let failing = ProfileStore {
            path: blocked_parent.join("mixer-profiles.json"),
            state: Arc::new(RwLock::new(ProfileFile::default())),
            transaction: Arc::new(tokio::sync::Mutex::new(())),
            persistence_control: None,
        };
        let error = failing
            .save("Must Not Exist".into(), mixer)
            .await
            .unwrap_err();
        assert!(matches!(error, BridgeError::Backend(_)));
        assert!(failing.list().await.profiles.is_empty());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_store_serializes_every_mutation_without_lost_updates() {
        let root = temporary_test_path("profile-concurrency");
        let (store, _) = test_profile_store(&root);
        let mixer = MockMixerBackend::default().snapshot().await.unwrap();
        store.save("Keep A".into(), mixer.clone()).await.unwrap();
        store.save("Keep B".into(), mixer.clone()).await.unwrap();
        store.save("Delete Me".into(), mixer.clone()).await.unwrap();

        let save = tokio::spawn({
            let store = store.clone();
            let mixer = mixer.clone();
            async move { store.save("Saved".into(), mixer).await }
        });
        let create = tokio::spawn({
            let store = store.clone();
            let mixer = mixer.clone();
            async move { store.create(mixer).await }
        });
        let duplicate = tokio::spawn({
            let store = store.clone();
            async move { store.duplicate("Keep A").await }
        });
        let activate = tokio::spawn({
            let store = store.clone();
            async move { store.set_active("Keep B").await }
        });
        let delete = tokio::spawn({
            let store = store.clone();
            async move { store.delete("Delete Me").await }
        });

        save.await.unwrap().unwrap();
        let created = create.await.unwrap().unwrap();
        let duplicated = duplicate.await.unwrap().unwrap();
        activate.await.unwrap().unwrap();
        delete.await.unwrap().unwrap();

        let memory = store.state.read().await.clone();
        let persisted = read_persisted_profiles(&store);
        assert_eq!(
            serde_json::to_value(&memory).unwrap(),
            serde_json::to_value(&persisted).unwrap()
        );
        let names = memory
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect::<HashSet<_>>();
        assert_eq!(names.len(), 5);
        assert!(names.contains("Keep A"));
        assert!(names.contains("Keep B"));
        assert!(names.contains("Saved"));
        assert!(names.contains(created.as_str()));
        assert!(names.contains(duplicated.as_str()));
        assert!(!names.contains("Delete Me"));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_store_finishes_an_active_transaction_after_caller_cancellation() {
        let root = temporary_test_path("profile-cancellation");
        let (store, control) = test_profile_store(&root);
        let mixer = MockMixerBackend::default().snapshot().await.unwrap();
        control.pause_once();
        let caller = tokio::spawn({
            let store = store.clone();
            let mixer = mixer.clone();
            async move { store.save("Canceled Caller".into(), mixer).await }
        });
        control.wait_until_paused().await;

        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(store.list().await.profiles.is_empty());
        control.release();

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if store.get("Canceled Caller").await.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("detached profile transaction should finish");
        let persisted = read_persisted_profiles(&store);
        assert_eq!(persisted.active.as_deref(), Some("Canceled Caller"));
        assert_eq!(persisted.profiles.len(), 1);

        store.create(mixer).await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_store_cancels_a_waiter_before_its_transaction_starts() {
        let root = temporary_test_path("profile-waiter-cancellation");
        let (store, control) = test_profile_store(&root);
        let mixer = MockMixerBackend::default().snapshot().await.unwrap();
        control.pause_once();
        let first = tokio::spawn({
            let store = store.clone();
            let mixer = mixer.clone();
            async move { store.save("First".into(), mixer).await }
        });
        control.wait_until_paused().await;

        let canceled = tokio::time::timeout(Duration::from_millis(25), store.create(mixer)).await;
        assert!(canceled.is_err());
        control.release();
        first.await.unwrap().unwrap();
        tokio::task::yield_now().await;

        let memory = store.state.read().await;
        assert_eq!(memory.profiles.len(), 1);
        assert_eq!(memory.profiles[0].name, "First");
        assert_eq!(read_persisted_profiles(&store).profiles.len(), 1);
        drop(memory);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn profile_store_failure_keeps_disk_and_memory_at_previous_commit() {
        let root = temporary_test_path("profile-failure");
        let (store, control) = test_profile_store(&root);
        let mixer = MockMixerBackend::default().snapshot().await.unwrap();
        store.save("Durable".into(), mixer.clone()).await.unwrap();
        let before_disk = std::fs::read(&store.path).unwrap();
        let before_memory = serde_json::to_value(&*store.state.read().await).unwrap();

        control.fail_once();
        let error = store
            .save("Must Fail".into(), mixer.clone())
            .await
            .unwrap_err();
        assert!(matches!(error, BridgeError::Backend(_)));
        assert_eq!(std::fs::read(&store.path).unwrap(), before_disk);
        assert_eq!(
            serde_json::to_value(&*store.state.read().await).unwrap(),
            before_memory
        );
        assert!(store.get("Must Fail").await.is_none());

        store.save("After Failure".into(), mixer).await.unwrap();
        assert!(store.get("After Failure").await.is_some());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_health_exposes_the_hardware_write_gate() {
        let response = HealthResponse {
            ok: true,
            message: "studiobridge is ready",
            runtime: RuntimeInfo {
                studio_mode: "beacn",
                mixer_mode: "pipeweaver",
                hardware_writes_enabled: false,
                link_control_enabled: true,
                dsp_write_modules: vec!["headphone_equalizer"],
                dsp_write_active_module: Some("headphone_equalizer"),
                dsp_write_lease_seconds: Some(120),
            },
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["studio_mode"], "beacn");
        assert_eq!(json["mixer_mode"], "pipeweaver");
        assert_eq!(json["hardware_writes_enabled"], false);
        assert_eq!(json["link_control_enabled"], true);
        assert_eq!(json["dsp_write_modules"][0], "headphone_equalizer");
        assert_eq!(json["dsp_write_active_module"], "headphone_equalizer");
        assert_eq!(json["dsp_write_lease_seconds"], 120);
    }

    #[test]
    fn dsp_editing_accepts_only_complete_real_or_mock_backend_pairs() {
        assert!(dsp_editing_backends_supported("beacn", "pipeweaver"));
        assert!(dsp_editing_backends_supported("mock", "mock"));
        assert!(!dsp_editing_backends_supported("beacn", "mock"));
        assert!(!dsp_editing_backends_supported("mock", "pipeweaver"));
    }

    #[test]
    fn headphone_equalizer_writes_require_explicit_subwoofer_state() {
        let error = parse_microphone_dsp_update(serde_json::json!({
            "module": "headphone_equalizer",
            "state": { "bands": [] }
        }))
        .unwrap_err();
        assert!(error.0.to_string().contains("explicit subwoofer state"));
    }

    #[test]
    fn profile_names_match_beacn_numbering() {
        assert_eq!(
            next_profile_name("New Profile", ["Default Profile"].into_iter(), false),
            "New Profile"
        );
        assert_eq!(
            next_profile_name(
                "New Profile",
                ["New Profile", "New Profile 1"].into_iter(),
                false,
            ),
            "New Profile 2"
        );
        assert_eq!(
            next_profile_name(
                "Streaming",
                ["Streaming", "Streaming (1)"].into_iter(),
                true,
            ),
            "Streaming (2)"
        );
    }
}
