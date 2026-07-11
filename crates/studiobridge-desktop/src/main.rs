use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use slint::{Color, Model, ModelRc, SharedString, VecModel, Weak};
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use ashpd::{
    AppID,
    desktop::{
        CreateSessionOptions,
        global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut},
    },
};
#[cfg(target_os = "linux")]
use futures_util::StreamExt;
#[cfg(unix)]
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
};
use studiobridge_core::{
    AppSnapshot, DspMode, DspWriteModule, LinkChannel, MicrophoneDspSnapshot, MicrophoneDspUpdate,
    MixBus, MixerChannel, MuteState,
};

mod client;
use client::DaemonClient;

slint::include_modules!();

const METER_URL: &str = "ws://127.0.0.1:14565/api/websocket/meter";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppPreferences {
    start_at_login: bool,
    close_to_tray: bool,
    save_confirmation: bool,
    beta_opt_in: bool,
    mixing_suite_enabled: bool,
    automatic_default_reset: bool,
    meter_crossfade: bool,
    hotkeys: HashMap<String, String>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            start_at_login: false,
            close_to_tray: true,
            save_confirmation: true,
            beta_opt_in: false,
            mixing_suite_enabled: true,
            automatic_default_reset: true,
            meter_crossfade: true,
            hotkeys: HashMap::new(),
        }
    }
}

