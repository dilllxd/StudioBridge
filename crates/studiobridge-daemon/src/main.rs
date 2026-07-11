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
    MockMixerBackend, MockStudioBackend, RemoveMixerSourceRequest, SetLinkAssignmentRequest,
    SetMicrophoneRequest, SetMixerApplicationRequest, SetMuteRequest, SetRouteRequest,
    SetTargetVolumeRequest, SetVolumeLinkedRequest, SetVolumeRequest, StudioBackend,
    StudioBridgeService,
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

#[derive(Clone)]
struct ProfileStore {
    path: PathBuf,
    state: Arc<RwLock<ProfileFile>>,
}

impl ProfileStore {
    fn load() -> Self {
        let path = daemon_config_root().join("mixer-profiles.json");
        let state = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            state: Arc::new(RwLock::new(state)),
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
        let mut state = self.state.write().await;
        if let Some(profile) = state
            .profiles
            .iter_mut()
            .find(|profile| profile.name == name)
        {
            profile.mixer = mixer;
        } else {
            state.profiles.push(SavedMixerProfile {
                name: name.clone(),
                mixer,
            });
        }
        state.active = Some(name);
        self.persist(&state)
    }

    async fn set_active(&self, name: &str) -> Result<(), BridgeError> {
        let mut state = self.state.write().await;
        state.active = Some(name.to_owned());
        self.persist(&state)
    }

    async fn delete(&self, name: &str) -> Result<(), BridgeError> {
        let mut state = self.state.write().await;
        let before = state.profiles.len();
        state.profiles.retain(|profile| profile.name != name);
        if state.profiles.len() == before {
            return Err(BridgeError::InvalidValue(format!(
                "unknown mixer profile: {name}"
            )));
        }
        if state.active.as_deref() == Some(name) {
            state.active = None;
        }
        self.persist(&state)
    }

    fn persist(&self, state: &ProfileFile) -> Result<(), BridgeError> {
        let parent = self.path.parent().ok_or_else(|| {
            BridgeError::Backend("mixer profile path has no parent directory".into())
        })?;
        std::fs::create_dir_all(parent).map_err(|error| BridgeError::Backend(error.to_string()))?;
        let json = serde_json::to_vec_pretty(state)
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        std::fs::write(&self.path, json).map_err(|error| BridgeError::Backend(error.to_string()))
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
        .route("/api/mixer/volume-link", post(set_volume_linked))
        .route("/api/mixer/mute", post(set_mute))
        .route("/api/mixer/route", post(set_route))
        .route("/api/mixer/source", post(create_mixer_source))
        .route("/api/mixer/source/remove", post(remove_mixer_source))
        .route("/api/mixer/profiles", get(list_mixer_profiles))
        .route("/api/mixer/profile/save", post(save_mixer_profile))
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
    Json(update): Json<MicrophoneDspUpdate>,
) -> Result<Json<studiobridge_core::MicrophoneDspWriteResult>, ApiError> {
    Ok(Json(state.service.set_microphone_dsp(update).await?))
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
    if state.runtime.studio_mode != "beacn" || state.runtime.mixer_mode != "pipeweaver" {
        return Err(ApiError(BridgeError::BackendUnavailable(
            "DSP editing requires the real BEACN and PipeWeaver backends".into(),
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

async fn remove_mixer_source(
    State(state): State<AppState>,
    Json(request): Json<RemoveMixerSourceRequest>,
) -> Result<Json<ApiMessage>, ApiError> {
    state.service.remove_source(&request.source_id).await?;
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
                dsp_write_lease_seconds: Some(120),
            },
        };
        let json = serde_json::to_value(response).unwrap();
        assert_eq!(json["studio_mode"], "beacn");
        assert_eq!(json["mixer_mode"], "pipeweaver");
        assert_eq!(json["hardware_writes_enabled"], false);
        assert_eq!(json["link_control_enabled"], true);
        assert_eq!(json["dsp_write_modules"][0], "headphone_equalizer");
        assert_eq!(json["dsp_write_lease_seconds"], 120);
    }
}