fn config_root() -> std::path::PathBuf {
    #[cfg(windows)]
    if let Some(app_data) = std::env::var_os("APPDATA") {
        return std::path::PathBuf::from(app_data).join("StudioBridge");
    }
    #[cfg(windows)]
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return std::path::PathBuf::from(profile)
            .join("AppData")
            .join("Roaming")
            .join("StudioBridge");
    }

    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".config"))
        })
        .or_else(|| std::env::var_os("APPDATA").map(std::path::PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
        .join("studiobridge")
}

fn load_preferences() -> AppPreferences {
    let path = config_root().join("settings.json");
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_preferences(preferences: &AppPreferences) -> Result<(), String> {
    let root = config_root();
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(preferences).map_err(|error| error.to_string())?;
    std::fs::write(root.join("settings.json"), json).map_err(|error| error.to_string())?;
    sync_linux_autostart(preferences.start_at_login).map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn sync_linux_autostart(enabled: bool) -> std::io::Result<()> {
    let autostart = config_root()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("autostart");
    let entry = autostart.join("studiobridge.desktop");
    if !enabled {
        match std::fs::remove_file(entry) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        }
    }
    std::fs::create_dir_all(&autostart)?;
    let executable = std::env::current_exe()?;
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName=StudioBridge\nExec=\"{}\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        executable.display()
    );
    std::fs::write(entry, contents)
}

#[cfg(not(target_os = "linux"))]
fn sync_linux_autostart(_enabled: bool) -> std::io::Result<()> {
    Ok(())
}

#[derive(Debug, Deserialize)]
struct MeterEvent {
    id: String,
    percent: f32,
}

struct HotkeyRuntime {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<HotKey>,
    actions: Arc<Mutex<HashMap<u32, String>>>,
    #[cfg(target_os = "linux")]
    portal_generation: Arc<AtomicU64>,
    #[cfg(target_os = "linux")]
    portal_client: DaemonClient,
    #[cfg(target_os = "linux")]
    portal_window: Weak<MainWindow>,
}

impl HotkeyRuntime {
    fn new(preferences: &AppPreferences, client: DaemonClient, window: Weak<MainWindow>) -> Self {
        #[cfg(target_os = "linux")]
        let portal_client = client.clone();
        #[cfg(target_os = "linux")]
        let portal_window = window.clone();
        let actions = Arc::new(Mutex::new(HashMap::<u32, String>::new()));
        let event_actions = actions.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            let action = event_actions
                .lock()
                .ok()
                .and_then(|actions| actions.get(&event.id).cloned());
            if let Some(action) = action {
                execute_hotkey_action(action, client.clone(), window.clone());
            }
        }));

        let manager = if using_wayland() {
            None
        } else {
            GlobalHotKeyManager::new().ok()
        };
        let mut runtime = Self {
            manager,
            registered: Vec::new(),
            actions,
            #[cfg(target_os = "linux")]
            portal_generation: Arc::new(AtomicU64::new(0)),
            #[cfg(target_os = "linux")]
            portal_client,
            #[cfg(target_os = "linux")]
            portal_window,
        };
        runtime.reload(preferences);
        runtime
    }

    fn reload(&mut self, preferences: &AppPreferences) {
        if using_wayland() {
            #[cfg(target_os = "linux")]
            self.reload_wayland(preferences);
            return;
        }
        let Some(manager) = &self.manager else {
            return;
        };
        if !self.registered.is_empty() {
            let _ = manager.unregister_all(&self.registered);
        }
        self.registered.clear();
        if let Ok(mut actions) = self.actions.lock() {
            actions.clear();
            for (action, binding) in &preferences.hotkeys {
                let Ok(hotkey) = binding.parse::<HotKey>() else {
                    continue;
                };
                if manager.register(hotkey).is_ok() {
                    actions.insert(hotkey.id(), action.clone());
                    self.registered.push(hotkey);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn reload_wayland(&mut self, preferences: &AppPreferences) {
        let generation = self.portal_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let bindings = preferences
            .hotkeys
            .iter()
            .filter(|(_, binding)| !binding.trim().is_empty())
            .map(|(action, binding)| (action.clone(), binding.clone()))
            .collect::<Vec<_>>();
        if bindings.is_empty() {
            return;
        }

        let generation_state = self.portal_generation.clone();
        let client = self.portal_client.clone();
        let window = self.portal_window.clone();
        thread::spawn(move || {
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(runtime) => runtime,
                Err(error) => {
                    set_status(window, format!("Wayland hotkeys unavailable: {error}"));
                    return;
                }
            };
            let result = runtime.block_on(run_wayland_hotkeys(
                bindings,
                generation,
                generation_state,
                client,
                window.clone(),
            ));
            if let Err(error) = result {
                set_status(window, format!("Wayland hotkeys unavailable: {error}"));
            }
        });
    }
}

#[cfg(target_os = "linux")]
async fn run_wayland_hotkeys(
    bindings: Vec<(String, String)>,
    generation: u64,
    generation_state: Arc<AtomicU64>,
    client: DaemonClient,
    window: Weak<MainWindow>,
) -> Result<(), String> {
    if let Ok(app_id) = AppID::try_from("io.github.dilllxd.StudioBridge") {
        // Registration is optional on older portals, so a missing Registry
        // interface must not prevent GlobalShortcuts from working.
        let _ = ashpd::register_host_app(app_id).await;
    }

    let portal = GlobalShortcuts::new()
        .await
        .map_err(|error| error.to_string())?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await
        .map_err(|error| error.to_string())?;

    let mut actions = HashMap::new();
    let shortcuts = bindings
        .iter()
        .enumerate()
        .map(|(index, (action, binding))| {
            let id = format!("studiobridge_{index}");
            actions.insert(id.clone(), action.clone());
            NewShortcut::new(id, hotkey_description(action))
                .preferred_trigger(Some(binding.as_str()))
        })
        .collect::<Vec<_>>();
    let request = portal
        .bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default())
        .await
        .map_err(|error| error.to_string())?;
    request.response().map_err(|error| error.to_string())?;
    set_status(window.clone(), "Wayland hotkeys registered".into());

    let mut activated = portal
        .receive_activated()
        .await
        .map_err(|error| error.to_string())?;
    loop {
        tokio::select! {
            event = activated.next() => {
                let Some(event) = event else { break; };
                if let Some(action) = actions.get(event.shortcut_id()) {
                    execute_hotkey_action(action.clone(), client.clone(), window.clone());
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if generation_state.load(Ordering::SeqCst) != generation {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn hotkey_description(action: &str) -> String {
    if let Some(profile) = action.strip_prefix("profile:") {
        format!("Load mixer profile {profile}")
    } else if let Some(channel) = action.strip_prefix("mute:") {
        format!("Toggle mute for {channel}")
    } else if action == "mix:toggle-personal-device" {
        "Toggle Personal Mix device".into()
    } else {
        action.replace([':', '-'], " ")
    }
}

fn using_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn execute_hotkey_action(action: String, client: DaemonClient, window: Weak<MainWindow>) {
    thread::spawn(move || {
        if let Some(profile) = action.strip_prefix("profile:") {
            match client.load_mixer_profile(profile) {
                Ok(()) => set_status(window.clone(), format!("Loaded mixer profile {profile}")),
                Err(error) => set_status(window.clone(), format!("Hotkey failed: {error}")),
            }
        } else if let Some(channel_id) = action.strip_prefix("mute:") {
            let current = client.snapshot().ok().and_then(|snapshot| {
                snapshot
                    .mixer
                    .channels
                    .into_iter()
                    .find(|channel| channel.id == channel_id)
            });
            if let Some(current) = current {
                let next = if current.mute_state == MuteState::Unmuted {
                    MuteState::MutedAll
                } else {
                    MuteState::Unmuted
                };
                if let Err(error) = client.set_mute(channel_id, next) {
                    set_status(window.clone(), format!("Hotkey failed: {error}"));
                }
            }
        } else if action == "mix:toggle-personal-device" {
            set_status(
                window.clone(),
                "Personal device cycling needs at least two PipeWeaver outputs".into(),
            );
        }
        refresh_all(window, client);
    });
}

#[cfg(unix)]
struct InstanceGuard {
    socket_path: PathBuf,
}

#[cfg(not(unix))]
struct InstanceGuard;

#[cfg(unix)]
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[derive(Default)]
struct DspSession {
    snapshot: Option<MicrophoneDspSnapshot>,
    captured: Option<MicrophoneDspSnapshot>,
    selected: DspSelection,
}

#[derive(Clone, Copy, Default)]
enum DspSelection {
    Equalizer,
    Compressor,
    Expander,
    #[default]
    NoiseSuppression,
    EnhancementSuite,
    HeadphoneEqualizer,
}

impl DspSelection {
    fn parse(value: &str) -> Self {
        match value {
            "equalizer" => Self::Equalizer,
            "compressor" => Self::Compressor,
            "expander" => Self::Expander,
            "noise_suppression" => Self::NoiseSuppression,
            "enhancement_suite" => Self::EnhancementSuite,
            _ => Self::HeadphoneEqualizer,
        }
    }

    const fn module(self) -> DspWriteModule {
        match self {
            Self::Equalizer => DspWriteModule::Equalizer,
            Self::Compressor => DspWriteModule::Compressor,
            Self::Expander => DspWriteModule::Expander,
            Self::NoiseSuppression => DspWriteModule::NoiseSuppression,
            Self::EnhancementSuite => DspWriteModule::EnhancementSuite,
            Self::HeadphoneEqualizer => DspWriteModule::HeadphoneEqualizer,
        }
    }

    const fn as_str(self) -> &'static str {
        self.module().as_str()
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let start_in_background = std::env::args().any(|argument| argument == "--background");
    if notify_existing_instance(start_in_background) {
        return Ok(());
    }
    let window = MainWindow::new()?;
    let tray = StudioBridgeTray::new()?;
    let Some(_instance_guard) = claim_single_instance(window.as_weak(), start_in_background) else {
        return Ok(());
    };
    let client = DaemonClient::localhost().expect("failed to create daemon client");
    let dsp_session = Arc::new(Mutex::new(DspSession::default()));
    let preferences = load_preferences();
    window.set_start_at_login(preferences.start_at_login);
    window.set_close_to_tray(preferences.close_to_tray);
    window.set_save_confirmation(preferences.save_confirmation);
    window.set_beta_opt_in(preferences.beta_opt_in);
    window.set_mixing_suite_enabled(preferences.mixing_suite_enabled);
    window.set_automatic_default_reset(preferences.automatic_default_reset);
    window.set_meter_crossfade(preferences.meter_crossfade);
    let hotkey_runtime = Rc::new(RefCell::new(HotkeyRuntime::new(
        &preferences,
        client.clone(),
        window.as_weak(),
    )));

    window.set_sources(empty_sources());
    window.set_routes(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_linux_applications(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_link_applications(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_targets(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_mixer_profiles(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_hotkeys(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_mixer_channel_names(string_model(vec!["Unassigned".into()]));
    window.set_link_channel_names(string_model(vec![
        "System".into(),
        "Link 1".into(),
        "Link 2".into(),
        "Link 3".into(),
        "Link 4".into(),
    ]));

    let preferences_window = window.as_weak();
    window.on_save_preferences(
        move |start_at_login,
              close_to_tray,
              save_confirmation,
              beta_opt_in,
              mixing_suite_enabled,
              automatic_default_reset,
              meter_crossfade| {
            let preferences = AppPreferences {
                start_at_login,
                close_to_tray,
                save_confirmation,
                beta_opt_in,
                mixing_suite_enabled,
                automatic_default_reset,
                meter_crossfade,
                hotkeys: load_preferences().hotkeys,
            };
            match save_preferences(&preferences) {
                Ok(()) => set_status(preferences_window.clone(), "Settings saved".into()),
                Err(error) => set_status(
                    preferences_window.clone(),
                    format!("Settings save failed: {error}"),
                ),
            }
        },
    );

    let hotkey_window = window.as_weak();
    let hotkey_client = client.clone();
    let hotkey_runtime_for_save = hotkey_runtime.clone();
    window.on_save_hotkey(move |action, binding| {
        let action = action.trim().to_string();
        let binding = binding.trim().to_string();
        let mut preferences = load_preferences();
        if binding.is_empty() {
            preferences.hotkeys.remove(&action);
        } else {
            preferences.hotkeys.insert(action, binding);
        }
        match save_preferences(&preferences) {
            Ok(()) => {
                hotkey_runtime_for_save.borrow_mut().reload(&preferences);
                let status = if using_wayland() {
                    "Hotkey saved; Wayland portal registration is pending"
                } else {
                    "Hotkey assignment registered"
                };
                set_status(hotkey_window.clone(), status.into());
            }
            Err(error) => set_status(
                hotkey_window.clone(),
                format!("Hotkey save failed: {error}"),
            ),
        }
        refresh_all(hotkey_window.clone(), hotkey_client.clone());
    });

    let refresh_window = window.as_weak();
    let refresh_client = client.clone();
    window.on_refresh(move || refresh_all(refresh_window.clone(), refresh_client.clone()));

    let volume_window = window.as_weak();
    let volume_client = client.clone();
    window.on_set_volume(move |channel_id, mix, value| {
        let window = volume_window.clone();
        let client = volume_client.clone();
        let channel_id = channel_id.to_string();
        let mix = if mix == "personal" {
            MixBus::Personal
        } else {
            MixBus::Audience
        };
        thread::spawn(move || {
            let volume = value.round().clamp(0.0, 100.0) as u8;
            if let Err(error) = client.set_volume(&channel_id, mix, volume) {
                set_status(window.clone(), format!("Volume update failed: {error}"));
            }
            refresh_all(window, client);
        });
    });

    let target_volume_window = window.as_weak();
    let target_volume_client = client.clone();
    window.on_set_target_volume(move |target_id, value| {
        let window = target_volume_window.clone();
        let client = target_volume_client.clone();
        let target_id = target_id.to_string();
        thread::spawn(move || {
            let volume = value.round().clamp(0.0, 100.0) as u8;
            if let Err(error) = client.set_target_volume(&target_id, volume) {
                set_status(
                    window.clone(),
                    format!("Output volume update failed: {error}"),
                );
            }
            refresh_all(window, client);
        });
    });

    let mute_window = window.as_weak();
    let mute_client = client.clone();
    window.on_set_source_mute(move |channel_id, mix, muted| {
        let window = mute_window.clone();
        let client = mute_client.clone();
        let channel_id = channel_id.to_string();
        let mix = mix.to_string();
        thread::spawn(move || {
            let current = client.snapshot().ok().and_then(|snapshot| {
                snapshot
                    .mixer
                    .channels
                    .into_iter()
                    .find(|channel| channel.id == channel_id)
            });
            let Some(current) = current else {
                set_status(window, "Could not read the current mute state".into());
                return;
            };
            let personal = if mix == "personal" {
                muted
            } else {
                matches!(
                    current.mute_state,
                    MuteState::MutedAll | MuteState::MutedPersonal
                )
            };
            let audience = if mix == "audience" {
                muted
            } else {
                matches!(
                    current.mute_state,
                    MuteState::MutedAll | MuteState::MutedAudience
                )
            };
            let state = match (personal, audience) {
                (false, false) => MuteState::Unmuted,
                (true, true) => MuteState::MutedAll,
                (true, false) => MuteState::MutedPersonal,
                (false, true) => MuteState::MutedAudience,
            };
            if let Err(error) = client.set_mute(&channel_id, state) {
                set_status(window.clone(), format!("Mute update failed: {error}"));
            }
            refresh_all(window, client);
        });
    });

    let link_volume_window = window.as_weak();
    let link_volume_client = client.clone();
    window.on_set_volume_linked(move |channel_id, linked| {
        let window = link_volume_window.clone();
        let client = link_volume_client.clone();
        let channel_id = channel_id.to_string();
        thread::spawn(move || {
            if let Err(error) = client.set_volume_linked(&channel_id, linked) {
                set_status(window.clone(), format!("Level link update failed: {error}"));
            }
            refresh_all(window, client);
        });
    });

    let route_window = window.as_weak();
    let route_client = client.clone();
    window.on_set_route(move |source_id, target_id, enabled| {
        let window = route_window.clone();
        let client = route_client.clone();
        let source_id = source_id.to_string();
        let target_id = target_id.to_string();
        thread::spawn(move || {
            match client.set_route(&source_id, &target_id, enabled) {
                Ok(()) => set_status(window.clone(), "Software route updated".into()),
                Err(error) => set_status(window.clone(), format!("Route update failed: {error}")),
            }
            refresh_all(window, client);
        });
    });

    let create_source_window = window.as_weak();
    let create_source_client = client.clone();
    window.on_create_source(move |name| {
        let window = create_source_window.clone();
        let client = create_source_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            match client.create_source(&name) {
                Ok(()) => set_status(window.clone(), format!("Added {name} mixer knob")),
                Err(error) => set_status(window.clone(), format!("Add knob failed: {error}")),
            }
            refresh_all(window, client);
        });
    });

    let remove_source_window = window.as_weak();
    let remove_source_client = client.clone();
    window.on_remove_source(move |source_id| {
        let window = remove_source_window.clone();
        let client = remove_source_client.clone();
        let source_id = source_id.to_string();
        thread::spawn(move || {
            match client.remove_source(&source_id) {
                Ok(()) => set_status(window.clone(), "Mixer knob removed".into()),
                Err(error) => set_status(window.clone(), format!("Remove knob failed: {error}")),
            }
            refresh_all(window, client);
        });
    });

    let save_profile_window = window.as_weak();
    let save_profile_client = client.clone();
    window.on_save_mixer_profile(move |name| {
        let window = save_profile_window.clone();
        let client = save_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            match client.save_mixer_profile(&name) {
                Ok(()) => set_status(window.clone(), format!("Saved mixer profile {name}")),
                Err(error) => set_status(window.clone(), format!("Profile save failed: {error}")),
            }
            refresh_all(window, client);
        });
    });

    let load_profile_window = window.as_weak();
    let load_profile_client = client.clone();
    window.on_load_mixer_profile(move |name| {
        let window = load_profile_window.clone();
        let client = load_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            match client.load_mixer_profile(&name) {
                Ok(()) => set_status(window.clone(), format!("Loaded mixer profile {name}")),
                Err(error) => set_status(window.clone(), format!("Profile load failed: {error}")),
            }
            refresh_all(window, client);
        });
    });

    let delete_profile_window = window.as_weak();
    let delete_profile_client = client.clone();
    window.on_delete_mixer_profile(move |name| {
        let window = delete_profile_window.clone();
        let client = delete_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            match client.delete_mixer_profile(&name) {
                Ok(()) => set_status(window.clone(), format!("Deleted mixer profile {name}")),
                Err(error) => set_status(window.clone(), format!("Profile delete failed: {error}")),
            }
            refresh_all(window, client);
        });
    });

    let linux_app_window = window.as_weak();
    let linux_app_client = client.clone();
    window.on_assign_linux_application(move |process, name, channel_index| {
        let window = linux_app_window.clone();
        let client = linux_app_client.clone();
        let process = process.to_string();
        let name = name.to_string();
        thread::spawn(move || {
            let channel_id = if channel_index <= 0 {
                None
            } else {
                client.snapshot().ok().and_then(|snapshot| {
                    snapshot
                        .mixer
                        .channels
                        .get((channel_index - 1) as usize)
                        .map(|channel| channel.id.clone())
                })
            };
            match client.set_mixer_application(&process, &name, channel_id) {
                Ok(()) => set_status(window.clone(), "Linux application assigned".into()),
                Err(error) => set_status(
                    window.clone(),
                    format!("Application assignment failed: {error}"),
                ),
            }
            refresh_all(window, client);
        });
    });

    let link_app_window = window.as_weak();
    let link_app_client = client.clone();
    window.on_assign_link_application(move |application, channel_index| {
        let window = link_app_window.clone();
        let client = link_app_client.clone();
        let application = application.to_string();
        let channel = match channel_index {
            1 => LinkChannel::Link1,
            2 => LinkChannel::Link2,
            3 => LinkChannel::Link3,
            4 => LinkChannel::Link4,
            _ => LinkChannel::System,
        };
        thread::spawn(move || {
            match client.set_link_application(&application, channel) {
                Ok(()) => set_status(window.clone(), "Windows Link assignment updated".into()),
                Err(error) => {
                    set_status(window.clone(), format!("Link assignment failed: {error}"))
                }
            }
            refresh_all(window, client);
        });
    });

    let select_window = window.as_weak();
    let select_session = dsp_session.clone();
    window.on_select_dsp_module(move |module| {
        if let Ok(mut session) = select_session.lock() {
            session.selected = DspSelection::parse(module.as_str());
            if let Some(snapshot) = session.snapshot.clone() {
                show_dsp_snapshot(&select_window, &snapshot, session.selected);
            }
        }
    });

    let load_window = window.as_weak();
    let load_client = client.clone();
    let load_session = dsp_session.clone();
    window.on_load_dsp(move || {
        load_dsp(
            load_window.clone(),
            load_client.clone(),
            load_session.clone(),
            false,
        )
    });

    let arm_window = window.as_weak();
    let arm_client = client.clone();
    let arm_session = dsp_session.clone();
    window.on_arm_dsp(move || arm_dsp(arm_window.clone(), arm_client.clone(), arm_session.clone()));

    let disarm_window = window.as_weak();
    let disarm_client = client.clone();
    window.on_disarm_dsp(move || {
        let window = disarm_window.clone();
        let client = disarm_client.clone();
        thread::spawn(move || match client.disarm_dsp() {
            Ok(()) => {
                set_status(window.clone(), "DSP editing disarmed".into());
                set_dsp_lease(window, None, None);
            }
            Err(error) => set_status(window, format!("Could not disarm DSP editing: {error}")),
        });
    });

    let apply_window = window.as_weak();
    let apply_client = client.clone();
    let apply_session = dsp_session.clone();
    window.on_apply_dsp(move |a, b, c| {
        apply_dsp(
            apply_window.clone(),
            apply_client.clone(),
            apply_session.clone(),
            a,
            b,
            c,
            false,
        )
    });

    let revert_window = window.as_weak();
    let revert_client = client.clone();
    let revert_session = dsp_session.clone();
    window.on_revert_dsp(move || {
        apply_dsp(
            revert_window.clone(),
            revert_client.clone(),
            revert_session.clone(),
            0.0,
            0.0,
            0.0,
            true,
        )
    });

    let show_window = window.as_weak();
    tray.on_show_app(move || {
        if let Some(window) = show_window.upgrade() {
            let _ = window.show();
        }
    });

    let safe_window = window.as_weak();
    tray.on_show_safe_mode(move || {
        if let Some(window) = safe_window.upgrade() {
            window.set_active_page(1);
            let _ = window.show();
        }
    });

    let quit_client = client.clone();
    tray.on_quit_app(move || {
        let _ = quit_client.disarm_dsp();
        let _ = slint::quit_event_loop();
    });

    refresh_all(window.as_weak(), client.clone());
    load_dsp(window.as_weak(), client.clone(), dsp_session, false);
    start_health_monitor(window.as_weak(), client.clone());
    start_meter_stream(window.as_weak(), client);
    if !start_in_background {
        window.show()?;
    }
    tray.show()?;
    slint::run_event_loop()
}

#[cfg(unix)]
fn instance_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("studiobridge-desktop.sock")
}

#[cfg(unix)]
fn notify_existing_instance(start_in_background: bool) -> bool {
    let Ok(mut existing) = UnixStream::connect(instance_socket_path()) else {
        return false;
    };
    let command = if start_in_background {
        b"ping\n"
    } else {
        b"show\n"
    };
    existing.write_all(command).is_ok()
}

#[cfg(not(unix))]
fn notify_existing_instance(_start_in_background: bool) -> bool {
    false
}

#[cfg(unix)]
fn claim_single_instance(
    window: Weak<MainWindow>,
    start_in_background: bool,
) -> Option<InstanceGuard> {
    let socket_path = instance_socket_path();

    if let Ok(mut existing) = UnixStream::connect(&socket_path) {
        let command = if start_in_background {
            b"ping\n"
        } else {
            b"show\n"
        };
        let _ = existing.write_all(command);
        return None;
    }
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("StudioBridge could not create its single-instance socket: {error}");
            return None;
        }
    };
    let _ = fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600));
    thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut command = String::new();
            if stream.read_to_string(&mut command).is_ok() && command.trim() == "show" {
                let window = window.clone();
                if slint::invoke_from_event_loop(move || {
                    if let Some(window) = window.upgrade() {
                        let _ = window.show();
                    }
                })
                .is_err()
                {
                    break;
                }
            }
        }
    });
    Some(InstanceGuard { socket_path })
}

#[cfg(not(unix))]
fn claim_single_instance(
    _window: Weak<MainWindow>,
    _start_in_background: bool,
) -> Option<InstanceGuard> {
    Some(InstanceGuard)
}

fn source(channel: &MixerChannel) -> SourceModel {
    SourceModel {
        channel_id: SharedString::from(&channel.id),
        meter_id: SharedString::from(&channel.meter_id),
        name: SharedString::from(&channel.name),
        detail: SharedString::from(channel_detail(channel)),
        personal: channel.personal_volume as f32,
        audience: channel.audience_volume as f32,
        meter: 0.0,
        linked: channel.volumes_linked,
        personal_muted: matches!(
            channel.mute_state,
            MuteState::MutedAll | MuteState::MutedPersonal
        ),
        audience_muted: matches!(
            channel.mute_state,
            MuteState::MutedAll | MuteState::MutedAudience
        ),
        colour: parse_colour(&channel.colour),
    }
}

fn empty_sources() -> ModelRc<SourceModel> {
    ModelRc::from(Rc::new(VecModel::from(Vec::new())))
}

fn string_model(values: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(
        values
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )))
}

fn refresh_all(window: Weak<MainWindow>, client: DaemonClient) {
    thread::spawn(move || {
        let health = client.health();
        let snapshot = client.snapshot();
        let profiles = client.mixer_profiles();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            let profile_names = profiles
                .as_ref()
                .map(|profiles| {
                    profiles
                        .profiles
                        .iter()
                        .map(|profile| profile.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            match health {
                Ok(health) => {
                    let safe = !health.hardware_writes_enabled;
                    window.set_daemon_connected(health.ok);
                    window.set_hardware_safe(safe);
                    window.set_link_active(health.link_control_enabled);
                    window.set_dsp_gate_count(health.dsp_write_modules.len() as i32);
                    window.set_status_text(
                        format!("{} + {}", health.studio_mode, health.mixer_mode).into(),
                    );
                }
                Err(error) => {
                    window.set_daemon_connected(false);
                    window.set_hardware_safe(true);
                    window.set_status_text(format!("Daemon unavailable: {error}").into());
                }
            }
            match snapshot {
                Ok(snapshot) => apply_snapshot(&window, snapshot, &profile_names),
                Err(error) => {
                    window.set_status_text(format!("Snapshot unavailable: {error}").into())
                }
            }
            if let Ok(profiles) = profiles {
                window.set_mixer_profiles(ModelRc::from(Rc::new(VecModel::from(
                    profiles
                        .profiles
                        .into_iter()
                        .map(|profile| ProfileModel {
                            name: profile.name.into(),
                            active: profile.active,
                        })
                        .collect::<Vec<_>>(),
                ))));
            }
        });
    });
}

fn apply_snapshot(window: &MainWindow, snapshot: AppSnapshot, profile_names: &[String]) {
    let preferences = load_preferences();
    let mut hotkeys = vec![hotkey_entry(
        "MIX DEVICES",
        "Toggle Personal Mix Device",
        "mix:toggle-personal-device",
        &preferences,
    )];
    for (index, profile) in profile_names.iter().enumerate() {
        hotkeys.push(hotkey_entry(
            if index == 0 { "MIXER PROFILES" } else { "" },
            profile,
            &format!("profile:{profile}"),
            &preferences,
        ));
    }
    for (index, channel) in snapshot.mixer.channels.iter().enumerate() {
        hotkeys.push(hotkey_entry(
            if index == 0 { "MUTES" } else { "" },
            &format!("{} - Mute", channel.name),
            &format!("mute:{}", channel.id),
            &preferences,
        ));
    }
    window.set_hotkeys(ModelRc::from(Rc::new(VecModel::from(hotkeys))));
    window.set_device_name(snapshot.studio.identity.product.clone().into());
    window.set_device_serial(
        snapshot
            .studio
            .identity
            .serial
            .clone()
            .unwrap_or_else(|| "Unavailable".into())
            .into(),
    );
    window.set_device_firmware(
        snapshot
            .studio
            .identity
            .firmware
            .clone()
            .unwrap_or_else(|| "Unavailable".into())
            .into(),
    );
    let mut ordered_targets = snapshot.mixer.targets.iter().collect::<Vec<_>>();
    ordered_targets.sort_by_key(|target| match target.name.as_str() {
        "Headphones" => 0,
        "Audience Mix" => 1,
        "Voice Chat Mic" => 2,
        "VOD Track" => 3,
        _ => 4,
    });
    let active_routes = snapshot
        .mixer
        .routes
        .iter()
        .map(|route| (route.source_id.as_str(), route.target_id.as_str()))
        .collect::<HashSet<_>>();
    let mut routes = Vec::new();
    for (row, target) in ordered_targets.iter().enumerate() {
        for (column, channel) in snapshot.mixer.channels.iter().enumerate() {
            routes.push(RouteModel {
                source_id: channel.id.clone().into(),
                source_name: channel.name.clone().into(),
                target_id: target.id.clone().into(),
                target_name: target.name.clone().into(),
                target_mix: match target.mix {
                    MixBus::Personal => "Personal".into(),
                    MixBus::Audience => "Audience".into(),
                },
                colour: parse_colour(&channel.colour),
                enabled: active_routes.contains(&(channel.id.as_str(), target.id.as_str())),
                row: row as i32,
                column: column as i32,
            });
        }
    }
    let linux_applications = snapshot
        .mixer
        .applications
        .iter()
        .map(|application| ApplicationModel {
            process: application.process.clone().into(),
            name: application.name.clone().into(),
            title: application
                .title
                .as_deref()
                .unwrap_or(&application.name)
                .into(),
            channel_index: application
                .channel_id
                .as_ref()
                .and_then(|id| {
                    snapshot
                        .mixer
                        .channels
                        .iter()
                        .position(|channel| &channel.id == id)
                })
                .map_or(0, |index| index as i32 + 1),
        })
        .collect::<Vec<_>>();
    let link_applications = snapshot
        .studio
        .linked_applications
        .iter()
        .map(|application| LinkApplicationModel {
            name: application.name.clone().into(),
            channel_index: match application.channel {
                LinkChannel::System => 0,
                LinkChannel::Link1 => 1,
                LinkChannel::Link2 => 2,
                LinkChannel::Link3 => 3,
                LinkChannel::Link4 => 4,
            },
        })
        .collect::<Vec<_>>();
    let mut channel_names = vec!["Unassigned".to_string()];
    channel_names.extend(
        snapshot
            .mixer
            .channels
            .iter()
            .map(|channel| channel.name.clone()),
    );
    let sources = snapshot
        .mixer
        .channels
        .iter()
        .map(source)
        .collect::<Vec<_>>();
    let targets = ordered_targets
        .iter()
        .map(|target| TargetModel {
            target_id: target.id.clone().into(),
            meter_id: target.meter_id.clone().into(),
            name: target.name.clone().into(),
            mix: match target.mix {
                MixBus::Personal => "Personal".into(),
                MixBus::Audience => "Audience".into(),
            },
            volume: target.volume as f32,
            meter: 0.0,
            muted: target.muted,
        })
        .collect::<Vec<_>>();
    window.set_sources(ModelRc::from(Rc::new(VecModel::from(sources))));
    window.set_routes(ModelRc::from(Rc::new(VecModel::from(routes))));
    window.set_linux_applications(ModelRc::from(Rc::new(VecModel::from(linux_applications))));
    window.set_link_applications(ModelRc::from(Rc::new(VecModel::from(link_applications))));
    window.set_targets(ModelRc::from(Rc::new(VecModel::from(targets))));
    window.set_mixer_channel_names(string_model(channel_names));
}

fn hotkey_entry(
    section: &str,
    label: &str,
    action: &str,
    preferences: &AppPreferences,
) -> HotkeyModel {
    HotkeyModel {
        section: section.into(),
        label: label.into(),
        action: action.into(),
        binding: preferences
            .hotkeys
            .get(action)
            .cloned()
            .unwrap_or_default()
            .into(),
    }
}

fn channel_detail(channel: &MixerChannel) -> String {
    if channel.applications.is_empty() {
        match channel.source_kind {
            studiobridge_core::MixerSourceKind::Physical => "Physical input".into(),
            studiobridge_core::MixerSourceKind::Virtual => "Waiting for an application".into(),
        }
    } else {
        channel.applications.join(" · ")
    }
}

fn parse_colour(value: &str) -> Color {
    let encoded = value
        .strip_prefix('#')
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .unwrap_or(0x58d5cb);
    Color::from_argb_encoded(0xff00_0000 | encoded)
}

fn set_status(window: Weak<MainWindow>, status: String) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window.upgrade() {
            window.set_status_text(status.into());
        }
    });
}

fn load_dsp(
    window: Weak<MainWindow>,
    client: DaemonClient,
    session: Arc<Mutex<DspSession>>,
    capture: bool,
) {
    thread::spawn(move || match client.microphone_dsp() {
        Ok(snapshot) => {
            let selected = if let Ok(mut state) = session.lock() {
                state.snapshot = Some(snapshot.clone());
                if capture {
                    state.captured = Some(snapshot.clone());
                }
                state.selected
            } else {
                return;
            };
            show_dsp_snapshot(&window, &snapshot, selected);
            set_status(window, "Microphone DSP read-back is current".into());
        }
        Err(error) => set_status(window, format!("DSP read-back failed: {error}")),
    });
}

fn arm_dsp(window: Weak<MainWindow>, client: DaemonClient, session: Arc<Mutex<DspSession>>) {
    thread::spawn(move || {
        let selected = session
            .lock()
            .map(|state| state.selected)
            .unwrap_or_default();
        let snapshot = match client.microphone_dsp() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                set_status(window, format!("DSP safety read-back failed: {error}"));
                return;
            }
        };
        if let Ok(mut state) = session.lock() {
            state.snapshot = Some(snapshot.clone());
            state.captured = Some(snapshot.clone());
        }
        match client.arm_dsp(selected.module(), 300) {
            Ok(()) => {
                show_dsp_snapshot(&window, &snapshot, selected);
                set_dsp_lease(window.clone(), Some(selected.as_str().into()), Some(300));
                set_status(
                    window,
                    format!(
                        "{} editing armed for 5 minutes; original state captured",
                        selected.as_str()
                    ),
                );
            }
            Err(error) => set_status(window, format!("DSP editing was not armed: {error}")),
        }
    });
}

fn apply_dsp(
    window: Weak<MainWindow>,
    client: DaemonClient,
    session: Arc<Mutex<DspSession>>,
    a: f32,
    b: f32,
    c: f32,
    revert: bool,
) {
    thread::spawn(move || {
        let (selected, base) = match session.lock() {
            Ok(state) => {
                let snapshot = if revert {
                    state.captured.clone()
                } else {
                    state.snapshot.clone()
                };
                (state.selected, snapshot)
            }
            Err(_) => return,
        };
        let Some(base) = base else {
            set_status(
                window,
                "Load the microphone chain before applying changes".into(),
            );
            return;
        };
        let update = if revert {
            update_from_snapshot(selected, &base)
        } else {
            update_from_values(selected, base, a, b, c)
        };
        match client.set_microphone_dsp(&update) {
            Ok(result) if result.verified => {
                if let Ok(mut state) = session.lock() {
                    state.snapshot = Some(result.snapshot.clone());
                }
                show_dsp_snapshot(&window, &result.snapshot, selected);
                set_status(
                    window,
                    if revert {
                        "Captured DSP state restored and verified".into()
                    } else {
                        "DSP change applied and verified by read-back".into()
                    },
                );
            }
            Ok(_) => set_status(
                window,
                "DSP write did not verify; no success reported".into(),
            ),
            Err(error) => set_status(window, format!("DSP change rejected: {error}")),
        }
    });
}

fn update_from_snapshot(
    selected: DspSelection,
    snapshot: &MicrophoneDspSnapshot,
) -> MicrophoneDspUpdate {
    match selected {
        DspSelection::Equalizer => MicrophoneDspUpdate::Equalizer(snapshot.equalizer.clone()),
        DspSelection::Compressor => MicrophoneDspUpdate::Compressor(snapshot.compressor.clone()),
        DspSelection::Expander => MicrophoneDspUpdate::Expander(snapshot.expander.clone()),
        DspSelection::NoiseSuppression => {
            MicrophoneDspUpdate::NoiseSuppression(snapshot.noise_suppression.clone())
        }
        DspSelection::EnhancementSuite => {
            MicrophoneDspUpdate::EnhancementSuite(snapshot.enhancement_suite.clone())
        }
        DspSelection::HeadphoneEqualizer => {
            MicrophoneDspUpdate::HeadphoneEqualizer(snapshot.headphone_equalizer.clone())
        }
    }
}

fn update_from_values(
    selected: DspSelection,
    mut snapshot: MicrophoneDspSnapshot,
    a: f32,
    b: f32,
    c: f32,
) -> MicrophoneDspUpdate {
    match selected {
        DspSelection::Equalizer => {
            let profile = match snapshot.equalizer.active_mode {
                DspMode::Simple => &mut snapshot.equalizer.simple,
                DspMode::Advanced => &mut snapshot.equalizer.advanced,
            };
            for (band, value) in profile.bands.iter_mut().zip([a, b, c]) {
                band.gain_db = value;
            }
            MicrophoneDspUpdate::Equalizer(snapshot.equalizer)
        }
        DspSelection::Compressor => {
            let profile = match snapshot.compressor.active_mode {
                DspMode::Simple => &mut snapshot.compressor.simple,
                DspMode::Advanced => &mut snapshot.compressor.advanced,
            };
            profile.threshold_db = a;
            profile.ratio = b;
            profile.makeup_gain_db = c;
            MicrophoneDspUpdate::Compressor(snapshot.compressor)
        }
        DspSelection::Expander => {
            let profile = match snapshot.expander.active_mode {
                DspMode::Simple => &mut snapshot.expander.simple,
                DspMode::Advanced => &mut snapshot.expander.advanced,
            };
            profile.threshold_db = a;
            profile.ratio = b;
            profile.release_ms = c;
            MicrophoneDspUpdate::Expander(snapshot.expander)
        }
        DspSelection::NoiseSuppression => {
            snapshot.noise_suppression.amount_percent = a;
            snapshot.noise_suppression.sensitivity_db = b;
            snapshot.noise_suppression.adapt_time_ms = c;
            MicrophoneDspUpdate::NoiseSuppression(snapshot.noise_suppression)
        }
        DspSelection::EnhancementSuite => {
            snapshot.enhancement_suite.bass.amount = a;
            snapshot.enhancement_suite.de_esser.amount_percent = b;
            snapshot.enhancement_suite.exciter.amount_percent = c;
            MicrophoneDspUpdate::EnhancementSuite(snapshot.enhancement_suite)
        }
        DspSelection::HeadphoneEqualizer => {
            for (band, value) in snapshot.headphone_equalizer.bands.iter_mut().zip([a, b, c]) {
                band.amount_db = value;
            }
            MicrophoneDspUpdate::HeadphoneEqualizer(snapshot.headphone_equalizer)
        }
    }
}

fn show_dsp_snapshot(
    window: &Weak<MainWindow>,
    snapshot: &MicrophoneDspSnapshot,
    selected: DspSelection,
) {
    let (title, labels, values, ranges) = dsp_display(snapshot, selected);
    let window = window.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(window) = window.upgrade() else {
            return;
        };
        window.set_dsp_selected_module(selected.as_str().into());
        window.set_dsp_editor_title(title.into());
        window.set_dsp_label_a(labels.0.into());
        window.set_dsp_label_b(labels.1.into());
        window.set_dsp_label_c(labels.2.into());
        window.set_dsp_value_a(values.0);
        window.set_dsp_value_b(values.1);
        window.set_dsp_value_c(values.2);
        window.set_dsp_min_a(ranges.0.0);
        window.set_dsp_max_a(ranges.0.1);
        window.set_dsp_min_b(ranges.1.0);
        window.set_dsp_max_b(ranges.1.1);
        window.set_dsp_min_c(ranges.2.0);
        window.set_dsp_max_c(ranges.2.1);
    });
}

type DspDisplay = (
    &'static str,
    (&'static str, &'static str, &'static str),
    (f32, f32, f32),
    ((f32, f32), (f32, f32), (f32, f32)),
);

fn dsp_display(snapshot: &MicrophoneDspSnapshot, selected: DspSelection) -> DspDisplay {
    match selected {
        DspSelection::Equalizer => {
            let profile = match snapshot.equalizer.active_mode {
                DspMode::Simple => &snapshot.equalizer.simple,
                DspMode::Advanced => &snapshot.equalizer.advanced,
            };
            let value = |index: usize| profile.bands.get(index).map_or(0.0, |band| band.gain_db);
            (
                "VOICE EQUALIZER",
                ("Band 1 gain", "Band 2 gain", "Band 3 gain"),
                (value(0), value(1), value(2)),
                ((-12.0, 12.0), (-12.0, 12.0), (-12.0, 12.0)),
            )
        }
        DspSelection::Compressor => {
            let p = match snapshot.compressor.active_mode {
                DspMode::Simple => &snapshot.compressor.simple,
                DspMode::Advanced => &snapshot.compressor.advanced,
            };
            (
                "COMPRESSOR",
                ("Threshold dB", "Ratio", "Makeup gain dB"),
                (p.threshold_db, p.ratio, p.makeup_gain_db),
                ((-60.0, 0.0), (1.0, 20.0), (0.0, 24.0)),
            )
        }
        DspSelection::Expander => {
            let p = match snapshot.expander.active_mode {
                DspMode::Simple => &snapshot.expander.simple,
                DspMode::Advanced => &snapshot.expander.advanced,
            };
            (
                "EXPANDER / GATE",
                ("Threshold dB", "Ratio", "Release ms"),
                (p.threshold_db, p.ratio, p.release_ms),
                ((-90.0, 0.0), (1.0, 20.0), (1.0, 2000.0)),
            )
        }
        DspSelection::NoiseSuppression => (
            "NOISE SUPPRESSION",
            ("Amount %", "Sensitivity dB", "Adapt time ms"),
            (
                snapshot.noise_suppression.amount_percent,
                snapshot.noise_suppression.sensitivity_db,
                snapshot.noise_suppression.adapt_time_ms,
            ),
            ((0.0, 100.0), (-90.0, 0.0), (10.0, 5000.0)),
        ),
        DspSelection::EnhancementSuite => (
            "ENHANCEMENT SUITE",
            ("Bass amount", "De-esser %", "Exciter %"),
            (
                snapshot.enhancement_suite.bass.amount,
                snapshot.enhancement_suite.de_esser.amount_percent,
                snapshot.enhancement_suite.exciter.amount_percent,
            ),
            ((0.0, 100.0), (0.0, 100.0), (0.0, 100.0)),
        ),
        DspSelection::HeadphoneEqualizer => {
            let value = |index: usize| {
                snapshot
                    .headphone_equalizer
                    .bands
                    .get(index)
                    .map_or(0.0, |band| band.amount_db)
            };
            (
                "HEADPHONE EQUALIZER",
                ("Bass dB", "Mids dB", "Treble dB"),
                (value(0), value(1), value(2)),
                ((-12.0, 12.0), (-12.0, 12.0), (-12.0, 12.0)),
            )
        }
    }
}

fn set_dsp_lease(window: Weak<MainWindow>, module: Option<String>, seconds: Option<u64>) {
    let armed = module.is_some();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window.upgrade() {
            window.set_dsp_active_module(module.unwrap_or_default().into());
            window.set_dsp_lease_seconds(seconds.unwrap_or_default() as i32);
            if armed {
                window.set_arm_confirm_pending(false);
            }
        }
    });
}

fn start_health_monitor(window: Weak<MainWindow>, client: DaemonClient) {
    thread::spawn(move || {
        loop {
            match client.health() {
                Ok(health) => {
                    let active = health.dsp_write_modules.last().cloned();
                    let seconds = health.dsp_write_lease_seconds;
                    let update_window = window.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(window) = update_window.upgrade() else {
                            return;
                        };
                        window.set_daemon_connected(health.ok);
                        window.set_hardware_safe(!health.hardware_writes_enabled);
                        window.set_link_active(health.link_control_enabled);
                        window.set_dsp_gate_count(health.dsp_write_modules.len() as i32);
                    });
                    set_dsp_lease(window.clone(), active, seconds);
                }
                Err(_) => set_dsp_lease(window.clone(), None, None),
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn mock_meter_level(tick: u64, index: usize, target: bool) -> f32 {
    let phase = tick as f32 * 0.21 + index as f32 * 0.83;
    let carrier = (phase.sin() + 1.0) * 0.5;
    let detail = ((phase * 2.37).sin() + 1.0) * 0.5;
    let level = if target {
        16.0 + carrier * 37.0 + detail * 9.0
    } else {
        12.0 + carrier * 46.0 + detail * 12.0
    };
    level.clamp(0.0, 100.0)
}

fn show_mock_meter_frame(window: Weak<MainWindow>, tick: u64) -> bool {
    slint::invoke_from_event_loop(move || {
        let Some(window) = window.upgrade() else {
            return;
        };
        let sources = window.get_sources();
        for index in 0..sources.row_count() {
            let Some(mut source) = sources.row_data(index) else {
                continue;
            };
            source.meter = mock_meter_level(tick, index, false);
            sources.set_row_data(index, source);
        }
        let targets = window.get_targets();
        for index in 0..targets.row_count() {
            let Some(mut target) = targets.row_data(index) else {
                continue;
            };
            target.meter = mock_meter_level(tick, index, true);
            targets.set_row_data(index, target);
        }
    })
    .is_ok()
}

fn start_meter_stream(window: Weak<MainWindow>, client: DaemonClient) {
    thread::spawn(move || {
        let mut mock_tick = 0_u64;
        loop {
            match tungstenite::connect(METER_URL) {
                Ok((mut socket, _)) => {
                    let mut levels = HashMap::<String, f32>::new();
                    let mut last_flush = Instant::now();
                    loop {
                        let Ok(message) = socket.read() else {
                            break;
                        };
                        let Ok(text) = message.to_text() else {
                            continue;
                        };
                        let Ok(event) = serde_json::from_str::<MeterEvent>(text) else {
                            continue;
                        };
                        let incoming = event.percent.clamp(0.0, 100.0);
                        let level = levels.entry(event.id).or_default();
                        *level = incoming;
                        if last_flush.elapsed() < Duration::from_millis(16) {
                            continue;
                        }
                        last_flush = Instant::now();
                        let frame = levels.clone();
                        let window = window.clone();
                        if slint::invoke_from_event_loop(move || {
                            let Some(window) = window.upgrade() else {
                                return;
                            };
                            let sources = window.get_sources();
                            for index in 0..sources.row_count() {
                                let Some(mut source) = sources.row_data(index) else {
                                    continue;
                                };
                                if let Some(level) = frame.get(source.meter_id.as_str()) {
                                    source.meter = *level;
                                    sources.set_row_data(index, source);
                                }
                            }
                            let targets = window.get_targets();
                            for index in 0..targets.row_count() {
                                let Some(mut target) = targets.row_data(index) else {
                                    continue;
                                };
                                if let Some(level) = frame.get(target.meter_id.as_str()) {
                                    target.meter = *level;
                                    targets.set_row_data(index, target);
                                }
                            }
                        })
                        .is_err()
                        {
                            return;
                        }
                    }
                }
                Err(_) => {
                    while client
                        .health()
                        .is_ok_and(|health| health.mixer_mode == "mock")
                    {
                        for _ in 0..30 {
                            if !show_mock_meter_frame(window.clone(), mock_tick) {
                                return;
                            }
                            mock_tick = mock_tick.wrapping_add(1);
                            thread::sleep(Duration::from_millis(50));
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(1_500));
        }
    });
}

#[cfg(test)]
mod desktop_tests {
    use super::mock_meter_level;

    #[test]
    fn mock_meter_motion_is_bounded_and_changes_over_time() {
        for target in [false, true] {
            let samples = (0..120)
                .map(|tick| mock_meter_level(tick, 2, target))
                .collect::<Vec<_>>();
            assert!(samples.iter().all(|level| (0.0..=100.0).contains(level)));
            assert!(samples.windows(2).any(|pair| pair[0] != pair[1]));
        }
    }
}
