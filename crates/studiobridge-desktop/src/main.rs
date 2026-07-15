use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use serde::{Deserialize, Serialize};
use slint::{Color, Model, ModelRc, SharedString, VecModel, Weak};
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicU64;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
    AppSnapshot, DspMode, DspWriteModule, EqualizerBandType, EqualizerProfile, HeadphoneOutputMode,
    LinkChannel, MicrophoneDspSnapshot, MicrophoneDspUpdate, MixBus, MixerChannel,
    MixerDeviceChoice, MixerPhysicalDeviceChoice, MixerPhysicalDeviceDescriptor, MixerSourceKind,
    MuteState, NoiseSuppressionStyle,
};

mod client;
use client::{DaemonClient, DaemonClientError, DaemonErrorKind};

slint::include_modules!();

const METER_URL: &str = "ws://127.0.0.1:14565/api/websocket/meter";
const PROJECT_URL: &str = "https://github.com/dilllxd/StudioBridge";
const SUPPORT_URL: &str = "https://github.com/dilllxd/StudioBridge/issues";
static DAEMON_READY: AtomicBool = AtomicBool::new(false);
static REFRESH_COORDINATOR: RefreshCoordinator = RefreshCoordinator::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonStatusPresentation {
    kind: &'static str,
    title: &'static str,
    detail: String,
    status: String,
}

#[derive(Debug, Clone)]
struct RefreshStatus {
    status: String,
    alert: bool,
    failure_context: String,
    requires_profile_readback: bool,
    preferred_default: Option<(bool, String)>,
}

#[derive(Debug)]
struct RefreshOrderingState {
    generation: u64,
    last_applied_generation: u64,
    pending_status: Option<RefreshStatus>,
}

#[derive(Debug)]
struct RefreshCoordinator {
    state: Mutex<RefreshOrderingState>,
}

impl RefreshCoordinator {
    const fn new() -> Self {
        Self {
            state: Mutex::new(RefreshOrderingState {
                generation: 0,
                last_applied_generation: 0,
                pending_status: None,
            }),
        }
    }

    fn begin(&self, status: Option<RefreshStatus>) -> u64 {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.generation = state.generation.wrapping_add(1).max(1);
        if let Some(status) = status {
            state.pending_status = Some(match state.pending_status.take() {
                Some(pending) => merge_pending_refresh_status(pending, status),
                None => status,
            });
        }
        state.generation
    }

    fn monitor_generation(&self) -> Option<u64> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        (state.generation == state.last_applied_generation).then_some(state.generation)
    }

    fn lock_if_current(
        &self,
        generation: u64,
    ) -> Option<std::sync::MutexGuard<'_, RefreshOrderingState>> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.generation != generation {
            return None;
        }
        Some(state)
    }
}

fn merge_pending_refresh_status(pending: RefreshStatus, incoming: RefreshStatus) -> RefreshStatus {
    let requires_profile_readback =
        pending.requires_profile_readback || incoming.requires_profile_readback;
    let preferred_default = incoming
        .preferred_default
        .clone()
        .or_else(|| pending.preferred_default.clone());
    let mut merged = if pending.alert && !incoming.alert {
        pending
    } else {
        incoming
    };
    merged.requires_profile_readback = requires_profile_readback;
    merged.preferred_default = preferred_default;
    merged
}

fn daemon_status_for_kind(kind: DaemonErrorKind, context: &str) -> DaemonStatusPresentation {
    let (kind, title, reason, detail) = match kind {
        DaemonErrorKind::Offline => (
            "offline",
            "STUDIOBRIDGE DAEMON IS OFFLINE",
            "daemon offline",
            "StudioBridge cannot reach the local daemon at 127.0.0.1:17840. Start or restart the daemon, then retry.",
        ),
        DaemonErrorKind::Timeout => (
            "timeout",
            "STUDIOBRIDGE DAEMON DID NOT RESPOND",
            "request timed out",
            "The local daemon did not answer within the bounded timeout. No change is assumed to have completed.",
        ),
        DaemonErrorKind::HttpStatus(status) => {
            return DaemonStatusPresentation {
                kind: "http",
                title: "STUDIOBRIDGE DAEMON REJECTED THE REQUEST",
                detail: format!(
                    "The local daemon returned HTTP {status}. Existing read-only data remains available; retry after checking the daemon status."
                ),
                status: format!("{context}: daemon returned HTTP {status}"),
            };
        }
        DaemonErrorKind::InvalidResponse => (
            "invalid_response",
            "STUDIOBRIDGE DAEMON RESPONSE IS INVALID",
            "invalid daemon response",
            "The daemon replied, but StudioBridge could not safely decode its response. Controls remain unconfirmed until a valid refresh succeeds.",
        ),
        DaemonErrorKind::Transport => (
            "transport",
            "STUDIOBRIDGE DAEMON CONNECTION FAILED",
            "daemon transport failed",
            "The localhost connection failed outside the normal offline or timeout cases. No change is assumed to have completed.",
        ),
    };
    DaemonStatusPresentation {
        kind,
        title,
        detail: detail.into(),
        status: format!("{context}: {reason}"),
    }
}

fn daemon_status_for_error(error: &DaemonClientError, context: &str) -> DaemonStatusPresentation {
    daemon_status_for_kind(error.kind(), context)
}

fn daemon_not_ready_status(context: &str) -> DaemonStatusPresentation {
    DaemonStatusPresentation {
        kind: "not_ready",
        title: "STUDIOBRIDGE DAEMON IS NOT READY",
        detail: "The local daemon answered but reported that its audio runtime is not ready. Read-only navigation remains available.".into(),
        status: format!("{context}: daemon reported not ready"),
    }
}

fn default_readback_matches(snapshot: &AppSnapshot, input: bool, requested_id: &str) -> bool {
    if input {
        default_selection_matches(snapshot.mixer.default_input.as_deref(), requested_id)
    } else {
        default_selection_matches(snapshot.mixer.default_output.as_deref(), requested_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreferredDefaultReadback {
    Retain,
    Persist,
    Reject,
}

fn preferred_default_readback(
    health_ready: bool,
    snapshot: Option<&AppSnapshot>,
    input: bool,
    requested_id: &str,
) -> PreferredDefaultReadback {
    if !health_ready {
        return PreferredDefaultReadback::Retain;
    }
    match snapshot {
        None => PreferredDefaultReadback::Retain,
        Some(snapshot) if default_readback_matches(snapshot, input, requested_id) => {
            PreferredDefaultReadback::Persist
        }
        Some(_) => PreferredDefaultReadback::Reject,
    }
}

fn default_selection_matches(actual: Option<&str>, requested_id: &str) -> bool {
    actual == Some(requested_id)
}

fn mutation_refresh_status(action: &str, result: &Result<(), DaemonClientError>) -> RefreshStatus {
    let failure_context = format!("{action} was not confirmed");
    match result {
        Ok(()) => RefreshStatus {
            status: format!("{action} request accepted; current state reloaded"),
            alert: false,
            failure_context,
            requires_profile_readback: false,
            preferred_default: None,
        },
        Err(error) => {
            let presentation = daemon_status_for_error(error, &failure_context);
            let ambiguity = if error.kind() == DaemonErrorKind::Timeout {
                "; result was ambiguous, so current state was reloaded"
            } else {
                "; current state was reloaded"
            };
            RefreshStatus {
                status: format!("{}{ambiguity}", presentation.status),
                alert: true,
                failure_context,
                requires_profile_readback: false,
                preferred_default: None,
            }
        }
    }
}

fn profile_mutation_refresh_status(
    action: &str,
    result: &Result<(), DaemonClientError>,
) -> RefreshStatus {
    let mut status = mutation_refresh_status(action, result);
    status.requires_profile_readback = true;
    status
}

fn default_mutation_refresh_status(
    action: &str,
    result: &Result<(), DaemonClientError>,
    input: bool,
    requested_id: &str,
) -> RefreshStatus {
    let mut status = mutation_refresh_status(action, result);
    if result.is_ok() {
        status.preferred_default = Some((input, requested_id.into()));
    }
    status
}

fn mutation_readback_is_authoritative(
    status: &RefreshStatus,
    health_ready: bool,
    snapshot_ready: bool,
    profiles_ready: bool,
) -> bool {
    health_ready && snapshot_ready && (!status.requires_profile_readback || profiles_ready)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppPreferences {
    start_at_login: bool,
    #[serde(alias = "close_to_tray")]
    open_to_system_tray: bool,
    save_confirmation: bool,
    beta_opt_in: bool,
    mixing_suite_enabled: bool,
    automatic_default_reset: bool,
    preferred_default_input: Option<String>,
    preferred_default_output: Option<String>,
    meter_crossfade: bool,
    studio_profiles_expanded: bool,
    mixer_profiles_expanded: bool,
    profiles_drawer_open: bool,
    hotkeys: HashMap<String, String>,
    mute_actions: HashMap<String, String>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            start_at_login: false,
            open_to_system_tray: true,
            save_confirmation: true,
            beta_opt_in: false,
            mixing_suite_enabled: true,
            automatic_default_reset: true,
            preferred_default_input: None,
            preferred_default_output: None,
            meter_crossfade: true,
            studio_profiles_expanded: true,
            mixer_profiles_expanded: true,
            profiles_drawer_open: true,
            hotkeys: HashMap::new(),
            mute_actions: HashMap::new(),
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

fn should_start_in_background(requested: bool, preferences: &AppPreferences) -> bool {
    requested && preferences.open_to_system_tray
}

fn external_link_url(destination: &str) -> Option<&'static str> {
    match destination {
        "project" => Some(PROJECT_URL),
        "support" => Some(SUPPORT_URL),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DefaultDeviceRepair {
    input: Option<String>,
    output: Option<String>,
}

fn preferred_default_repair(
    enabled: bool,
    preferred: Option<&str>,
    current: Option<&str>,
    choices: &[MixerDeviceChoice],
) -> Option<String> {
    let preferred = enabled.then_some(preferred).flatten()?;
    if current == Some(preferred) || !choices.iter().any(|choice| choice.id == preferred) {
        return None;
    }
    Some(preferred.into())
}

fn default_device_repair(
    preferences: &AppPreferences,
    snapshot: &AppSnapshot,
) -> DefaultDeviceRepair {
    DefaultDeviceRepair {
        input: preferred_default_repair(
            preferences.automatic_default_reset,
            preferences.preferred_default_input.as_deref(),
            snapshot.mixer.default_input.as_deref(),
            &snapshot.mixer.default_inputs,
        ),
        output: preferred_default_repair(
            preferences.automatic_default_reset,
            preferences.preferred_default_output.as_deref(),
            snapshot.mixer.default_output.as_deref(),
            &snapshot.mixer.default_outputs,
        ),
    }
}

fn remember_preferred_default(input: bool, device_id: &str) -> Result<(), String> {
    let mut preferences = load_preferences();
    if input {
        preferences.preferred_default_input = Some(device_id.into());
    } else {
        preferences.preferred_default_output = Some(device_id.into());
    }
    save_preferences(&preferences)
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

fn hotkey_key_name(text: &str) -> Option<String> {
    use slint::platform::Key;

    let mut chars = text.chars();
    let key = chars.next()?;
    if chars.next().is_some() {
        return None;
    }

    let special_keys = [
        (Key::Escape, "escape"),
        (Key::Backspace, "Backspace"),
        (Key::Tab, "Tab"),
        (Key::Return, "Enter"),
        (Key::Delete, "Delete"),
        (Key::Space, "Space"),
        (Key::UpArrow, "ArrowUp"),
        (Key::DownArrow, "ArrowDown"),
        (Key::LeftArrow, "ArrowLeft"),
        (Key::RightArrow, "ArrowRight"),
        (Key::F1, "F1"),
        (Key::F2, "F2"),
        (Key::F3, "F3"),
        (Key::F4, "F4"),
        (Key::F5, "F5"),
        (Key::F6, "F6"),
        (Key::F7, "F7"),
        (Key::F8, "F8"),
        (Key::F9, "F9"),
        (Key::F10, "F10"),
        (Key::F11, "F11"),
        (Key::F12, "F12"),
        (Key::F13, "F13"),
        (Key::F14, "F14"),
        (Key::F15, "F15"),
        (Key::F16, "F16"),
        (Key::F17, "F17"),
        (Key::F18, "F18"),
        (Key::F19, "F19"),
        (Key::F20, "F20"),
        (Key::F21, "F21"),
        (Key::F22, "F22"),
        (Key::F23, "F23"),
        (Key::F24, "F24"),
        (Key::Insert, "Insert"),
        (Key::Home, "Home"),
        (Key::End, "End"),
        (Key::PageUp, "PageUp"),
        (Key::PageDown, "PageDown"),
        (Key::Pause, "Pause"),
    ];
    if let Some((_, name)) = special_keys
        .into_iter()
        .find(|(candidate, _)| char::from(*candidate) == key)
    {
        return Some(name.into());
    }

    if [
        Key::Shift,
        Key::ShiftR,
        Key::Control,
        Key::ControlR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
    ]
    .into_iter()
    .any(|candidate| char::from(candidate) == key)
    {
        return None;
    }

    let name = match key {
        'a'..='z' | 'A'..='Z' => key.to_ascii_uppercase().to_string(),
        '0'..='9' => key.to_string(),
        '`' | '~' => "Backquote".into(),
        '\\' | '|' => "Backslash".into(),
        '[' | '{' => "BracketLeft".into(),
        ']' | '}' => "BracketRight".into(),
        ',' | '<' => "Comma".into(),
        '.' | '>' => "Period".into(),
        '/' | '?' => "Slash".into(),
        ';' | ':' => "Semicolon".into(),
        '\'' | '"' => "Quote".into(),
        '-' | '_' => "Minus".into(),
        '=' | '+' => "Equal".into(),
        '!' => "1".into(),
        '@' => "2".into(),
        '#' => "3".into(),
        '$' => "4".into(),
        '%' => "5".into(),
        '^' => "6".into(),
        '&' => "7".into(),
        '*' => "8".into(),
        '(' => "9".into(),
        ')' => "0".into(),
        _ => return None,
    };
    Some(name)
}

fn normalize_captured_hotkey(
    text: &str,
    control: bool,
    alt: bool,
    shift: bool,
    meta: bool,
) -> Option<String> {
    let key = hotkey_key_name(text)?;
    let mut parts = Vec::with_capacity(5);
    if control {
        parts.push("ctrl".to_string());
    }
    if shift {
        parts.push("shift".to_string());
    }
    if alt {
        parts.push("alt".to_string());
    }
    if meta {
        parts.push("super".to_string());
    }
    parts.push(key);
    Some(parts.join(" + "))
}

fn normalize_mute_action(action: &str) -> &'static str {
    match action {
        "all" => "all",
        "personal" => "personal",
        _ => "audience",
    }
}

fn source_name_available(name: &str, source_names: &str) -> bool {
    !source_names
        .lines()
        .any(|source| source.eq_ignore_ascii_case(name))
}

fn application_chip_text_width(text: &str, minimum: f32, maximum: f32) -> f32 {
    let measured = text.chars().fold(0.0_f32, |width, character| {
        width
            + if character.is_ascii_uppercase() {
                6.2
            } else if character.is_ascii_whitespace() {
                3.2
            } else if matches!(character, 'i' | 'l' | 'I' | '.' | ',' | ':' | ';' | '!') {
                3.3
            } else {
                5.2
            }
    });
    (measured + 3.0).clamp(minimum, maximum)
}

fn link_channel_label(channel: LinkChannel) -> &'static str {
    match channel {
        LinkChannel::System => "System",
        LinkChannel::Link1 => "Link 1",
        LinkChannel::Link2 => "Link 2",
        LinkChannel::Link3 => "Link 3",
        LinkChannel::Link4 => "Link 4",
    }
}

fn link_channel_for_source(name: &str) -> i32 {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized == "system" {
        return 0;
    }
    for channel in (2..=4).rev() {
        if normalized.contains(&format!("link {channel}"))
            || normalized.contains(&format!("link{channel}"))
        {
            return channel;
        }
    }
    if normalized == "link in" || normalized.contains("link 1") || normalized.contains("link1") {
        return 1;
    }
    -1
}

fn mute_state_with_target(current: MuteState, target: &str, muted: bool) -> MuteState {
    let personal = if target == "personal" || target == "all" {
        muted
    } else {
        matches!(current, MuteState::MutedAll | MuteState::MutedPersonal)
    };
    let audience = if target == "audience" || target == "all" {
        muted
    } else {
        matches!(current, MuteState::MutedAll | MuteState::MutedAudience)
    };
    match (personal, audience) {
        (false, false) => MuteState::Unmuted,
        (true, true) => MuteState::MutedAll,
        (true, false) => MuteState::MutedPersonal,
        (false, true) => MuteState::MutedAudience,
    }
}

fn using_wayland() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn execute_hotkey_action(action: String, client: DaemonClient, window: Weak<MainWindow>) {
    thread::spawn(move || {
        match client.health() {
            Ok(health) if health.ok => {}
            Ok(_) => {
                set_daemon_failure(
                    window,
                    daemon_not_ready_status("Hotkey action was not attempted"),
                );
                return;
            }
            Err(error) => {
                set_daemon_failure(
                    window,
                    daemon_status_for_error(&error, "Hotkey action was not attempted"),
                );
                return;
            }
        }
        let mut terminal_status = None;
        if let Some(profile) = action.strip_prefix("profile:") {
            let result = client.load_mixer_profile(profile);
            terminal_status = Some(profile_mutation_refresh_status("Profile hotkey", &result));
        } else if let Some(channel_id) = action.strip_prefix("mute:") {
            let current = match client.snapshot() {
                Ok(snapshot) => snapshot
                    .mixer
                    .channels
                    .into_iter()
                    .find(|channel| channel.id == channel_id),
                Err(error) => {
                    refresh_after_mutation(window, client, Err(error), "Mute hotkey");
                    return;
                }
            };
            if let Some(current) = current {
                let next = if current.mute_state == MuteState::Unmuted {
                    MuteState::MutedAll
                } else {
                    MuteState::Unmuted
                };
                let result = client.set_mute(channel_id, next);
                terminal_status = Some(mutation_refresh_status("Mute hotkey", &result));
            } else {
                terminal_status = Some(RefreshStatus {
                    status:
                        "Mute hotkey was not attempted: source is absent from the current read-back"
                            .into(),
                    alert: true,
                    failure_context: "Mute hotkey was not confirmed".into(),
                    requires_profile_readback: false,
                    preferred_default: None,
                });
            }
        } else if action == "mix:toggle-personal-device" {
            match client.snapshot() {
                Ok(snapshot) if snapshot.mixer.default_outputs.len() >= 2 => {
                    let current = snapshot
                        .mixer
                        .default_output
                        .as_ref()
                        .and_then(|id| {
                            snapshot
                                .mixer
                                .default_outputs
                                .iter()
                                .position(|device| &device.id == id)
                        })
                        .unwrap_or(0);
                    let next = &snapshot.mixer.default_outputs
                        [(current + 1) % snapshot.mixer.default_outputs.len()];
                    let result = client.set_default_output(&next.id);
                    let status = default_mutation_refresh_status(
                        "Personal Mix device hotkey",
                        &result,
                        false,
                        &next.id,
                    );
                    terminal_status = Some(status);
                }
                Ok(_) => {
                    terminal_status = Some(RefreshStatus {
                        status: "Personal Mix device cycling needs at least two outputs".into(),
                        alert: true,
                        failure_context: "Personal Mix device hotkey was not confirmed".into(),
                        requires_profile_readback: false,
                        preferred_default: None,
                    });
                }
                Err(error) => {
                    refresh_after_mutation(
                        window,
                        client,
                        Err(error),
                        "Personal Mix device hotkey",
                    );
                    return;
                }
            }
        }
        refresh_all_with_status(window, client, terminal_status);
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
    selected_eq_band: usize,
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

const fn should_preserve_drawer_window_size(maximized: bool, fullscreen: bool) -> bool {
    !maximized && !fullscreen
}

fn main() -> Result<(), slint::PlatformError> {
    let preferences = load_preferences();
    let start_in_background = should_start_in_background(
        std::env::args().any(|argument| argument == "--background"),
        &preferences,
    );
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
    window.set_start_at_login(preferences.start_at_login);
    window.set_open_to_system_tray(preferences.open_to_system_tray);
    window.set_save_confirmation(preferences.save_confirmation);
    window.set_beta_opt_in(preferences.beta_opt_in);
    window.set_mixing_suite_enabled(preferences.mixing_suite_enabled);
    window.set_automatic_default_reset(preferences.automatic_default_reset);
    window.set_meter_crossfade(preferences.meter_crossfade);
    window.set_studio_profiles_expanded(preferences.studio_profiles_expanded);
    window.set_mixer_profiles_expanded(preferences.mixer_profiles_expanded);
    window.set_profiles_drawer_open(preferences.profiles_drawer_open);
    if !preferences.profiles_drawer_open {
        // Applying a persisted collapsed layout changes Slint's preferred width
        // before the first show; restore BEACN's measured default client size.
        window
            .window()
            .set_size(slint::LogicalSize::new(1402.0, 778.0));
    }
    let hotkey_runtime = Rc::new(RefCell::new(HotkeyRuntime::new(
        &preferences,
        client.clone(),
        window.as_weak(),
    )));

    window.set_sources(empty_sources());
    window.set_routes(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_linux_applications(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_link_applications(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_link_outputs(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_targets(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_eq_bands(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_mixer_profiles(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_hotkeys(ModelRc::from(Rc::new(VecModel::from(Vec::new()))));
    window.set_mixer_channel_names(string_model(vec!["Unassigned".into()]));
    window.set_default_input_names(string_model(Vec::new()));
    window.set_default_input_ids(string_model(Vec::new()));
    window.set_default_output_names(string_model(Vec::new()));
    window.set_default_output_ids(string_model(Vec::new()));
    window.set_physical_output_names(string_model(vec!["Unassigned".into()]));
    window.set_physical_output_ids(string_model(vec![String::new()]));
    window.set_physical_input_names(string_model(Vec::new()));
    window.set_physical_input_ids(string_model(Vec::new()));
    window.set_link_output_target_names(string_model(vec!["Unassigned".into()]));
    window.set_link_output_target_ids(string_model(vec![String::new()]));
    window.set_link_channel_names(string_model(vec![
        "System".into(),
        "Link 1".into(),
        "Link 2".into(),
        "Link 3".into(),
        "Link 4".into(),
    ]));

    window.on_normalize_hotkey(|text, control, alt, shift, meta| {
        normalize_captured_hotkey(text.as_str(), control, alt, shift, meta)
            .unwrap_or_default()
            .into()
    });
    window.on_hotkey_key_label(|text| hotkey_key_name(text.as_str()).unwrap_or_default().into());
    window.on_source_name_available(|name, source_names| {
        source_name_available(name.as_str(), source_names.as_str())
    });
    window.on_link_channel_for_source(|name| link_channel_for_source(name.as_str()));

    let preferences_window = window.as_weak();
    window.on_save_preferences(
        move |start_at_login,
              open_to_system_tray,
              save_confirmation,
              beta_opt_in,
              mixing_suite_enabled,
              automatic_default_reset,
              meter_crossfade| {
            let existing = load_preferences();
            let preferences = AppPreferences {
                start_at_login,
                open_to_system_tray,
                save_confirmation,
                beta_opt_in,
                mixing_suite_enabled,
                automatic_default_reset,
                preferred_default_input: existing.preferred_default_input,
                preferred_default_output: existing.preferred_default_output,
                meter_crossfade,
                studio_profiles_expanded: existing.studio_profiles_expanded,
                mixer_profiles_expanded: existing.mixer_profiles_expanded,
                profiles_drawer_open: existing.profiles_drawer_open,
                hotkeys: existing.hotkeys,
                mute_actions: existing.mute_actions,
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

    let profile_expansion_window = window.as_weak();
    window.on_save_profile_expansion(move |studio_expanded, mixer_expanded| {
        let mut preferences = load_preferences();
        preferences.studio_profiles_expanded = studio_expanded;
        preferences.mixer_profiles_expanded = mixer_expanded;
        match save_preferences(&preferences) {
            Ok(()) => set_status(
                profile_expansion_window.clone(),
                "Profile view saved".into(),
            ),
            Err(error) => set_status(
                profile_expansion_window.clone(),
                format!("Profile view save failed: {error}"),
            ),
        }
    });

    let profile_drawer_window = window.as_weak();
    window.on_save_profile_drawer(move |open| {
        if let Some(window) = profile_drawer_window.upgrade() {
            let size = window.window().size();
            let preserve_size = should_preserve_drawer_window_size(
                window.window().is_maximized(),
                window.window().is_fullscreen(),
            );
            window.set_profiles_drawer_open(open);
            if preserve_size {
                window.window().set_size(size);
                let restore_window = profile_drawer_window.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = restore_window.upgrade().filter(|window| {
                        should_preserve_drawer_window_size(
                            window.window().is_maximized(),
                            window.window().is_fullscreen(),
                        )
                    }) {
                        window.window().set_size(size);
                    }
                });
            }
        }
        let mut preferences = load_preferences();
        preferences.profiles_drawer_open = open;
        match save_preferences(&preferences) {
            Ok(()) => set_status(
                profile_drawer_window.clone(),
                if open {
                    "Profiles opened".into()
                } else {
                    "Profiles hidden".into()
                },
            ),
            Err(error) => set_status(
                profile_drawer_window.clone(),
                format!("Profile drawer save failed: {error}"),
            ),
        }
    });

    let hotkey_window = window.as_weak();
    let hotkey_client = client.clone();
    let hotkey_runtime_for_save = hotkey_runtime.clone();
    window.on_save_hotkey(move |action, binding| {
        let action = action.trim().to_string();
        let binding = binding.trim().to_string();
        if !binding.is_empty() && binding.parse::<HotKey>().is_err() {
            set_status(
                hotkey_window.clone(),
                format!("Hotkey is not supported: {binding}"),
            );
            return;
        }
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

    let external_link_window = window.as_weak();
    window.on_open_external_link(move |destination| {
        let destination = destination.to_string();
        let Some(url) = external_link_url(&destination) else {
            set_status(
                external_link_window.clone(),
                "Unknown external destination refused".into(),
            );
            return;
        };
        let window = external_link_window.clone();
        thread::spawn(move || match webbrowser::open(url) {
            Ok(()) => set_status(window, format!("Opened StudioBridge {destination}")),
            Err(error) => set_status(window, format!("Could not open {destination}: {error}")),
        });
    });

    let volume_window = window.as_weak();
    let volume_client = client.clone();
    window.on_set_volume(move |channel_id, mix, value| {
        let window = volume_window.clone();
        if !daemon_write_allowed(&window, "Volume update") {
            return;
        }
        let client = volume_client.clone();
        let channel_id = channel_id.to_string();
        let mix = if mix == "personal" {
            MixBus::Personal
        } else {
            MixBus::Audience
        };
        thread::spawn(move || {
            let volume = value.round().clamp(0.0, 100.0) as u8;
            let result = client.set_volume(&channel_id, mix, volume);
            refresh_after_mutation(window, client, result, "Volume update");
        });
    });

    let target_volume_window = window.as_weak();
    let target_volume_client = client.clone();
    window.on_set_target_volume(move |target_id, value| {
        let window = target_volume_window.clone();
        if !daemon_write_allowed(&window, "Output volume update") {
            return;
        }
        let client = target_volume_client.clone();
        let target_id = target_id.to_string();
        thread::spawn(move || {
            let volume = value.round().clamp(0.0, 100.0) as u8;
            let result = client.set_target_volume(&target_id, volume);
            refresh_after_mutation(window, client, result, "Output volume update");
        });
    });

    let target_mute_window = window.as_weak();
    let target_mute_client = client.clone();
    window.on_set_target_mute(move |target_id, muted| {
        let window = target_mute_window.clone();
        if !daemon_write_allowed(&window, "Output mute update") {
            return;
        }
        let client = target_mute_client.clone();
        let target_id = target_id.to_string();
        thread::spawn(move || {
            let result = client.set_target_mute(&target_id, muted);
            refresh_after_mutation(window, client, result, "Output mute update");
        });
    });

    let target_device_window = window.as_weak();
    let target_device_client = client.clone();
    window.on_set_target_device(move |target_id, device_node_id| {
        let window = target_device_window.clone();
        if !daemon_write_allowed(&window, "Output device update") {
            return;
        }
        let client = target_device_client.clone();
        let target_id = target_id.to_string();
        let device_node_id = device_node_id.to_string();
        thread::spawn(move || {
            let node_id = if device_node_id.is_empty() {
                None
            } else {
                match device_node_id.parse::<u32>() {
                    Ok(node_id) => Some(node_id),
                    Err(error) => {
                        set_status(
                            window,
                            format!("Output device selection is invalid: {error}"),
                        );
                        return;
                    }
                }
            };
            let result = client.set_target_device(&target_id, node_id);
            refresh_after_mutation(window, client, result, "Output device update");
        });
    });

    let source_device_window = window.as_weak();
    let source_device_client = client.clone();
    window.on_set_source_device(move |channel_id, device_node_id| {
        let window = source_device_window.clone();
        if !daemon_write_allowed(&window, "Input device update") {
            return;
        }
        let client = source_device_client.clone();
        let channel_id = channel_id.to_string();
        let device_node_id = device_node_id.to_string();
        thread::spawn(move || {
            let node_id = if device_node_id.is_empty() {
                None
            } else {
                match device_node_id.parse::<u32>() {
                    Ok(node_id) => Some(node_id),
                    Err(error) => {
                        set_status(
                            window,
                            format!("Input device selection is invalid: {error}"),
                        );
                        return;
                    }
                }
            };
            let result = client.set_source_device(&channel_id, node_id);
            refresh_after_mutation(window, client, result, "Input device update");
        });
    });

    let link_output_window = window.as_weak();
    let link_output_client = client.clone();
    window.on_set_link_output_assignment(move |output_node_id, target_id| {
        let window = link_output_window.clone();
        if !daemon_write_allowed(&window, "Outgoing Studio Link update") {
            return;
        }
        let client = link_output_client.clone();
        let output_node_id = output_node_id.to_string();
        let target_id = target_id.to_string();
        thread::spawn(move || {
            let node_id = match output_node_id.parse::<u32>() {
                Ok(node_id) => node_id,
                Err(error) => {
                    set_status(window, format!("Link output selection is invalid: {error}"));
                    return;
                }
            };
            let target_id = (!target_id.is_empty()).then_some(target_id);
            let result = client.set_link_output_assignment(node_id, target_id.as_deref());
            refresh_after_mutation(window, client, result, "Outgoing Studio Link update");
        });
    });

    let default_input_window = window.as_weak();
    let default_input_client = client.clone();
    window.on_set_default_input(move |device_id| {
        let window = default_input_window.clone();
        if !daemon_write_allowed(&window, "Default recording device update") {
            return;
        }
        let client = default_input_client.clone();
        let device_id = device_id.to_string();
        thread::spawn(move || {
            let result = client.set_default_input(&device_id);
            let status = default_mutation_refresh_status(
                "Default recording device update",
                &result,
                true,
                &device_id,
            );
            refresh_all_with_status(window, client, Some(status));
        });
    });

    let default_output_window = window.as_weak();
    let default_output_client = client.clone();
    window.on_set_default_output(move |device_id| {
        let window = default_output_window.clone();
        if !daemon_write_allowed(&window, "Default playback device update") {
            return;
        }
        let client = default_output_client.clone();
        let device_id = device_id.to_string();
        thread::spawn(move || {
            let result = client.set_default_output(&device_id);
            let status = default_mutation_refresh_status(
                "Default playback device update",
                &result,
                false,
                &device_id,
            );
            refresh_all_with_status(window, client, Some(status));
        });
    });

    let mute_window = window.as_weak();
    let mute_client = client.clone();
    window.on_set_source_mute(move |channel_id, mix, muted| {
        let window = mute_window.clone();
        if !daemon_write_allowed(&window, "Mute update") { return; }
        let client = mute_client.clone();
        let channel_id = channel_id.to_string();
        let mix = mix.to_string();
        thread::spawn(move || {
            let current = match client.snapshot() {
                Ok(snapshot) => snapshot
                    .mixer
                    .channels
                    .into_iter()
                    .find(|channel| channel.id == channel_id),
                Err(error) => {
                    refresh_after_mutation(window, client, Err(error), "Mute update");
                    return;
                }
            };
            let Some(current) = current else {
                refresh_all_with_status(
                    window,
                    client,
                    Some(RefreshStatus {
                        status: "Mute update was not attempted: source is absent from the current read-back".into(),
                        alert: true,
                        failure_context: "Mute update was not confirmed".into(),
                        requires_profile_readback: false,
                        preferred_default: None,
                    }),
                );
                return;
            };
            let state = mute_state_with_target(current.mute_state, &mix, muted);
            let result = client.set_mute(&channel_id, state);
            refresh_after_mutation(window, client, result, "Mute update");
        });
    });

    let mute_action_window = window.as_weak();
    let mute_action_client = client.clone();
    window.on_set_source_mute_action(move |channel_id, action| {
        let channel_id = channel_id.trim().to_string();
        let action = normalize_mute_action(action.trim()).to_string();
        let mut preferences = load_preferences();
        preferences.mute_actions.insert(channel_id, action.clone());
        match save_preferences(&preferences) {
            Ok(()) => set_status(
                mute_action_window.clone(),
                format!("Source mute action set to {action}"),
            ),
            Err(error) => set_status(
                mute_action_window.clone(),
                format!("Mute action save failed: {error}"),
            ),
        }
        refresh_all(mute_action_window.clone(), mute_action_client.clone());
    });

    let link_volume_window = window.as_weak();
    let link_volume_client = client.clone();
    window.on_set_volume_linked(move |channel_id, linked| {
        let window = link_volume_window.clone();
        if !daemon_write_allowed(&window, "Level link update") {
            return;
        }
        let client = link_volume_client.clone();
        let channel_id = channel_id.to_string();
        thread::spawn(move || {
            let result = client.set_volume_linked(&channel_id, linked);
            refresh_after_mutation(window, client, result, "Level link update");
        });
    });

    let route_window = window.as_weak();
    let route_client = client.clone();
    window.on_set_route(move |source_id, target_id, enabled| {
        let window = route_window.clone();
        if !daemon_write_allowed(&window, "Software route update") {
            return;
        }
        let client = route_client.clone();
        let source_id = source_id.to_string();
        let target_id = target_id.to_string();
        thread::spawn(move || {
            let result = client.set_route(&source_id, &target_id, enabled);
            refresh_after_mutation(window, client, result, "Software route update");
        });
    });

    let create_source_window = window.as_weak();
    let create_source_client = client.clone();
    window.on_create_source(move |name| {
        let window = create_source_window.clone();
        if !daemon_write_allowed(&window, "Add mixer knob") {
            return;
        }
        let client = create_source_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            let result = client.create_source(&name);
            refresh_after_mutation(window, client, result, "Add mixer knob");
        });
    });

    let remove_source_window = window.as_weak();
    let remove_source_client = client.clone();
    window.on_remove_source(move |source_id| {
        let window = remove_source_window.clone();
        if !daemon_write_allowed(&window, "Remove mixer knob") {
            return;
        }
        let client = remove_source_client.clone();
        let source_id = source_id.to_string();
        thread::spawn(move || {
            let result = client.remove_source(&source_id);
            refresh_after_mutation(window, client, result, "Remove mixer knob");
        });
    });

    let reorder_source_window = window.as_weak();
    let reorder_source_client = client.clone();
    window.on_reorder_source(move |source_id, requested_position| {
        let window = reorder_source_window.clone();
        if !daemon_write_allowed(&window, "Mixer knob reorder") {
            return;
        }
        let client = reorder_source_client.clone();
        let source_id = source_id.to_string();
        thread::spawn(move || {
            let snapshot = match client.snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    refresh_after_mutation(window, client, Err(error), "Mixer knob reorder");
                    return;
                }
            };
            let count = snapshot.mixer.channels.len();
            if count == 0 || !requested_position.is_finite() {
                return;
            }
            let position =
                (requested_position.round() as isize).clamp(0, count as isize - 1) as usize;
            let current = snapshot
                .mixer
                .channels
                .iter()
                .position(|channel| channel.id == source_id);
            if current == Some(position) {
                return;
            }
            let result = client.reorder_source(&source_id, position);
            refresh_after_mutation(window, client, result, "Mixer knob reorder");
        });
    });

    let create_profile_window = window.as_weak();
    let create_profile_client = client.clone();
    window.on_create_mixer_profile(move || {
        let window = create_profile_window.clone();
        if !daemon_write_allowed(&window, "Mixer profile creation") {
            return;
        }
        let client = create_profile_client.clone();
        thread::spawn(move || {
            let result = client.create_mixer_profile();
            refresh_after_profile_mutation(window, client, result, "Mixer profile creation");
        });
    });

    let save_profile_window = window.as_weak();
    let save_profile_client = client.clone();
    window.on_save_mixer_profile(move |name| {
        let window = save_profile_window.clone();
        if !daemon_write_allowed(&window, "Mixer profile save") {
            return;
        }
        let client = save_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            let result = client.save_mixer_profile(&name);
            refresh_after_profile_mutation(window, client, result, "Mixer profile save");
        });
    });

    let duplicate_profile_window = window.as_weak();
    let duplicate_profile_client = client.clone();
    window.on_duplicate_mixer_profile(move |name| {
        let window = duplicate_profile_window.clone();
        if !daemon_write_allowed(&window, "Mixer profile duplication") {
            return;
        }
        let client = duplicate_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            let result = client.duplicate_mixer_profile(&name);
            refresh_after_profile_mutation(window, client, result, "Mixer profile duplication");
        });
    });

    let load_profile_window = window.as_weak();
    let load_profile_client = client.clone();
    window.on_load_mixer_profile(move |name| {
        let window = load_profile_window.clone();
        if !daemon_write_allowed(&window, "Mixer profile load") {
            return;
        }
        let client = load_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            let result = client.load_mixer_profile(&name);
            refresh_after_profile_mutation(window, client, result, "Mixer profile load");
        });
    });

    let delete_profile_window = window.as_weak();
    let delete_profile_client = client.clone();
    window.on_delete_mixer_profile(move |name| {
        let window = delete_profile_window.clone();
        if !daemon_write_allowed(&window, "Mixer profile deletion") {
            return;
        }
        let client = delete_profile_client.clone();
        let name = name.to_string();
        thread::spawn(move || {
            let result = client.delete_mixer_profile(&name);
            refresh_after_profile_mutation(window, client, result, "Mixer profile deletion");
        });
    });

    let linux_app_window = window.as_weak();
    let linux_app_client = client.clone();
    window.on_assign_linux_application(move |process, name, channel_index| {
        let window = linux_app_window.clone();
        if !daemon_write_allowed(&window, "Linux application assignment") {
            return;
        }
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
            let result = client.set_mixer_application(&process, &name, channel_id);
            refresh_after_mutation(window, client, result, "Linux application assignment");
        });
    });

    let link_app_window = window.as_weak();
    let link_app_client = client.clone();
    window.on_assign_link_application(move |application, channel_index| {
        let window = link_app_window.clone();
        if !daemon_write_allowed(&window, "Windows Link assignment") {
            return;
        }
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
            let result = client.set_link_application(&application, channel);
            refresh_after_mutation(window, client, result, "Windows Link assignment");
        });
    });

    let select_window = window.as_weak();
    let select_session = dsp_session.clone();
    window.on_select_dsp_module(move |module| {
        if let Ok(mut session) = select_session.lock() {
            session.selected = DspSelection::parse(module.as_str());
            if let Some(snapshot) = session.snapshot.clone() {
                show_dsp_snapshot(
                    &select_window,
                    &snapshot,
                    session.selected,
                    session.selected_eq_band,
                );
            }
        }
    });

    let mode_window = window.as_weak();
    let mode_session = dsp_session.clone();
    window.on_select_dsp_mode(move |advanced| {
        let staged = mode_session.lock().ok().and_then(|mut session| {
            let selected = session.selected;
            let snapshot = session.snapshot.as_mut()?;
            let mode = if advanced {
                DspMode::Advanced
            } else {
                DspMode::Simple
            };
            match selected {
                DspSelection::Equalizer => snapshot.equalizer.active_mode = mode,
                DspSelection::Compressor => snapshot.compressor.active_mode = mode,
                DspSelection::Expander => snapshot.expander.active_mode = mode,
                _ => return None,
            }
            Some((snapshot.clone(), selected, session.selected_eq_band))
        });
        if let Some((snapshot, selected, eq_band)) = staged {
            show_dsp_snapshot(&mode_window, &snapshot, selected, eq_band);
        }
    });

    let eq_select_window = window.as_weak();
    let eq_select_session = dsp_session.clone();
    window.on_select_eq_band(move |index| {
        let staged = eq_select_session.lock().ok().and_then(|mut session| {
            let snapshot = session.snapshot.clone()?;
            let band_count = active_eq_profile(&snapshot).bands.len();
            if band_count == 0 {
                return None;
            }
            session.selected = DspSelection::Equalizer;
            session.selected_eq_band = (index.max(0) as usize).min(band_count - 1);
            Some((snapshot, session.selected_eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &eq_select_window,
                &snapshot,
                DspSelection::Equalizer,
                eq_band,
            );
        }
    });

    let eq_type_window = window.as_weak();
    let eq_type_session = dsp_session.clone();
    window.on_set_eq_band_type(move |band_type| {
        let staged = eq_type_session.lock().ok().and_then(|mut session| {
            let eq_band = session.selected_eq_band;
            session.selected = DspSelection::Equalizer;
            let snapshot = session.snapshot.as_mut()?;
            set_eq_band_type_value(snapshot, eq_band, band_type.as_str())?;
            Some((snapshot.clone(), eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(&eq_type_window, &snapshot, DspSelection::Equalizer, eq_band);
        }
    });

    let eq_adjust_window = window.as_weak();
    let eq_adjust_session = dsp_session.clone();
    window.on_adjust_eq_band(move |field, direction| {
        let staged = eq_adjust_session.lock().ok().and_then(|mut session| {
            let eq_band = session.selected_eq_band;
            session.selected = DspSelection::Equalizer;
            let snapshot = session.snapshot.as_mut()?;
            adjust_eq_band_value(snapshot, eq_band, field.as_str(), direction)?;
            Some((snapshot.clone(), eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &eq_adjust_window,
                &snapshot,
                DspSelection::Equalizer,
                eq_band,
            );
        }
    });

    let eq_value_window = window.as_weak();
    let eq_value_session = dsp_session.clone();
    window.on_set_eq_band_value(move |field, value| {
        let staged = eq_value_session.lock().ok().and_then(|mut session| {
            let eq_band = session.selected_eq_band;
            session.selected = DspSelection::Equalizer;
            let snapshot = session.snapshot.as_mut()?;
            set_eq_band_value(snapshot, eq_band, field.as_str(), value)?;
            Some((snapshot.clone(), eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &eq_value_window,
                &snapshot,
                DspSelection::Equalizer,
                eq_band,
            );
        }
    });

    let eq_add_window = window.as_weak();
    let eq_add_session = dsp_session.clone();
    window.on_add_eq_band(move || {
        let staged = eq_add_session.lock().ok().and_then(|mut session| {
            session.selected = DspSelection::Equalizer;
            let (snapshot, eq_band) = {
                let snapshot = session.snapshot.as_mut()?;
                let eq_band = add_eq_band(snapshot)?;
                (snapshot.clone(), eq_band)
            };
            session.selected_eq_band = eq_band;
            Some((snapshot, eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(&eq_add_window, &snapshot, DspSelection::Equalizer, eq_band);
        }
    });

    let eq_remove_window = window.as_weak();
    let eq_remove_session = dsp_session.clone();
    window.on_remove_eq_band(move || {
        let staged = eq_remove_session.lock().ok().and_then(|mut session| {
            let eq_band = session.selected_eq_band;
            session.selected = DspSelection::Equalizer;
            let snapshot = session.snapshot.as_mut()?;
            remove_eq_band(snapshot, eq_band)?;
            Some((snapshot.clone(), eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &eq_remove_window,
                &snapshot,
                DspSelection::Equalizer,
                eq_band,
            );
        }
    });

    let enhancement_preset_window = window.as_weak();
    let enhancement_preset_session = dsp_session.clone();
    window.on_set_enhancement_preset(move |preset| {
        let staged = enhancement_preset_session
            .lock()
            .ok()
            .and_then(|mut session| {
                session.selected = DspSelection::EnhancementSuite;
                let eq_band = session.selected_eq_band;
                let snapshot = session.snapshot.as_mut()?;
                set_enhancement_preset(snapshot, preset)?;
                Some((snapshot.clone(), eq_band))
            });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &enhancement_preset_window,
                &snapshot,
                DspSelection::EnhancementSuite,
                eq_band,
            );
        }
    });

    let enhancement_value_window = window.as_weak();
    let enhancement_value_session = dsp_session.clone();
    window.on_set_enhancement_value(move |field, value| {
        let staged = enhancement_value_session
            .lock()
            .ok()
            .and_then(|mut session| {
                session.selected = DspSelection::EnhancementSuite;
                let eq_band = session.selected_eq_band;
                let snapshot = session.snapshot.as_mut()?;
                set_enhancement_value(snapshot, field.as_str(), value)?;
                Some((snapshot.clone(), eq_band))
            });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &enhancement_value_window,
                &snapshot,
                DspSelection::EnhancementSuite,
                eq_band,
            );
        }
    });

    let headphone_band_window = window.as_weak();
    let headphone_band_session = dsp_session.clone();
    window.on_set_headphone_eq_band(move |index, value| {
        let staged = headphone_band_session.lock().ok().and_then(|mut session| {
            session.selected = DspSelection::HeadphoneEqualizer;
            let eq_band = session.selected_eq_band;
            let snapshot = session.snapshot.as_mut()?;
            set_headphone_eq_band_value(snapshot, index, value)?;
            Some((snapshot.clone(), eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &headphone_band_window,
                &snapshot,
                DspSelection::HeadphoneEqualizer,
                eq_band,
            );
        }
    });

    let subwoofer_window = window.as_weak();
    let subwoofer_session = dsp_session.clone();
    window.on_set_headphone_subwoofer(move |value| {
        let staged = subwoofer_session.lock().ok().and_then(|mut session| {
            session.selected = DspSelection::HeadphoneEqualizer;
            let eq_band = session.selected_eq_band;
            let snapshot = session.snapshot.as_mut()?;
            set_headphone_subwoofer_value(snapshot, value)?;
            Some((snapshot.clone(), eq_band))
        });
        if let Some((snapshot, eq_band)) = staged {
            show_dsp_snapshot(
                &subwoofer_window,
                &snapshot,
                DspSelection::HeadphoneEqualizer,
                eq_band,
            );
        }
    });

    let enabled_window = window.as_weak();
    let enabled_session = dsp_session.clone();
    window.on_set_dsp_enabled(move |enabled| {
        let staged = enabled_session.lock().ok().and_then(|mut session| {
            let selected = session.selected;
            let snapshot = session.snapshot.as_mut()?;
            set_selected_dsp_enabled(snapshot, selected, enabled);
            Some((snapshot.clone(), selected, session.selected_eq_band))
        });
        if let Some((snapshot, selected, eq_band)) = staged {
            show_dsp_snapshot(&enabled_window, &snapshot, selected, eq_band);
        }
    });

    let noise_style_window = window.as_weak();
    let noise_style_session = dsp_session.clone();
    window.on_select_noise_style(move |snapshot_style| {
        let staged = noise_style_session.lock().ok().and_then(|mut session| {
            let selected = session.selected;
            let snapshot = session.snapshot.as_mut()?;
            if !matches!(selected, DspSelection::NoiseSuppression) {
                return None;
            }
            snapshot.noise_suppression.style = if snapshot_style {
                NoiseSuppressionStyle::Snapshot
            } else {
                NoiseSuppressionStyle::Adaptive
            };
            Some((snapshot.clone(), selected, session.selected_eq_band))
        });
        if let Some((snapshot, selected, eq_band)) = staged {
            show_dsp_snapshot(&noise_style_window, &snapshot, selected, eq_band);
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
        if !daemon_write_allowed(&window, "DSP disarm") {
            return;
        }
        let client = disarm_client.clone();
        thread::spawn(move || {
            let result = client.disarm_dsp();
            if result.is_ok() {
                set_dsp_lease(window.clone(), None, None);
            } else {
                refresh_after_mutation(window, client, result, "DSP disarm");
                return;
            }
            refresh_after_mutation(window, client, result, "DSP disarm");
        });
    });

    let apply_window = window.as_weak();
    let apply_client = client.clone();
    let apply_session = dsp_session.clone();
    window.on_apply_dsp(move |a, b, c, d, e| {
        apply_dsp(
            apply_window.clone(),
            apply_client.clone(),
            apply_session.clone(),
            (a, b, c, d, e),
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
            (0.0, 0.0, 0.0, 0.0, 0.0),
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
            window.set_dsp_selected_module("mic_setup".into());
            let _ = window.show();
        }
    });

    let quit_client = client.clone();
    tray.on_quit_app(move || {
        let _ = quit_client.disarm_dsp();
        let _ = slint::quit_event_loop();
    });

    refresh_all(window.as_weak(), client.clone());
    load_dsp(
        window.as_weak(),
        client.clone(),
        dsp_session.clone(),
        false,
        false,
    );
    start_health_monitor(window.as_weak(), client.clone(), dsp_session);
    start_default_device_monitor(window.as_weak(), client.clone());
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

fn source(
    channel: &MixerChannel,
    preferences: &AppPreferences,
    physical_inputs: &[MixerPhysicalDeviceChoice],
) -> SourceModel {
    let mute_action = preferences
        .mute_actions
        .get(&channel.id)
        .map_or("audience", |action| normalize_mute_action(action));
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
        mute_action: mute_action.into(),
        colour: parse_colour(&channel.colour),
        physical: channel.source_kind == MixerSourceKind::Physical,
        input_device_index: channel
            .attached_devices
            .iter()
            .find_map(|attached| {
                physical_inputs
                    .iter()
                    .position(|input| physical_device_matches(attached, &input.descriptor))
            })
            .map_or(-1, |index| index as i32),
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
    refresh_all_with_status(window, client, None);
}

fn refresh_after_mutation(
    window: Weak<MainWindow>,
    client: DaemonClient,
    result: Result<(), DaemonClientError>,
    action: &str,
) {
    let status = mutation_refresh_status(action, &result);
    refresh_all_with_status(window, client, Some(status));
}

fn refresh_after_profile_mutation(
    window: Weak<MainWindow>,
    client: DaemonClient,
    result: Result<(), DaemonClientError>,
    action: &str,
) {
    let status = profile_mutation_refresh_status(action, &result);
    refresh_all_with_status(window, client, Some(status));
}

fn apply_daemon_failure(window: &MainWindow, presentation: &DaemonStatusPresentation) {
    DAEMON_READY.store(false, Ordering::SeqCst);
    window.set_daemon_connected(false);
    window.set_hardware_safe(true);
    window.set_link_active(false);
    window.set_dsp_gate_count(0);
    window.set_arm_confirm_pending(false);
    window.set_daemon_status_kind(presentation.kind.into());
    window.set_daemon_status_title(presentation.title.into());
    window.set_daemon_status_detail(presentation.detail.clone().into());
    window.set_status_alert(true);
    window.set_status_text(presentation.status.clone().into());
}

fn refresh_all_with_status(
    window: Weak<MainWindow>,
    client: DaemonClient,
    terminal_status: Option<RefreshStatus>,
) {
    let generation = REFRESH_COORDINATOR.begin(terminal_status);
    thread::spawn(move || {
        let health = client.health();
        let snapshot = client.snapshot();
        let profiles = client.mixer_profiles();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            // Slint runs this closure synchronously. Keep the ordering guard until every
            // property/model write is complete; none of the guarded code may dispatch a refresh.
            // This also serializes the small local preference write used after matching default
            // read-back, preventing a newer generation from being validated against older state.
            let Some(mut refresh_ordering) = REFRESH_COORDINATOR.lock_if_current(generation) else {
                return;
            };
            let mut terminal_status = refresh_ordering.pending_status.clone();
            let health_ready = health.as_ref().is_ok_and(|health| health.ok);
            let snapshot_ready = snapshot.is_ok();
            let profiles_ready = profiles.is_ok();
            let clear_pending_status = terminal_status.as_ref().is_some_and(|status| {
                mutation_readback_is_authoritative(
                    status,
                    health_ready,
                    snapshot_ready,
                    profiles_ready,
                )
            });
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
            let failure_context = terminal_status.as_ref().map_or_else(
                || "Refresh failed".to_string(),
                |status| status.failure_context.clone(),
            );
            if let Some(status) = terminal_status.as_mut()
                && let Some((input, requested_id)) = status.preferred_default.take()
            {
                match preferred_default_readback(
                    health_ready,
                    snapshot.as_ref().ok(),
                    input,
                    &requested_id,
                ) {
                    PreferredDefaultReadback::Persist => {
                        if let Err(error) = remember_preferred_default(input, &requested_id) {
                            status.status = format!(
                                "{}; current state reloaded, but automatic reset preference save failed: {error}",
                                status.failure_context
                            );
                            status.alert = true;
                        }
                    }
                    PreferredDefaultReadback::Reject => {
                        status.status = format!(
                            "{}: requested default was not confirmed by read-back; automatic reset preference unchanged",
                            status.failure_context
                        );
                        status.alert = true;
                    }
                    PreferredDefaultReadback::Retain => {}
                }
            }
            let mut ready = false;
            match health {
                Ok(health) if health.ok => {
                    DAEMON_READY.store(true, Ordering::SeqCst);
                    let safe = !health.hardware_writes_enabled;
                    ready = true;
                    window.set_daemon_connected(true);
                    window.set_hardware_safe(safe);
                    window.set_link_active(health.link_control_enabled);
                    window.set_dsp_gate_count(health.dsp_write_modules.len() as i32);
                    window.set_daemon_status_kind("ready".into());
                    window.set_daemon_status_title("STUDIOBRIDGE DAEMON IS READY".into());
                    window.set_daemon_status_detail("Local audio state is current.".into());
                    window.set_status_alert(false);
                    window.set_status_text(
                        format!("{} + {}", health.studio_mode, health.mixer_mode).into(),
                    );
                }
                Ok(_) => apply_daemon_failure(&window, &daemon_not_ready_status(&failure_context)),
                Err(error) => {
                    apply_daemon_failure(
                        &window,
                        &daemon_status_for_error(&error, &failure_context),
                    );
                }
            }
            match snapshot {
                Ok(snapshot) if ready => apply_snapshot(&window, snapshot, &profile_names),
                Ok(_) => {}
                Err(error) if ready => {
                    ready = false;
                    apply_daemon_failure(
                        &window,
                        &daemon_status_for_error(&error, &failure_context),
                    );
                }
                Err(_) => {}
            }
            let mut profile_readback_error = None;
            match profiles {
                Ok(profiles) if ready => {
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
                Ok(_) => {}
                Err(error) if ready => {
                    profile_readback_error = Some(daemon_status_for_error(
                        &error,
                        "Mixer profile read-back failed",
                    ));
                }
                Err(_) => {}
            }
            if ready && let Some(status) = terminal_status {
                if status.requires_profile_readback
                    && let Some(presentation) = profile_readback_error
                {
                    window.set_status_alert(true);
                    window.set_status_text(
                        format!("{}; no profile change is confirmed", presentation.status).into(),
                    );
                } else {
                    window.set_status_alert(status.alert);
                    window.set_status_text(status.status.into());
                }
            } else if ready && let Some(presentation) = profile_readback_error {
                window.set_status_alert(true);
                window.set_status_text(presentation.status.into());
            }
            if clear_pending_status {
                refresh_ordering.pending_status = None;
            }
            refresh_ordering.last_applied_generation = generation;
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
    window.set_driverless_mode(snapshot.studio.identity.driverless_mode);
    window.set_microphone_gain_db(snapshot.studio.microphone.gain_db as f32);
    window.set_phantom_power(snapshot.studio.microphone.phantom_power);
    window.set_headphone_volume_percent(snapshot.studio.headphones.volume as f32);
    window.set_headphone_monitor_percent(snapshot.studio.headphones.mic_monitor as f32);
    window.set_headphone_channels_linked(snapshot.studio.headphones.channels_linked);
    window.set_headphone_output_mode(
        match snapshot.studio.headphones.output_mode {
            HeadphoneOutputMode::InEarMonitors => "in_ear_monitors",
            HeadphoneOutputMode::LineLevel => "line_level",
            HeadphoneOutputMode::NormalPower => "normal_power",
            HeadphoneOutputMode::HighImpedance => "high_impedance",
        }
        .into(),
    );
    window
        .set_mic_output_gain_db(snapshot.studio.headphones.mic_output_gain_tenths_db as f32 / 10.0);
    window.set_source_name_key(
        snapshot
            .mixer
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect::<Vec<_>>()
            .join("\n")
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
        .map(|application| {
            let channel_index = application
                .channel_id
                .as_ref()
                .and_then(|id| {
                    snapshot
                        .mixer
                        .channels
                        .iter()
                        .position(|channel| &channel.id == id)
                })
                .map_or(0, |index| index as i32 + 1);
            let destination = if channel_index <= 0 {
                "Unassigned"
            } else {
                snapshot
                    .mixer
                    .channels
                    .get((channel_index - 1) as usize)
                    .map_or("Unassigned", |channel| channel.name.as_str())
            };
            ApplicationModel {
                process: application.process.clone().into(),
                name: application.name.clone().into(),
                channel_index,
                destination: destination.into(),
                name_width: application_chip_text_width(&application.name, 28.0, 126.0),
                destination_width: application_chip_text_width(destination, 38.0, 86.0) + 6.0,
            }
        })
        .collect::<Vec<_>>();
    let link_applications = snapshot
        .studio
        .linked_applications
        .iter()
        .map(|application| {
            let channel_index = match application.channel {
                LinkChannel::System => 0,
                LinkChannel::Link1 => 1,
                LinkChannel::Link2 => 2,
                LinkChannel::Link3 => 3,
                LinkChannel::Link4 => 4,
            };
            let destination = link_channel_label(application.channel);
            LinkApplicationModel {
                name: application.name.clone().into(),
                channel_index,
                destination: destination.into(),
                name_width: application_chip_text_width(&application.name, 28.0, 126.0),
                destination_width: application_chip_text_width(destination, 38.0, 86.0) + 6.0,
            }
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
        .map(|channel| source(channel, &preferences, &snapshot.mixer.physical_inputs))
        .collect::<Vec<_>>();
    let targets =
        ordered_targets
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
                output_device_index: target
                    .attached_devices
                    .iter()
                    .find_map(|attached| {
                        snapshot.mixer.physical_outputs.iter().position(|output| {
                            physical_device_matches(attached, &output.descriptor)
                        })
                    })
                    .map_or(0, |index| index as i32 + 1),
            })
            .collect::<Vec<_>>();
    let mut link_output_target_names = vec!["Unassigned".to_string()];
    link_output_target_names.extend(ordered_targets.iter().map(|target| target.name.clone()));
    let mut link_output_target_ids = vec![String::new()];
    link_output_target_ids.extend(ordered_targets.iter().map(|target| target.id.clone()));
    let link_outputs = (1..=4)
        .map(|slot| {
            let assignment = snapshot
                .mixer
                .link_outputs
                .iter()
                .find(|assignment| assignment.slot == slot);
            LinkOutputModel {
                slot: i32::from(slot),
                node_id: assignment
                    .map(|assignment| assignment.node_id.to_string())
                    .unwrap_or_default()
                    .into(),
                label: if slot == 1 {
                    "Link Out".into()
                } else {
                    format!("Link {slot} Out").into()
                },
                target_index: assignment
                    .and_then(|assignment| assignment.target_id.as_ref())
                    .and_then(|target_id| {
                        ordered_targets
                            .iter()
                            .position(|target| &target.id == target_id)
                    })
                    .map_or(0, |index| index as i32 + 1),
                available: assignment.is_some(),
            }
        })
        .collect::<Vec<_>>();
    let default_input_index = snapshot
        .mixer
        .default_input
        .as_ref()
        .and_then(|id| {
            snapshot
                .mixer
                .default_inputs
                .iter()
                .position(|device| &device.id == id)
        })
        .map_or(-1, |index| index as i32);
    let default_output_index = snapshot
        .mixer
        .default_output
        .as_ref()
        .and_then(|id| {
            snapshot
                .mixer
                .default_outputs
                .iter()
                .position(|device| &device.id == id)
        })
        .map_or(-1, |index| index as i32);
    window.set_default_input_names(string_model(
        snapshot
            .mixer
            .default_inputs
            .iter()
            .map(|device| device.name.clone())
            .collect(),
    ));
    window.set_default_input_ids(string_model(
        snapshot
            .mixer
            .default_inputs
            .iter()
            .map(|device| device.id.clone())
            .collect(),
    ));
    window.set_default_output_names(string_model(
        snapshot
            .mixer
            .default_outputs
            .iter()
            .map(|device| device.name.clone())
            .collect(),
    ));
    window.set_default_output_ids(string_model(
        snapshot
            .mixer
            .default_outputs
            .iter()
            .map(|device| device.id.clone())
            .collect(),
    ));
    let mut physical_output_names = vec!["Unassigned".to_string()];
    physical_output_names.extend(
        snapshot
            .mixer
            .physical_outputs
            .iter()
            .map(|device| device.name.clone()),
    );
    let mut physical_output_ids = vec![String::new()];
    physical_output_ids.extend(
        snapshot
            .mixer
            .physical_outputs
            .iter()
            .map(|device| device.node_id.to_string()),
    );
    window.set_physical_output_names(string_model(physical_output_names));
    window.set_physical_output_ids(string_model(physical_output_ids));
    window.set_physical_input_names(string_model(
        snapshot
            .mixer
            .physical_inputs
            .iter()
            .map(|device| device.name.clone())
            .collect(),
    ));
    window.set_physical_input_ids(string_model(
        snapshot
            .mixer
            .physical_inputs
            .iter()
            .map(|device| device.node_id.to_string())
            .collect(),
    ));
    window.set_link_output_target_names(string_model(link_output_target_names));
    window.set_link_output_target_ids(string_model(link_output_target_ids));
    window.set_default_input_index(default_input_index);
    window.set_default_output_index(default_output_index);
    window.set_sources(ModelRc::from(Rc::new(VecModel::from(sources))));
    window.set_routes(ModelRc::from(Rc::new(VecModel::from(routes))));
    window.set_linux_applications(ModelRc::from(Rc::new(VecModel::from(linux_applications))));
    window.set_link_applications(ModelRc::from(Rc::new(VecModel::from(link_applications))));
    window.set_link_outputs(ModelRc::from(Rc::new(VecModel::from(link_outputs))));
    window.set_targets(ModelRc::from(Rc::new(VecModel::from(targets))));
    window.set_mixer_channel_names(string_model(channel_names));
}

fn physical_device_matches(
    left: &MixerPhysicalDeviceDescriptor,
    right: &MixerPhysicalDeviceDescriptor,
) -> bool {
    match (&left.name, &right.name) {
        (Some(left), Some(right)) => left == right,
        _ => left.description.is_some() && left.description == right.description,
    }
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
            studiobridge_core::MixerSourceKind::Virtual => String::new(),
        }
    } else {
        channel.applications.join("\n")
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
            window.set_status_alert(false);
            window.set_status_text(status.into());
        }
    });
}

fn daemon_write_allowed(window: &Weak<MainWindow>, action: &str) -> bool {
    if DAEMON_READY.load(Ordering::SeqCst) {
        true
    } else {
        set_alert_status(
            window.clone(),
            format!("{action} was not attempted: daemon is not ready"),
        );
        false
    }
}

fn set_alert_status(window: Weak<MainWindow>, status: String) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window.upgrade() {
            window.set_status_alert(true);
            window.set_status_text(status.into());
        }
    });
}

fn set_daemon_failure(window: Weak<MainWindow>, presentation: DaemonStatusPresentation) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(window) = window.upgrade() {
            apply_daemon_failure(&window, &presentation);
        }
    });
}

fn load_dsp(
    window: Weak<MainWindow>,
    client: DaemonClient,
    session: Arc<Mutex<DspSession>>,
    capture: bool,
    preserve_selection: bool,
) {
    thread::spawn(move || match client.microphone_dsp() {
        Ok(snapshot) => {
            let (selected, eq_band) = if let Ok(mut state) = session.lock() {
                state.snapshot = Some(snapshot.clone());
                if capture {
                    state.captured = Some(snapshot.clone());
                }
                (state.selected, state.selected_eq_band)
            } else {
                return;
            };
            show_dsp_snapshot_inner(&window, &snapshot, selected, eq_band, !preserve_selection);
            set_status(window, "Microphone DSP read-back is current".into());
        }
        Err(error) => set_daemon_failure(
            window,
            daemon_status_for_error(&error, "DSP read-back failed"),
        ),
    });
}

fn arm_dsp(window: Weak<MainWindow>, client: DaemonClient, session: Arc<Mutex<DspSession>>) {
    if !daemon_write_allowed(&window, "DSP arm") {
        return;
    }
    thread::spawn(move || {
        let (selected, eq_band) = session
            .lock()
            .map(|state| (state.selected, state.selected_eq_band))
            .unwrap_or_default();
        let snapshot = match client.microphone_dsp() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                refresh_after_mutation(window, client, Err(error), "DSP arm");
                return;
            }
        };
        if let Ok(mut state) = session.lock() {
            state.snapshot = Some(snapshot.clone());
            state.captured = Some(snapshot.clone());
        }
        let result = client.arm_dsp(selected.module(), 300);
        if result.is_ok() {
            show_dsp_snapshot(&window, &snapshot, selected, eq_band);
            set_dsp_lease(window.clone(), Some(selected.as_str().into()), Some(300));
        }
        refresh_after_mutation(window, client, result, "DSP arm");
    });
}

fn apply_dsp(
    window: Weak<MainWindow>,
    client: DaemonClient,
    session: Arc<Mutex<DspSession>>,
    values: (f32, f32, f32, f32, f32),
    revert: bool,
) {
    if !daemon_write_allowed(&window, "DSP change") {
        return;
    }
    thread::spawn(move || {
        let (selected, eq_band, base) = match session.lock() {
            Ok(state) => {
                let snapshot = if revert {
                    state.captured.clone()
                } else {
                    state.snapshot.clone()
                };
                (state.selected, state.selected_eq_band, snapshot)
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
            update_from_values(
                selected, base, values.0, values.1, values.2, values.3, values.4,
            )
        };
        match client.set_microphone_dsp(&update) {
            Ok(result) if result.verified => {
                if let Ok(mut state) = session.lock() {
                    state.snapshot = Some(result.snapshot.clone());
                }
                show_dsp_snapshot(&window, &result.snapshot, selected, eq_band);
                set_status(
                    window,
                    if revert {
                        "Captured DSP state restored and verified".into()
                    } else {
                        "DSP change applied and verified by read-back".into()
                    },
                );
            }
            Ok(_) => match client.microphone_dsp() {
                Ok(snapshot) => {
                    if let Ok(mut state) = session.lock() {
                        state.snapshot = Some(snapshot.clone());
                    }
                    show_dsp_snapshot(&window, &snapshot, selected, eq_band);
                    set_alert_status(
                        window,
                        "DSP write did not verify; current DSP state was reloaded".into(),
                    );
                }
                Err(error) => set_daemon_failure(
                    window,
                    daemon_status_for_error(
                        &error,
                        "DSP write did not verify and read-back failed",
                    ),
                ),
            },
            Err(error) => {
                let failure = daemon_status_for_error(&error, "DSP change was not confirmed");
                match client.microphone_dsp() {
                    Ok(snapshot) => {
                        if let Ok(mut state) = session.lock() {
                            state.snapshot = Some(snapshot.clone());
                        }
                        show_dsp_snapshot(&window, &snapshot, selected, eq_band);
                        set_alert_status(
                            window,
                            format!("{}; current DSP state was reloaded", failure.status),
                        );
                    }
                    Err(read_error) => set_daemon_failure(
                        window,
                        daemon_status_for_error(
                            &read_error,
                            "DSP change was not confirmed and read-back failed",
                        ),
                    ),
                }
            }
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
    d: f32,
    e: f32,
) -> MicrophoneDspUpdate {
    match selected {
        DspSelection::Equalizer => MicrophoneDspUpdate::Equalizer(snapshot.equalizer),
        DspSelection::Compressor => {
            let advanced = snapshot.compressor.active_mode == DspMode::Advanced;
            let profile = match snapshot.compressor.active_mode {
                DspMode::Simple => &mut snapshot.compressor.simple,
                DspMode::Advanced => &mut snapshot.compressor.advanced,
            };
            profile.threshold_db = a;
            if advanced {
                profile.ratio = b;
                profile.attack_ms = c;
                profile.release_ms = d;
            }
            profile.makeup_gain_db = e;
            MicrophoneDspUpdate::Compressor(snapshot.compressor)
        }
        DspSelection::Expander => {
            let profile = match snapshot.expander.active_mode {
                DspMode::Simple => &mut snapshot.expander.simple,
                DspMode::Advanced => &mut snapshot.expander.advanced,
            };
            profile.threshold_db = a;
            profile.ratio = b;
            profile.attack_ms = c;
            profile.release_ms = d;
            MicrophoneDspUpdate::Expander(snapshot.expander)
        }
        DspSelection::NoiseSuppression => {
            snapshot.noise_suppression.amount_percent = a;
            snapshot.noise_suppression.sensitivity_db = b;
            snapshot.noise_suppression.adapt_time_ms = c;
            MicrophoneDspUpdate::NoiseSuppression(snapshot.noise_suppression)
        }
        DspSelection::EnhancementSuite => {
            MicrophoneDspUpdate::EnhancementSuite(snapshot.enhancement_suite)
        }
        DspSelection::HeadphoneEqualizer => {
            MicrophoneDspUpdate::HeadphoneEqualizer(snapshot.headphone_equalizer)
        }
    }
}

fn active_eq_profile(snapshot: &MicrophoneDspSnapshot) -> &EqualizerProfile {
    match snapshot.equalizer.active_mode {
        DspMode::Simple => &snapshot.equalizer.simple,
        DspMode::Advanced => &snapshot.equalizer.advanced,
    }
}

fn active_eq_profile_mut(snapshot: &mut MicrophoneDspSnapshot) -> &mut EqualizerProfile {
    match snapshot.equalizer.active_mode {
        DspMode::Simple => &mut snapshot.equalizer.simple,
        DspMode::Advanced => &mut snapshot.equalizer.advanced,
    }
}

const fn eq_band_type_name(band_type: EqualizerBandType) -> &'static str {
    match band_type {
        EqualizerBandType::NotSet => "not_set",
        EqualizerBandType::LowPass => "low_pass",
        EqualizerBandType::HighPass => "high_pass",
        EqualizerBandType::Notch => "notch",
        EqualizerBandType::Bell => "bell",
        EqualizerBandType::LowShelf => "low_shelf",
        EqualizerBandType::HighShelf => "high_shelf",
    }
}

fn parse_eq_band_type(value: &str) -> Option<EqualizerBandType> {
    match value {
        "low_pass" => Some(EqualizerBandType::LowPass),
        "high_pass" => Some(EqualizerBandType::HighPass),
        "notch" => Some(EqualizerBandType::Notch),
        "bell" => Some(EqualizerBandType::Bell),
        "low_shelf" => Some(EqualizerBandType::LowShelf),
        "high_shelf" => Some(EqualizerBandType::HighShelf),
        _ => None,
    }
}

fn set_eq_band_type_value(
    snapshot: &mut MicrophoneDspSnapshot,
    index: usize,
    value: &str,
) -> Option<()> {
    let band_type = parse_eq_band_type(value)?;
    let band = active_eq_profile_mut(snapshot).bands.get_mut(index)?;
    band.band_type = band_type;
    band.enabled = true;
    Some(())
}

fn adjust_eq_band_value(
    snapshot: &mut MicrophoneDspSnapshot,
    index: usize,
    field: &str,
    direction: i32,
) -> Option<()> {
    let direction = direction.signum() as f32;
    if direction == 0.0 {
        return None;
    }
    let band = active_eq_profile_mut(snapshot).bands.get_mut(index)?;
    match field {
        "frequency" => {
            let step = if band.frequency_hz < 100.0 {
                1.0
            } else if band.frequency_hz < 1_000.0 {
                10.0
            } else if band.frequency_hz < 10_000.0 {
                100.0
            } else {
                1_000.0
            };
            band.frequency_hz = (band.frequency_hz + direction * step).clamp(20.0, 20_000.0);
        }
        "gain" => band.gain_db = (band.gain_db + direction * 0.5).clamp(-12.0, 12.0),
        "q" => band.q = (band.q + direction * 0.1).clamp(0.1, 10.0),
        _ => return None,
    }
    Some(())
}

fn set_eq_band_value(
    snapshot: &mut MicrophoneDspSnapshot,
    index: usize,
    field: &str,
    value: f32,
) -> Option<()> {
    let band = active_eq_profile_mut(snapshot).bands.get_mut(index)?;
    match field {
        "frequency" => band.frequency_hz = value.clamp(20.0, 20_000.0),
        "gain" => band.gain_db = value.clamp(-12.0, 12.0),
        "q" => band.q = value.clamp(0.1, 10.0),
        _ => return None,
    }
    Some(())
}

fn add_eq_band(snapshot: &mut MicrophoneDspSnapshot) -> Option<usize> {
    let profile = active_eq_profile_mut(snapshot);
    let (index, band) = profile
        .bands
        .iter_mut()
        .enumerate()
        .find(|(_, band)| !band.enabled || band.band_type == EqualizerBandType::NotSet)?;
    band.enabled = true;
    if band.band_type == EqualizerBandType::NotSet {
        band.band_type = EqualizerBandType::Bell;
    }
    Some(index)
}

fn remove_eq_band(snapshot: &mut MicrophoneDspSnapshot, index: usize) -> Option<()> {
    let band = active_eq_profile_mut(snapshot).bands.get_mut(index)?;
    band.enabled = false;
    band.band_type = EqualizerBandType::NotSet;
    Some(())
}

fn set_enhancement_preset(snapshot: &mut MicrophoneDspSnapshot, preset: i32) -> Option<()> {
    let preset = u8::try_from(preset).ok()?;
    if !(1..=4).contains(&preset) {
        return None;
    }
    snapshot.enhancement_suite.bass.preset = preset;
    Some(())
}

fn set_enhancement_value(
    snapshot: &mut MicrophoneDspSnapshot,
    field: &str,
    value: f32,
) -> Option<()> {
    if !value.is_finite() {
        return None;
    }
    let suite = &mut snapshot.enhancement_suite;
    match field {
        "bass_amount" => {
            suite.bass.amount = value.clamp(0.0, 10.0);
            suite.bass.enabled = suite.bass.amount > 0.0;
        }
        "de_esser_amount" => {
            suite.de_esser.amount_percent = value.clamp(0.0, 100.0);
            suite.de_esser.enabled = suite.de_esser.amount_percent > 0.0;
        }
        "exciter_amount" => {
            suite.exciter.amount_percent = value.clamp(0.0, 100.0);
            suite.exciter.enabled = suite.exciter.amount_percent > 0.0;
        }
        "exciter_frequency" => suite.exciter.frequency_hz = value.clamp(0.0, 5_000.0),
        _ => return None,
    }
    Some(())
}

fn set_headphone_eq_band_value(
    snapshot: &mut MicrophoneDspSnapshot,
    index: i32,
    value: f32,
) -> Option<()> {
    if !value.is_finite() {
        return None;
    }
    let index = usize::try_from(index).ok()?;
    snapshot.headphone_equalizer.bands.get_mut(index)?.amount_db = value.clamp(-12.0, 12.0);
    Some(())
}

fn set_headphone_subwoofer_value(snapshot: &mut MicrophoneDspSnapshot, value: f32) -> Option<()> {
    if !value.is_finite() {
        return None;
    }
    let amount = value.round().clamp(0.0, 10.0) as u8;
    snapshot.headphone_equalizer.subwoofer.amount = amount;
    snapshot.headphone_equalizer.subwoofer.enabled = amount > 0;
    Some(())
}

fn show_dsp_snapshot(
    window: &Weak<MainWindow>,
    snapshot: &MicrophoneDspSnapshot,
    selected: DspSelection,
    selected_eq_band: usize,
) {
    show_dsp_snapshot_inner(window, snapshot, selected, selected_eq_band, true);
}

fn show_dsp_snapshot_inner(
    window: &Weak<MainWindow>,
    snapshot: &MicrophoneDspSnapshot,
    selected: DspSelection,
    selected_eq_band: usize,
    update_selection: bool,
) {
    let (title, labels, values, ranges) = dsp_display(snapshot, selected);
    let advanced = match selected {
        DspSelection::Equalizer => snapshot.equalizer.active_mode == DspMode::Advanced,
        DspSelection::Compressor => snapshot.compressor.active_mode == DspMode::Advanced,
        DspSelection::Expander => snapshot.expander.active_mode == DspMode::Advanced,
        _ => false,
    };
    let noise_snapshot = snapshot.noise_suppression.style == NoiseSuppressionStyle::Snapshot;
    let enabled = selected_dsp_enabled(snapshot, selected);
    let eq_profile = active_eq_profile(snapshot);
    let selected_eq_band = selected_eq_band.min(eq_profile.bands.len().saturating_sub(1));
    let selected_band = eq_profile.bands.get(selected_eq_band);
    let eq_band_type = selected_band
        .map(|band| eq_band_type_name(band.band_type))
        .unwrap_or("not_set");
    let eq_frequency_hz = selected_band.map_or(1_000.0, |band| band.frequency_hz);
    let eq_gain_db = selected_band.map_or(0.0, |band| band.gain_db);
    let eq_q = selected_band.map_or(1.0, |band| band.q);
    let eq_band_enabled = selected_band.is_some_and(|band| band.enabled);
    let eq_advanced = snapshot.equalizer.active_mode == DspMode::Advanced;
    let eq_can_add_band = eq_profile
        .bands
        .iter()
        .any(|band| !band.enabled || band.band_type == EqualizerBandType::NotSet);
    let eq_bands = eq_profile
        .bands
        .iter()
        .enumerate()
        .map(|(index, band)| EqBandModel {
            band_index: index as i32,
            x_percent: ((band.frequency_hz.max(20.0).log10() - 20.0_f32.log10())
                / (20_000.0_f32.log10() - 20.0_f32.log10())
                * 100.0)
                .clamp(0.0, 100.0),
            y_percent: ((12.0 - band.gain_db) / 24.0 * 100.0).clamp(0.0, 100.0),
            colour: eq_band_colour(index),
            selected: index == selected_eq_band,
            enabled: band.enabled,
        })
        .collect::<Vec<_>>();
    let enhancement = &snapshot.enhancement_suite;
    let enhancement_bass_preset = i32::from(enhancement.bass.preset);
    let enhancement_bass_amount = enhancement.bass.amount;
    let enhancement_de_esser_amount = enhancement.de_esser.amount_percent;
    let enhancement_exciter_amount = enhancement.exciter.amount_percent;
    let enhancement_exciter_frequency = enhancement.exciter.frequency_hz;
    let enhancement_bass_enabled = enhancement.bass.enabled;
    let enhancement_de_esser_enabled = enhancement.de_esser.enabled;
    let enhancement_exciter_enabled = enhancement.exciter.enabled;
    let headphone_subwoofer_enabled = snapshot.headphone_equalizer.subwoofer.enabled;
    let headphone_subwoofer_amount = snapshot.headphone_equalizer.subwoofer.amount as f32;
    let window = window.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(window) = window.upgrade() else {
            return;
        };
        if update_selection {
            window.set_dsp_selected_module(selected.as_str().into());
        }
        window.set_dsp_editor_title(title.into());
        window.set_dsp_label_a(labels.0.into());
        window.set_dsp_label_b(labels.1.into());
        window.set_dsp_label_c(labels.2.into());
        window.set_dsp_label_d(labels.3.into());
        window.set_dsp_value_a(values.0);
        window.set_dsp_value_b(values.1);
        window.set_dsp_value_c(values.2);
        window.set_dsp_value_d(values.3);
        window.set_dsp_value_e(values.4);
        window.set_dsp_advanced(advanced);
        window.set_dsp_enabled(enabled);
        window.set_noise_snapshot(noise_snapshot);
        window.set_eq_selected_band(selected_eq_band as i32);
        window.set_eq_advanced(eq_advanced);
        window.set_eq_band_type(eq_band_type.into());
        window.set_eq_frequency_hz(eq_frequency_hz);
        window.set_eq_gain_db(eq_gain_db);
        window.set_eq_q(eq_q);
        window.set_eq_band_enabled(eq_band_enabled);
        window.set_eq_can_add_band(eq_can_add_band);
        window.set_eq_bands(ModelRc::from(Rc::new(VecModel::from(eq_bands))));
        window.set_enhancement_bass_preset(enhancement_bass_preset);
        window.set_enhancement_bass_amount(enhancement_bass_amount);
        window.set_enhancement_de_esser_amount(enhancement_de_esser_amount);
        window.set_enhancement_exciter_amount(enhancement_exciter_amount);
        window.set_enhancement_exciter_frequency(enhancement_exciter_frequency);
        window.set_enhancement_bass_enabled(enhancement_bass_enabled);
        window.set_enhancement_de_esser_enabled(enhancement_de_esser_enabled);
        window.set_enhancement_exciter_enabled(enhancement_exciter_enabled);
        window.set_headphone_subwoofer_enabled(headphone_subwoofer_enabled);
        window.set_headphone_subwoofer_amount(headphone_subwoofer_amount);
        window.set_dsp_min_a(ranges.0.0);
        window.set_dsp_max_a(ranges.0.1);
        window.set_dsp_min_b(ranges.1.0);
        window.set_dsp_max_b(ranges.1.1);
        window.set_dsp_min_c(ranges.2.0);
        window.set_dsp_max_c(ranges.2.1);
        window.set_dsp_min_d(ranges.3.0);
        window.set_dsp_max_d(ranges.3.1);
        window.set_dsp_min_e(ranges.4.0);
        window.set_dsp_max_e(ranges.4.1);
    });
}

fn eq_band_colour(index: usize) -> Color {
    const COLOURS: [&str; 8] = [
        "#ef4f58", "#42c9c6", "#e4b638", "#ef4c8c", "#9d70d6", "#58a7e8", "#e48145", "#7cc65b",
    ];
    parse_colour(COLOURS[index % COLOURS.len()])
}

fn selected_dsp_enabled(snapshot: &MicrophoneDspSnapshot, selected: DspSelection) -> bool {
    match selected {
        DspSelection::Compressor => match snapshot.compressor.active_mode {
            DspMode::Simple => snapshot.compressor.simple.enabled,
            DspMode::Advanced => snapshot.compressor.advanced.enabled,
        },
        DspSelection::Expander => match snapshot.expander.active_mode {
            DspMode::Simple => snapshot.expander.simple.enabled,
            DspMode::Advanced => snapshot.expander.advanced.enabled,
        },
        DspSelection::NoiseSuppression => snapshot.noise_suppression.enabled,
        DspSelection::HeadphoneEqualizer => snapshot
            .headphone_equalizer
            .bands
            .iter()
            .any(|band| band.enabled),
        DspSelection::Equalizer | DspSelection::EnhancementSuite => true,
    }
}

fn set_selected_dsp_enabled(
    snapshot: &mut MicrophoneDspSnapshot,
    selected: DspSelection,
    enabled: bool,
) {
    match selected {
        DspSelection::Compressor => match snapshot.compressor.active_mode {
            DspMode::Simple => snapshot.compressor.simple.enabled = enabled,
            DspMode::Advanced => snapshot.compressor.advanced.enabled = enabled,
        },
        DspSelection::Expander => match snapshot.expander.active_mode {
            DspMode::Simple => snapshot.expander.simple.enabled = enabled,
            DspMode::Advanced => snapshot.expander.advanced.enabled = enabled,
        },
        DspSelection::NoiseSuppression => snapshot.noise_suppression.enabled = enabled,
        DspSelection::HeadphoneEqualizer => {
            for band in &mut snapshot.headphone_equalizer.bands {
                band.enabled = enabled;
            }
        }
        DspSelection::Equalizer | DspSelection::EnhancementSuite => {}
    }
}

type DspDisplay = (
    &'static str,
    (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ),
    (f32, f32, f32, f32, f32),
    ((f32, f32), (f32, f32), (f32, f32), (f32, f32), (f32, f32)),
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
                ("Band 1 gain", "Band 2 gain", "Band 3 gain", "", ""),
                (value(0), value(1), value(2), 0.0, 0.0),
                (
                    (-12.0, 12.0),
                    (-12.0, 12.0),
                    (-12.0, 12.0),
                    (0.0, 1.0),
                    (0.0, 1.0),
                ),
            )
        }
        DspSelection::Compressor => {
            let p = match snapshot.compressor.active_mode {
                DspMode::Simple => &snapshot.compressor.simple,
                DspMode::Advanced => &snapshot.compressor.advanced,
            };
            (
                "COMPRESSOR",
                (
                    "Threshold dB",
                    "Ratio",
                    "Attack ms",
                    "Release ms",
                    "Makeup gain dB",
                ),
                (
                    p.threshold_db,
                    p.ratio,
                    p.attack_ms,
                    p.release_ms,
                    p.makeup_gain_db,
                ),
                (
                    (-50.0, 0.0),
                    (1.0, 16.0),
                    (1.0, 2_000.0),
                    (1.0, 2_000.0),
                    (0.0, 12.0),
                ),
            )
        }
        DspSelection::Expander => {
            let p = match snapshot.expander.active_mode {
                DspMode::Simple => &snapshot.expander.simple,
                DspMode::Advanced => &snapshot.expander.advanced,
            };
            (
                "EXPANDER / GATE",
                ("Threshold dB", "Ratio", "Attack ms", "Release ms", ""),
                (p.threshold_db, p.ratio, p.attack_ms, p.release_ms, 0.0),
                (
                    (-90.0, 0.0),
                    (1.0, 10.0),
                    (1.0, 2000.0),
                    (1.0, 2000.0),
                    (0.0, 1.0),
                ),
            )
        }
        DspSelection::NoiseSuppression => (
            "NOISE SUPPRESSION",
            ("Amount %", "Sensitivity dB", "Adapt time ms", "", ""),
            (
                snapshot.noise_suppression.amount_percent,
                snapshot.noise_suppression.sensitivity_db,
                snapshot.noise_suppression.adapt_time_ms,
                0.0,
                0.0,
            ),
            (
                (0.0, 100.0),
                (-120.0, -60.0),
                (100.0, 5000.0),
                (0.0, 1.0),
                (0.0, 1.0),
            ),
        ),
        DspSelection::EnhancementSuite => (
            "ENHANCEMENT SUITE",
            ("Bass amount", "De-esser %", "Exciter %", "", ""),
            (
                snapshot.enhancement_suite.bass.amount,
                snapshot.enhancement_suite.de_esser.amount_percent,
                snapshot.enhancement_suite.exciter.amount_percent,
                0.0,
                0.0,
            ),
            (
                (0.0, 10.0),
                (0.0, 100.0),
                (0.0, 100.0),
                (0.0, 5_000.0),
                (0.0, 1.0),
            ),
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
                ("Bass dB", "Mids dB", "Treble dB", "", ""),
                (value(0), value(1), value(2), 0.0, 0.0),
                (
                    (-12.0, 12.0),
                    (-12.0, 12.0),
                    (-12.0, 12.0),
                    (0.0, 1.0),
                    (0.0, 1.0),
                ),
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

fn apply_default_device_repair(
    client: &DaemonClient,
    repair: &DefaultDeviceRepair,
) -> Result<Option<String>, String> {
    if let Some(device_id) = repair.input.as_deref() {
        client
            .set_default_input(device_id)
            .map_err(|error| format!("recording device restore failed: {error}"))?;
    }
    if let Some(device_id) = repair.output.as_deref() {
        client
            .set_default_output(device_id)
            .map_err(|error| format!("playback device restore failed: {error}"))?;
    }
    let status = match (repair.input.is_some(), repair.output.is_some()) {
        (true, true) => Some("Preferred recording and playback devices restored".into()),
        (true, false) => Some("Preferred recording device restored".into()),
        (false, true) => Some("Preferred playback device restored".into()),
        (false, false) => None,
    };
    Ok(status)
}

fn start_default_device_monitor(window: Weak<MainWindow>, client: DaemonClient) {
    thread::spawn(move || {
        let mut last_error = None;
        loop {
            let preferences = load_preferences();
            if DAEMON_READY.load(Ordering::SeqCst)
                && preferences.automatic_default_reset
                && let Ok(snapshot) = client.snapshot()
            {
                let repair = default_device_repair(&preferences, &snapshot);
                match apply_default_device_repair(&client, &repair) {
                    Ok(Some(status)) => {
                        last_error = None;
                        set_status(window.clone(), status);
                        refresh_all(window.clone(), client.clone());
                    }
                    Ok(None) => last_error = None,
                    Err(error) if last_error.as_ref() != Some(&error) => {
                        set_status(
                            window.clone(),
                            format!("Automatic default device reset failed: {error}"),
                        );
                        last_error = Some(error);
                    }
                    Err(_) => {}
                }
            } else {
                last_error = None;
            }
            thread::sleep(Duration::from_secs(2));
        }
    });
}

fn start_health_monitor(
    window: Weak<MainWindow>,
    client: DaemonClient,
    session: Arc<Mutex<DspSession>>,
) {
    thread::spawn(move || {
        loop {
            let Some(generation) = REFRESH_COORDINATOR.monitor_generation() else {
                thread::sleep(Duration::from_secs(1));
                continue;
            };
            match client.health() {
                Ok(health) => {
                    let active = health.dsp_write_active_module.clone();
                    let seconds = health.dsp_write_lease_seconds;
                    let armed = active.is_some();
                    let update_window = window.clone();
                    let recovery_client = client.clone();
                    let recovery_session = session.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(window) = update_window.upgrade() else {
                            return;
                        };
                        let Some(_refresh_ordering) =
                            REFRESH_COORDINATOR.lock_if_current(generation)
                        else {
                            return;
                        };
                        let recovered = monitor_connection_recovered(
                            health.ok,
                            DAEMON_READY.load(Ordering::SeqCst),
                        );
                        if health.ok {
                            DAEMON_READY.store(true, Ordering::SeqCst);
                            window.set_daemon_connected(true);
                            window.set_hardware_safe(!health.hardware_writes_enabled);
                            window.set_link_active(health.link_control_enabled);
                            window.set_dsp_gate_count(health.dsp_write_modules.len() as i32);
                            window.set_daemon_status_kind("ready".into());
                            window.set_daemon_status_title("STUDIOBRIDGE DAEMON IS READY".into());
                            window.set_daemon_status_detail("Local audio state is current.".into());
                            window.set_dsp_active_module(active.unwrap_or_default().into());
                            window.set_dsp_lease_seconds(seconds.unwrap_or_default() as i32);
                            if armed {
                                window.set_arm_confirm_pending(false);
                            }
                        } else {
                            apply_daemon_failure(
                                &window,
                                &daemon_not_ready_status("Connection monitor failed"),
                            );
                            window.set_dsp_active_module("".into());
                            window.set_dsp_lease_seconds(0);
                        }
                        drop(_refresh_ordering);
                        if recovered {
                            let recovery_window = window.as_weak();
                            refresh_all(recovery_window.clone(), recovery_client.clone());
                            load_dsp(
                                recovery_window,
                                recovery_client,
                                recovery_session,
                                false,
                                true,
                            );
                        }
                    });
                }
                Err(error) => {
                    let update_window = window.clone();
                    let presentation = daemon_status_for_error(&error, "Connection monitor failed");
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(window) = update_window.upgrade() else {
                            return;
                        };
                        let Some(_refresh_ordering) =
                            REFRESH_COORDINATOR.lock_if_current(generation)
                        else {
                            return;
                        };
                        apply_daemon_failure(&window, &presentation);
                        window.set_dsp_active_module("".into());
                        window.set_dsp_lease_seconds(0);
                    });
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

fn monitor_connection_recovered(health_ready: bool, daemon_was_ready: bool) -> bool {
    health_ready && !daemon_was_ready
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
    use super::{
        AppPreferences, DaemonErrorKind, DspSelection, PreferredDefaultReadback,
        RefreshCoordinator, RefreshStatus, active_eq_profile, add_eq_band, adjust_eq_band_value,
        application_chip_text_width, channel_detail, daemon_not_ready_status,
        daemon_status_for_kind, default_mutation_refresh_status, default_selection_matches,
        dsp_display, external_link_url, hotkey_key_name, link_channel_for_source,
        link_channel_label, merge_pending_refresh_status, mock_meter_level,
        monitor_connection_recovered, mutation_readback_is_authoritative, mutation_refresh_status,
        mute_state_with_target, normalize_captured_hotkey, normalize_mute_action,
        preferred_default_readback, preferred_default_repair, profile_mutation_refresh_status,
        remove_eq_band, selected_dsp_enabled, set_enhancement_preset, set_enhancement_value,
        set_eq_band_type_value, set_eq_band_value, set_headphone_eq_band_value,
        set_headphone_subwoofer_value, set_selected_dsp_enabled,
        should_preserve_drawer_window_size, should_start_in_background, source_name_available,
        update_from_values,
    };
    use global_hotkey::hotkey::HotKey;
    use slint::platform::Key;
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };
    use studiobridge_core::{
        AppSnapshot, BackendStatus, DspMode, EqualizerBandState, EqualizerBandType,
        HeadphoneEqualizerState, HeadphoneOutputMode, HeadphoneState, LinkChannel,
        MicrophoneDspSnapshot, MicrophoneDspUpdate, MicrophoneState, MixerChannel,
        MixerDeviceChoice, MixerSnapshot, MixerSourceKind, MuteState, StudioIdentity,
        StudioSnapshot,
    };

    fn take_refresh_status(
        coordinator: &RefreshCoordinator,
        generation: u64,
    ) -> Option<Option<RefreshStatus>> {
        let mut ordering = coordinator.lock_if_current(generation)?;
        Some(ordering.pending_status.take())
    }

    fn settle_refresh_status(
        coordinator: &RefreshCoordinator,
        generation: u64,
        health_ready: bool,
        snapshot_ready: bool,
        profiles_ready: bool,
    ) -> Option<RefreshStatus> {
        let mut ordering = coordinator
            .lock_if_current(generation)
            .expect("test generation should remain current");
        let status = ordering.pending_status.clone();
        if status.as_ref().is_some_and(|status| {
            mutation_readback_is_authoritative(status, health_ready, snapshot_ready, profiles_ready)
        }) {
            ordering.pending_status = None;
        }
        status
    }

    fn default_snapshot(input: Option<&str>, output: Option<&str>) -> AppSnapshot {
        AppSnapshot {
            studio: StudioSnapshot {
                identity: StudioIdentity {
                    status: BackendStatus::Mock,
                    error: None,
                    product: "test".into(),
                    serial: None,
                    firmware: None,
                    usb_port: "test".into(),
                    driverless_mode: true,
                },
                microphone: MicrophoneState {
                    gain_db: 0,
                    phantom_power: false,
                    muted: false,
                },
                headphones: HeadphoneState {
                    volume: 0,
                    mic_monitor: 0,
                    muted: false,
                    channels_linked: false,
                    output_mode: HeadphoneOutputMode::LineLevel,
                    mic_output_gain_tenths_db: 0,
                },
                linked_applications: Vec::new(),
            },
            mixer: MixerSnapshot {
                status: BackendStatus::Mock,
                error: None,
                engine: "test".into(),
                channels: Vec::new(),
                targets: Vec::new(),
                routes: Vec::new(),
                applications: Vec::new(),
                default_input: input.map(str::to_owned),
                default_output: output.map(str::to_owned),
                default_inputs: Vec::new(),
                default_outputs: Vec::new(),
                physical_outputs: Vec::new(),
                physical_inputs: Vec::new(),
                link_outputs: Vec::new(),
            },
        }
    }

    fn dsp_snapshot() -> MicrophoneDspSnapshot {
        let mut snapshot: MicrophoneDspSnapshot = serde_json::from_value(serde_json::json!({
            "equalizer": {
                "active_mode": "simple",
                "simple": { "mode": "simple", "bands": [] },
                "advanced": { "mode": "advanced", "bands": [] }
            },
            "compressor": {
                "active_mode": "simple",
                "simple": { "mode": "simple", "enabled": true, "threshold_db": -18.0, "ratio": 3.0, "attack_ms": 10.0, "release_ms": 120.0, "makeup_gain_db": 3.0 },
                "advanced": { "mode": "advanced", "enabled": true, "threshold_db": -18.0, "ratio": 3.0, "attack_ms": 10.0, "release_ms": 120.0, "makeup_gain_db": 3.0 }
            },
            "expander": {
                "active_mode": "advanced",
                "simple": { "mode": "simple", "enabled": true, "threshold_db": -48.0, "ratio": 2.0, "attack_ms": 10.0, "release_ms": 180.0 },
                "advanced": { "mode": "advanced", "enabled": true, "threshold_db": -48.0, "ratio": 2.0, "attack_ms": 10.0, "release_ms": 180.0 }
            },
            "noise_suppression": { "enabled": true, "style": "adaptive", "amount_percent": 70.0, "sensitivity_db": -85.0, "adapt_time_ms": 1000.0 },
            "enhancement_suite": {
                "bass": { "enabled": false, "preset": 1, "amount": 0.0, "drive": 0.0, "mix_percent": 0.0, "attack_ms": 10.0, "release_ms": 250.0, "threshold_db": -27.0, "knee": 2.0, "makeup_gain_db": 0.0, "ratio": 4.0, "cutoff_hz": 100.0, "q": 0.7, "lower_cutoff_hz": 40.0, "lower_q": 0.2 },
                "de_esser": { "enabled": true, "amount_percent": 35.0 },
                "exciter": { "enabled": false, "amount_percent": 0.0, "frequency_hz": 3000.0 }
            },
            "headphone_equalizer": { "bands": [
                { "band": "bass", "enabled": true, "amount_db": 1.5 },
                { "band": "mids", "enabled": true, "amount_db": 0.0 },
                { "band": "treble", "enabled": true, "amount_db": 2.0 }
            ], "subwoofer": { "enabled": false, "amount": 0 } }
        }))
        .expect("DSP fixture should deserialize");
        let bands = (1..=8)
            .map(|band| EqualizerBandState {
                band,
                band_type: EqualizerBandType::Bell,
                gain_db: 0.0,
                frequency_hz: 40.0 * 2.0_f32.powi((band - 1).into()),
                q: 1.0,
                enabled: true,
            })
            .collect::<Vec<_>>();
        snapshot.equalizer.simple.bands = bands.clone();
        snapshot.equalizer.advanced.bands = bands;
        snapshot
    }

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

    #[test]
    fn system_tray_preference_controls_background_launch_and_migrates() {
        let enabled = AppPreferences::default();
        assert!(should_start_in_background(true, &enabled));
        assert!(!should_start_in_background(false, &enabled));

        let disabled: AppPreferences = serde_json::from_value(serde_json::json!({
            "close_to_tray": false
        }))
        .unwrap();
        assert!(!disabled.open_to_system_tray);
        assert!(disabled.studio_profiles_expanded);
        assert!(disabled.mixer_profiles_expanded);
        assert!(disabled.profiles_drawer_open);
        assert!(disabled.preferred_default_input.is_none());
        assert!(disabled.preferred_default_output.is_none());
        assert!(!should_start_in_background(true, &disabled));

        let mut collapsed = disabled;
        collapsed.studio_profiles_expanded = false;
        collapsed.mixer_profiles_expanded = false;
        collapsed.profiles_drawer_open = false;
        collapsed.preferred_default_input = Some("voice-chat-mic".into());
        collapsed.preferred_default_output = Some("headphones".into());
        let serialized = serde_json::to_value(collapsed).unwrap();
        assert_eq!(serialized["open_to_system_tray"], false);
        assert_eq!(serialized["studio_profiles_expanded"], false);
        assert_eq!(serialized["mixer_profiles_expanded"], false);
        assert_eq!(serialized["profiles_drawer_open"], false);
        assert_eq!(serialized["preferred_default_input"], "voice-chat-mic");
        assert_eq!(serialized["preferred_default_output"], "headphones");
        assert!(serialized.get("close_to_tray").is_none());

        let restored: AppPreferences = serde_json::from_value(serialized).unwrap();
        assert!(!restored.studio_profiles_expanded);
        assert!(!restored.mixer_profiles_expanded);
        assert!(!restored.profiles_drawer_open);
        assert_eq!(
            restored.preferred_default_input.as_deref(),
            Some("voice-chat-mic")
        );
        assert_eq!(
            restored.preferred_default_output.as_deref(),
            Some("headphones")
        );
    }

    #[test]
    fn automatic_default_reset_only_restores_an_available_explicit_preference() {
        let choices = vec![
            MixerDeviceChoice {
                id: "headphones".into(),
                name: "Headphones".into(),
            },
            MixerDeviceChoice {
                id: "system".into(),
                name: "System".into(),
            },
        ];

        assert_eq!(
            preferred_default_repair(true, Some("headphones"), Some("system"), &choices),
            Some("headphones".into())
        );
        assert_eq!(
            preferred_default_repair(false, Some("headphones"), Some("system"), &choices),
            None
        );
        assert_eq!(
            preferred_default_repair(true, None, Some("system"), &choices),
            None
        );
        assert_eq!(
            preferred_default_repair(true, Some("headphones"), Some("headphones"), &choices),
            None
        );
        assert_eq!(
            preferred_default_repair(true, Some("disconnected"), Some("system"), &choices),
            None
        );
    }

    #[test]
    fn header_links_are_limited_to_fixed_https_destinations() {
        assert_eq!(
            external_link_url("project"),
            Some("https://github.com/dilllxd/StudioBridge")
        );
        assert_eq!(
            external_link_url("support"),
            Some("https://github.com/dilllxd/StudioBridge/issues")
        );
        assert_eq!(external_link_url("https://example.com"), None);
        assert_eq!(external_link_url("unknown"), None);
    }

    #[test]
    fn new_headphone_readback_fields_have_safe_upgrade_defaults() {
        let headphones: HeadphoneState = serde_json::from_value(serde_json::json!({
            "volume": 50,
            "mic_monitor": 25,
            "muted": false
        }))
        .unwrap();
        assert!(!headphones.channels_linked);
        assert_eq!(headphones.output_mode, HeadphoneOutputMode::LineLevel);
        assert_eq!(headphones.mic_output_gain_tenths_db, 0);

        let identity: StudioIdentity = serde_json::from_value(serde_json::json!({
            "status": "connected",
            "error": null,
            "product": "BEACN Studio",
            "serial": "test",
            "firmware": "1.0.0",
            "usb_port": "USB1"
        }))
        .unwrap();
        assert!(!identity.driverless_mode);

        let equalizer: HeadphoneEqualizerState = serde_json::from_value(serde_json::json!({
            "bands": [
                { "band": "bass", "enabled": true, "amount_db": 0.0 },
                { "band": "mids", "enabled": true, "amount_db": 0.0 },
                { "band": "treble", "enabled": true, "amount_db": 0.0 }
            ]
        }))
        .unwrap();
        assert!(!equalizer.subwoofer.enabled);
        assert_eq!(equalizer.subwoofer.amount, 0);
    }

    #[test]
    fn captured_hotkey_matches_beacn_modifier_order_and_is_registerable() {
        let f9 = char::from(Key::F9).to_string();
        let binding = normalize_captured_hotkey(&f9, true, true, true, false).unwrap();
        assert_eq!(binding, "ctrl + shift + alt + F9");
        assert!(binding.parse::<HotKey>().is_ok());
        assert_eq!(hotkey_key_name(&f9).as_deref(), Some("F9"));
    }

    #[test]
    fn captured_hotkey_ignores_modifier_only_events_and_maps_shifted_keys() {
        assert!(
            normalize_captured_hotkey(
                &char::from(Key::Control).to_string(),
                true,
                false,
                false,
                false
            )
            .is_none()
        );
        let shifted_digit = normalize_captured_hotkey("!", true, false, true, false).unwrap();
        assert_eq!(shifted_digit, "ctrl + shift + 1");
        assert!(shifted_digit.parse::<HotKey>().is_ok());

        let escape = char::from(Key::Escape).to_string();
        let escape_binding = normalize_captured_hotkey(&escape, false, false, false, false)
            .expect("Escape should be capturable like it is in BEACN");
        assert_eq!(escape_binding, "escape");
        assert!(escape_binding.parse::<HotKey>().is_ok());
    }

    #[test]
    fn source_mute_actions_match_beacn_targets() {
        assert_eq!(normalize_mute_action("all"), "all");
        assert_eq!(normalize_mute_action("personal"), "personal");
        assert_eq!(normalize_mute_action("audience"), "audience");
        assert_eq!(normalize_mute_action("unsupported"), "audience");

        assert_eq!(
            mute_state_with_target(MuteState::Unmuted, "all", true),
            MuteState::MutedAll
        );
        assert_eq!(
            mute_state_with_target(MuteState::MutedPersonal, "audience", true),
            MuteState::MutedAll
        );
        assert_eq!(
            mute_state_with_target(MuteState::MutedAll, "personal", false),
            MuteState::MutedAudience
        );
        assert_eq!(
            mute_state_with_target(MuteState::MutedAudience, "audience", false),
            MuteState::Unmuted
        );
    }

    #[test]
    fn source_detail_matches_beacn_compact_application_rows() {
        let mut channel = MixerChannel {
            id: "chat".into(),
            meter_id: "chat".into(),
            name: "Chat".into(),
            colour: "#42c9c6".into(),
            personal_volume: 50,
            audience_volume: 50,
            mute_state: MuteState::Unmuted,
            applications: Vec::new(),
            source_kind: MixerSourceKind::Virtual,
            volumes_linked: true,
            attached_devices: Vec::new(),
        };

        assert_eq!(channel_detail(&channel), "");
        channel.applications = vec!["Discord".into(), "Teams".into()];
        assert_eq!(channel_detail(&channel), "Discord\nTeams");
    }

    #[test]
    fn shared_selector_contract_keeps_selection_safe_until_commit() {
        let ui = include_str!("../ui/app-window.slint");

        assert!(ui.contains("property <int> option-count: root.model.length;"));
        assert!(ui.contains("height: root.option-count * 24px + 4px;"));
        assert!(ui.contains("text: root.selected ? \"✓\" : \"\";"));
        assert!(ui.contains("selected: index == root.current-index;"));
        assert!(ui.contains("if index != root.current-index {"));
        assert!(ui.contains("if event.text == Key.Escape && selector-menu.is-open"));
        assert!(
            ui.contains("root.highlighted-index = root.current-valid ? root.current-index : 0;")
        );

        let selector_start = ui.find("component BeacnSelector").unwrap();
        let selector_end = ui[selector_start..]
            .find("component InputDeviceRow")
            .map(|offset| selector_start + offset)
            .unwrap();
        let selector = &ui[selector_start..selector_end];
        let first_selection_commit = selector.find("root.selected(").unwrap();
        let down_arrow = selector.find("Key.DownArrow").unwrap();
        let up_arrow = selector.find("Key.UpArrow").unwrap();
        assert!(first_selection_commit < down_arrow);
        assert!(first_selection_commit < up_arrow);
        assert!(!selector[down_arrow..up_arrow].contains("root.selected("));
    }

    #[test]
    fn add_source_menu_disables_existing_names_case_insensitively() {
        let source_names = "Mic\nGame\nLink In";
        assert!(!source_name_available("Mic", source_names));
        assert!(!source_name_available("game", source_names));
        assert!(!source_name_available("LINK IN", source_names));
        assert!(source_name_available("Aux 1", source_names));
        assert!(source_name_available("Link 2 In", source_names));
    }

    #[test]
    fn application_chips_use_compact_beacn_destinations() {
        assert_eq!(link_channel_label(LinkChannel::System), "System");
        assert_eq!(link_channel_label(LinkChannel::Link4), "Link 4");
        assert_eq!(link_channel_for_source("System"), 0);
        assert_eq!(link_channel_for_source("Link In"), 1);
        assert_eq!(link_channel_for_source("Link 2 In"), 2);
        assert_eq!(link_channel_for_source("BEACN Link4"), 4);
        assert_eq!(link_channel_for_source("Chat"), -1);

        assert_eq!(application_chip_text_width("i", 28.0, 126.0), 28.0);
        assert_eq!(
            application_chip_text_width("a very long application display name", 28.0, 80.0),
            80.0
        );
        assert!(
            application_chip_text_width("powershell", 28.0, 126.0)
                > application_chip_text_width("chat", 28.0, 126.0)
        );
    }

    #[test]
    fn processor_enable_switches_stage_only_the_selected_profile() {
        let mut snapshot = dsp_snapshot();

        set_selected_dsp_enabled(&mut snapshot, DspSelection::Compressor, false);
        assert!(!snapshot.compressor.simple.enabled);
        assert!(snapshot.compressor.advanced.enabled);
        assert!(!selected_dsp_enabled(&snapshot, DspSelection::Compressor));

        snapshot.compressor.active_mode = DspMode::Advanced;
        assert!(selected_dsp_enabled(&snapshot, DspSelection::Compressor));

        set_selected_dsp_enabled(&mut snapshot, DspSelection::Expander, false);
        assert!(!snapshot.expander.advanced.enabled);
        assert!(snapshot.expander.simple.enabled);

        set_selected_dsp_enabled(&mut snapshot, DspSelection::NoiseSuppression, false);
        assert!(!snapshot.noise_suppression.enabled);

        set_selected_dsp_enabled(&mut snapshot, DspSelection::HeadphoneEqualizer, false);
        assert!(
            snapshot
                .headphone_equalizer
                .bands
                .iter()
                .all(|band| !band.enabled)
        );
    }

    #[test]
    fn equalizer_band_controls_stage_the_active_profile_without_flattening_it() {
        let mut snapshot = dsp_snapshot();
        snapshot.equalizer.active_mode = DspMode::Advanced;

        set_eq_band_type_value(&mut snapshot, 3, "high_pass").unwrap();
        adjust_eq_band_value(&mut snapshot, 3, "frequency", 1).unwrap();
        adjust_eq_band_value(&mut snapshot, 3, "gain", -1).unwrap();
        adjust_eq_band_value(&mut snapshot, 3, "q", 1).unwrap();

        let band = &active_eq_profile(&snapshot).bands[3];
        assert_eq!(band.band_type, EqualizerBandType::HighPass);
        assert_eq!(band.frequency_hz, 330.0);
        assert_eq!(band.gain_db, -0.5);
        assert!((band.q - 1.1).abs() < f32::EPSILON);
        assert_eq!(snapshot.equalizer.simple.bands[3].frequency_hz, 320.0);

        set_eq_band_value(&mut snapshot, 3, "frequency", 25_000.0).unwrap();
        set_eq_band_value(&mut snapshot, 3, "gain", -20.0).unwrap();
        set_eq_band_value(&mut snapshot, 3, "q", 0.0).unwrap();
        let band = &active_eq_profile(&snapshot).bands[3];
        assert_eq!(band.frequency_hz, 20_000.0);
        assert_eq!(band.gain_db, -12.0);
        assert_eq!(band.q, 0.1);

        let update = update_from_values(
            DspSelection::Equalizer,
            snapshot.clone(),
            12.0,
            -12.0,
            9.0,
            0.0,
            0.0,
        );
        let MicrophoneDspUpdate::Equalizer(equalizer) = update else {
            panic!("expected an equalizer update")
        };
        assert_eq!(equalizer, snapshot.equalizer);
    }

    #[test]
    fn equalizer_add_and_remove_reuse_a_disabled_validated_slot() {
        let mut snapshot = dsp_snapshot();
        remove_eq_band(&mut snapshot, 5).unwrap();
        let removed = &active_eq_profile(&snapshot).bands[5];
        assert!(!removed.enabled);
        assert_eq!(removed.band_type, EqualizerBandType::NotSet);

        assert_eq!(add_eq_band(&mut snapshot), Some(5));
        let restored = &active_eq_profile(&snapshot).bands[5];
        assert!(restored.enabled);
        assert_eq!(restored.band_type, EqualizerBandType::Bell);
        assert_eq!(add_eq_band(&mut snapshot), None);
    }

    #[test]
    fn enhancement_controls_stage_every_visible_beacn_value_and_enabled_state() {
        let mut snapshot = dsp_snapshot();

        set_enhancement_preset(&mut snapshot, 4).unwrap();
        set_enhancement_value(&mut snapshot, "bass_amount", 2.5).unwrap();
        set_enhancement_value(&mut snapshot, "de_esser_amount", 0.0).unwrap();
        set_enhancement_value(&mut snapshot, "exciter_amount", 42.0).unwrap();
        set_enhancement_value(&mut snapshot, "exciter_frequency", 3_100.0).unwrap();

        let suite = &snapshot.enhancement_suite;
        assert_eq!(suite.bass.preset, 4);
        assert_eq!(suite.bass.amount, 2.5);
        assert!(suite.bass.enabled);
        assert_eq!(suite.de_esser.amount_percent, 0.0);
        assert!(!suite.de_esser.enabled);
        assert_eq!(suite.exciter.amount_percent, 42.0);
        assert_eq!(suite.exciter.frequency_hz, 3_100.0);
        assert!(suite.exciter.enabled);

        let update = update_from_values(
            DspSelection::EnhancementSuite,
            snapshot.clone(),
            10.0,
            100.0,
            100.0,
            0.0,
            0.0,
        );
        let MicrophoneDspUpdate::EnhancementSuite(staged) = update else {
            panic!("expected an enhancement update")
        };
        assert_eq!(staged, snapshot.enhancement_suite);
    }

    #[test]
    fn enhancement_controls_reject_unknowns_and_clamp_protocol_ranges() {
        let mut snapshot = dsp_snapshot();

        assert!(set_enhancement_preset(&mut snapshot, 0).is_none());
        assert!(set_enhancement_preset(&mut snapshot, 5).is_none());
        assert!(set_enhancement_value(&mut snapshot, "unknown", 1.0).is_none());
        assert!(set_enhancement_value(&mut snapshot, "bass_amount", f32::NAN).is_none());

        set_enhancement_value(&mut snapshot, "bass_amount", 99.0).unwrap();
        set_enhancement_value(&mut snapshot, "de_esser_amount", -1.0).unwrap();
        set_enhancement_value(&mut snapshot, "exciter_frequency", 9_000.0).unwrap();
        assert_eq!(snapshot.enhancement_suite.bass.amount, 10.0);
        assert_eq!(snapshot.enhancement_suite.de_esser.amount_percent, 0.0);
        assert_eq!(snapshot.enhancement_suite.exciter.frequency_hz, 5_000.0);
    }

    #[test]
    fn headphone_controls_stage_exact_snapshot_values_without_apply_overwrite() {
        let mut snapshot = dsp_snapshot();

        set_headphone_eq_band_value(&mut snapshot, 0, 3.25).unwrap();
        set_headphone_eq_band_value(&mut snapshot, 1, -20.0).unwrap();
        set_headphone_subwoofer_value(&mut snapshot, 7.4).unwrap();
        assert_eq!(snapshot.headphone_equalizer.bands[0].amount_db, 3.25);
        assert_eq!(snapshot.headphone_equalizer.bands[1].amount_db, -12.0);
        assert_eq!(snapshot.headphone_equalizer.subwoofer.amount, 7);
        assert!(snapshot.headphone_equalizer.subwoofer.enabled);

        let update = update_from_values(
            DspSelection::HeadphoneEqualizer,
            snapshot.clone(),
            -12.0,
            12.0,
            -12.0,
            0.0,
            0.0,
        );
        let MicrophoneDspUpdate::HeadphoneEqualizer(staged) = update else {
            panic!("expected a headphone equalizer update")
        };
        assert_eq!(staged, snapshot.headphone_equalizer);

        set_headphone_subwoofer_value(&mut snapshot, 0.0).unwrap();
        assert_eq!(snapshot.headphone_equalizer.subwoofer.amount, 0);
        assert!(!snapshot.headphone_equalizer.subwoofer.enabled);
    }

    #[test]
    fn headphone_controls_reject_invalid_inputs_and_clamp_protocol_ranges() {
        let mut snapshot = dsp_snapshot();

        assert!(set_headphone_eq_band_value(&mut snapshot, -1, 0.0).is_none());
        assert!(set_headphone_eq_band_value(&mut snapshot, 3, 0.0).is_none());
        assert!(set_headphone_eq_band_value(&mut snapshot, 0, f32::NAN).is_none());
        assert!(set_headphone_subwoofer_value(&mut snapshot, f32::NAN).is_none());

        set_headphone_eq_band_value(&mut snapshot, 2, 99.0).unwrap();
        set_headphone_subwoofer_value(&mut snapshot, 99.0).unwrap();
        assert_eq!(snapshot.headphone_equalizer.bands[2].amount_db, 12.0);
        assert_eq!(snapshot.headphone_equalizer.subwoofer.amount, 10);
    }

    #[test]
    fn noise_sensitivity_uses_honest_db_units_and_protocol_range() {
        let snapshot = dsp_snapshot();
        let (_, labels, values, ranges) = dsp_display(&snapshot, DspSelection::NoiseSuppression);
        assert_eq!(labels.1, "Sensitivity dB");
        assert_eq!(values.1, -85.0);
        assert_eq!(ranges.1, (-120.0, -60.0));

        let ui = include_str!("../ui/app-window.slint");
        let start = ui
            .find("if root.dsp-selected-module == \"noise_suppression\"")
            .unwrap();
        let end = ui[start..]
            .find("if root.dsp-selected-module == \"expander\"")
            .map(|offset| start + offset)
            .unwrap();
        let noise_editor = &ui[start..end];
        assert!(noise_editor.contains("round(root.dsp-value-b) + \"dB\""));
        assert!(noise_editor.contains("sensitivity in decibels"));
        assert!(!noise_editor.contains("round(root.dsp-value-b) + \"%\""));
    }

    #[test]
    fn compressor_advanced_stages_ratio_attack_release_and_makeup_independently() {
        let mut snapshot = dsp_snapshot();
        snapshot.compressor.active_mode = DspMode::Advanced;
        let simple_before = snapshot.compressor.simple.clone();

        let update = update_from_values(
            DspSelection::Compressor,
            snapshot,
            -24.0,
            4.2,
            25.0,
            350.0,
            5.5,
        );
        let MicrophoneDspUpdate::Compressor(compressor) = update else {
            panic!("expected a compressor update")
        };
        assert_eq!(compressor.simple, simple_before);
        assert_eq!(compressor.advanced.threshold_db, -24.0);
        assert_eq!(compressor.advanced.ratio, 4.2);
        assert_eq!(compressor.advanced.attack_ms, 25.0);
        assert_eq!(compressor.advanced.release_ms, 350.0);
        assert_eq!(compressor.advanced.makeup_gain_db, 5.5);

        let snapshot = dsp_snapshot();
        let (_, labels, _, ranges) = dsp_display(&snapshot, DspSelection::Compressor);
        assert_eq!(
            labels,
            (
                "Threshold dB",
                "Ratio",
                "Attack ms",
                "Release ms",
                "Makeup gain dB"
            )
        );
        assert_eq!(ranges.0, (-50.0, 0.0));
        assert_eq!(ranges.1, (1.0, 16.0));
        assert_eq!(ranges.2, (1.0, 2_000.0));
        assert_eq!(ranges.3, (1.0, 2_000.0));
        assert_eq!(ranges.4, (0.0, 12.0));
    }

    #[test]
    fn compressor_simple_never_guesses_amount_to_ratio_mapping() {
        let snapshot = dsp_snapshot();
        let original = snapshot.compressor.simple.clone();
        let update = update_from_values(
            DspSelection::Compressor,
            snapshot,
            -22.0,
            15.0,
            999.0,
            1_999.0,
            4.5,
        );
        let MicrophoneDspUpdate::Compressor(compressor) = update else {
            panic!("expected a compressor update")
        };
        assert_eq!(compressor.simple.threshold_db, -22.0);
        assert_eq!(compressor.simple.makeup_gain_db, 4.5);
        assert_eq!(compressor.simple.ratio, original.ratio);
        assert_eq!(compressor.simple.attack_ms, original.attack_ms);
        assert_eq!(compressor.simple.release_ms, original.release_ms);

        let ui = include_str!("../ui/app-window.slint");
        assert!(ui.contains("Unavailable — no verified amount mapping"));
        assert!(ui.contains("label: \"Ratio\"; value-label:"));
        assert!(ui.contains("label: \"Attack\"; value-label:"));
        assert!(ui.contains("label: \"Release\"; value-label:"));
        assert!(ui.contains("root.apply-dsp(root.dsp-value-a, root.dsp-value-b, root.dsp-value-c, root.dsp-value-d, root.dsp-value-e)"));
    }

    #[test]
    fn profile_rail_reflow_contract_preserves_defaults_and_reclaims_space() {
        let ui = include_str!("../ui/app-window.slint");

        assert!(ui.contains("settings-content-width: root.profiles-drawer-open ? 684px : 865px;"));
        assert!(ui.contains("settings-left-padding: root.profiles-drawer-open ? 44px : 55px;"));
        assert!(ui.contains("padding-right: root.profiles-drawer-open ? 34px : 45px;"));
        assert!(ui.contains("if root.active-page == 4: settings-scroll := ScrollView"));
        assert!(ui.contains("width: root.settings-content-width;"));

        assert!(ui.contains("width: 897px;\n                    horizontal-stretch: 1;"));
        assert!(ui.contains("x: 28px; y: 20px; width: parent.width - 33px;"));
        assert!(ui.contains("x: parent.width - 174px; y: 5px; width: 78px;"));
        assert!(ui.contains("width: max(755px, parent.width - 290px);"));
        assert!(ui.contains("x: parent.width - 229px; y: 95px;"));

        let conditional_stretch = "horizontal-stretch: root.profiles-drawer-open ? 0 : 1;";
        assert!(ui.matches(conditional_stretch).count() >= 2);
        assert!(ui.contains("min-width: 1120px;"));
        assert!(ui.contains("init => { root.width = 1402px; root.height = 778px; }"));
        assert!(ui.contains("root.save-profile-drawer(!root.profiles-drawer-open);"));
        assert!(ui.contains("popup-width: 232px;"));

        assert!(should_preserve_drawer_window_size(false, false));
        assert!(!should_preserve_drawer_window_size(true, false));
        assert!(!should_preserve_drawer_window_size(false, true));
    }

    #[test]
    fn daemon_failures_have_deterministic_category_specific_copy() {
        let cases = [
            (DaemonErrorKind::Offline, "offline", "DAEMON IS OFFLINE"),
            (DaemonErrorKind::Timeout, "timeout", "DID NOT RESPOND"),
            (
                DaemonErrorKind::HttpStatus(503),
                "http",
                "REJECTED THE REQUEST",
            ),
            (
                DaemonErrorKind::InvalidResponse,
                "invalid_response",
                "RESPONSE IS INVALID",
            ),
            (DaemonErrorKind::Transport, "transport", "CONNECTION FAILED"),
        ];

        for (kind, expected_kind, title_fragment) in cases {
            let status = daemon_status_for_kind(kind, "Refresh failed");
            assert_eq!(status.kind, expected_kind);
            assert!(status.title.contains(title_fragment));
            assert!(status.status.starts_with("Refresh failed:"));
            assert!(!status.detail.is_empty());
        }
        assert!(
            daemon_status_for_kind(DaemonErrorKind::HttpStatus(503), "Refresh failed")
                .status
                .ends_with("HTTP 503")
        );
    }

    #[test]
    fn offline_banner_is_non_blocking_and_unused_settings_are_honest() {
        let ui = include_str!("../ui/app-window.slint");
        let banner_start = ui
            .find("if !root.daemon-connected: Rectangle")
            .expect("offline banner should exist");
        let banner_end = ui[banner_start..]
            .find("if root.legal-dialog-open")
            .map(|offset| banner_start + offset)
            .expect("offline banner should end before legal dialog");
        let banner = &ui[banner_start..banner_end];
        assert!(banner.contains("READ-ONLY NAVIGATION REMAINS AVAILABLE"));
        assert!(banner.contains("Retry StudioBridge daemon connection"));
        assert!(!banner.contains("TouchArea { width: parent.width; height: parent.height; }"));

        assert!(ui.contains("Update channels are not implemented in this build."));
        assert!(ui.contains("This compatibility preference has no runtime consumer yet."));
        assert!(ui.contains("Profile-switch crossfade is not implemented."));
        assert!(ui.matches("interactive: false; checked: root.").count() >= 3);
        assert!(ui.contains("accessible-enabled: root.interactive;"));
        assert!(ui.contains("if !root.daemon-connected && root.active-page <= 1: TouchArea"));
        assert!(
            ui.contains("width: parent.width - 72px - (root.profiles-drawer-open ? 285px : 0px);")
        );
        assert!(ui.contains("interactive: root.connected;"));
        assert!(ui.contains("enabled: root.connected;"));
        assert!(ui.contains(
            "if root.active-page == 0: FocusScope {\n                enabled: root.daemon-connected;"
        ));
        assert!(ui.contains(
            "if root.active-page == 1: FocusScope {\n                enabled: root.daemon-connected;"
        ));
        assert!(ui.contains("enabled: root.daemon-connected;\n                        clicked =>"));
        assert!(ui.contains("if root.daemon-connected {\n                                let name = root.pending-profile-save;"));
        assert!(
            ui.matches("available: root.daemon-connected && root.source-name-available")
                .count()
                >= 13
        );
        assert!(
            ui.matches("selected(name) => { if root.daemon-connected")
                .count()
                >= 13
        );
    }

    #[test]
    fn accepted_mutations_only_report_request_acceptance_and_readback() {
        let status = mutation_refresh_status("Volume update", &Ok(()));
        assert_eq!(
            status.status,
            "Volume update request accepted; current state reloaded"
        );
        assert!(!status.alert);
        assert!(!status.status.contains("succeeded"));
        assert!(!status.status.contains("confirmed"));
    }

    #[test]
    fn stale_refresh_cannot_claim_or_consume_a_newer_mutation_status() {
        let coordinator = RefreshCoordinator::new();
        let older = coordinator.begin(Some(mutation_refresh_status(
            "Older volume update",
            &Ok(()),
        )));
        let newer = coordinator.begin(Some(mutation_refresh_status("Newer mute update", &Ok(()))));

        assert!(take_refresh_status(&coordinator, older).is_none());
        let status = take_refresh_status(&coordinator, newer)
            .expect("newest refresh should be current")
            .expect("newest mutation status should remain pending");
        assert!(status.status.starts_with("Newer mute update"));
    }

    #[test]
    fn newer_plain_refresh_carries_forward_an_unreported_mutation_failure() {
        let coordinator = RefreshCoordinator::new();
        let failed_mutation = coordinator.begin(Some(RefreshStatus {
            status: "Volume update was not confirmed: request timed out; result was ambiguous, so current state was reloaded".into(),
            alert: true,
            failure_context: "Volume update was not confirmed".into(),
            requires_profile_readback: false,
            preferred_default: None,
        }));
        let reconnect_refresh = coordinator.begin(None);

        assert!(take_refresh_status(&coordinator, failed_mutation).is_none());
        let status = take_refresh_status(&coordinator, reconnect_refresh)
            .expect("reconnect refresh should be current")
            .expect("mutation failure must survive until authoritative read-back");
        assert!(status.alert);
        assert!(status.status.contains("was not confirmed"));
        assert!(status.status.contains("ambiguous"));
    }

    #[test]
    fn mutation_status_is_consumed_once_by_the_current_authoritative_refresh() {
        let coordinator = RefreshCoordinator::new();
        let mutation = coordinator.begin(Some(mutation_refresh_status(
            "Output device update",
            &Ok(()),
        )));

        assert!(
            take_refresh_status(&coordinator, mutation)
                .expect("mutation refresh should be current")
                .is_some()
        );
        assert!(matches!(
            take_refresh_status(&coordinator, mutation),
            Some(None)
        ));

        let later_refresh = coordinator.begin(None);
        assert!(matches!(
            take_refresh_status(&coordinator, later_refresh),
            Some(None)
        ));
    }

    #[test]
    fn newer_refresh_cannot_begin_during_a_claimed_ui_application() {
        let coordinator = Arc::new(RefreshCoordinator::new());
        let current = coordinator.begin(Some(mutation_refresh_status("Volume update", &Ok(()))));
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let applying_coordinator = coordinator.clone();
        let apply = thread::spawn(move || {
            let mut ordering = applying_coordinator
                .lock_if_current(current)
                .expect("current refresh should claim the application lock");
            let status = ordering.pending_status.clone();
            entered_tx
                .send(())
                .expect("test receiver should remain open");
            release_rx
                .recv()
                .expect("test should release the simulated UI application");
            ordering.pending_status = None;
            status
        });

        entered_rx
            .recv()
            .expect("simulated UI application should start");
        let (begun_tx, begun_rx) = mpsc::channel();
        let beginning_coordinator = coordinator.clone();
        let begin = thread::spawn(move || {
            let generation = beginning_coordinator.begin(None);
            begun_tx
                .send(generation)
                .expect("test receiver should remain open");
        });

        assert!(
            begun_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a newer refresh must block until the current UI application is complete"
        );
        release_tx
            .send(())
            .expect("simulated UI application should remain open");
        let status = apply.join().expect("application thread should not panic");
        assert!(status.is_some());
        let newer = begun_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("newer refresh should begin after application completes");
        begin.join().expect("begin thread should not panic");
        assert!(matches!(
            take_refresh_status(&coordinator, newer),
            Some(None)
        ));
    }

    #[test]
    fn ordinary_mutation_status_survives_failed_readback_until_reconnect() {
        let coordinator = RefreshCoordinator::new();
        let failed = coordinator.begin(Some(mutation_refresh_status("Mute update", &Ok(()))));
        assert!(settle_refresh_status(&coordinator, failed, false, false, false).is_some());

        let reconnect = coordinator.begin(None);
        assert!(settle_refresh_status(&coordinator, reconnect, true, true, false).is_some());
        assert!(matches!(
            take_refresh_status(&coordinator, reconnect),
            Some(None)
        ));
    }

    #[test]
    fn profile_mutation_waits_for_profile_readback_after_reconnect() {
        let coordinator = RefreshCoordinator::new();
        let failed = coordinator.begin(Some(profile_mutation_refresh_status(
            "Mixer profile save",
            &Ok(()),
        )));
        assert!(settle_refresh_status(&coordinator, failed, true, true, false).is_some());

        let reconnect = coordinator.begin(None);
        assert!(settle_refresh_status(&coordinator, reconnect, true, true, true).is_some());
        assert!(matches!(
            take_refresh_status(&coordinator, reconnect),
            Some(None)
        ));
    }

    #[test]
    fn preferred_default_mismatch_is_authoritative_and_settles_without_persisting() {
        let snapshot = default_snapshot(None, Some("speakers"));
        assert_eq!(
            preferred_default_readback(true, Some(&snapshot), false, "headphones"),
            PreferredDefaultReadback::Reject
        );
        let coordinator = RefreshCoordinator::new();
        let mismatch = coordinator.begin(Some(default_mutation_refresh_status(
            "Default playback device update",
            &Ok(()),
            false,
            "headphones",
        )));
        let status = settle_refresh_status(&coordinator, mismatch, true, true, true)
            .expect("mismatch should still produce one terminal status");
        assert!(status.preferred_default.is_some());
        assert!(matches!(
            take_refresh_status(&coordinator, mismatch),
            Some(None)
        ));
    }

    #[test]
    fn preferred_default_survives_failed_health_then_persists_once_on_match() {
        let snapshot = default_snapshot(None, Some("headphones"));
        assert_eq!(
            preferred_default_readback(false, Some(&snapshot), false, "headphones"),
            PreferredDefaultReadback::Retain
        );
        assert_eq!(
            preferred_default_readback(true, None, false, "headphones"),
            PreferredDefaultReadback::Retain
        );
        let coordinator = RefreshCoordinator::new();
        let failed = coordinator.begin(Some(default_mutation_refresh_status(
            "Default playback device update",
            &Ok(()),
            false,
            "headphones",
        )));
        assert!(settle_refresh_status(&coordinator, failed, false, true, true).is_some());

        let reconnect = coordinator.begin(None);
        assert_eq!(
            preferred_default_readback(true, Some(&snapshot), false, "headphones"),
            PreferredDefaultReadback::Persist
        );
        assert!(settle_refresh_status(&coordinator, reconnect, true, true, true).is_some());
        assert!(matches!(
            take_refresh_status(&coordinator, reconnect),
            Some(None)
        ));
    }

    #[test]
    fn unreported_alert_is_not_replaced_by_a_newer_success() {
        let alert = RefreshStatus {
            status: "Volume update was not confirmed: request timed out".into(),
            alert: true,
            failure_context: "Volume update was not confirmed".into(),
            requires_profile_readback: false,
            preferred_default: None,
        };
        let success = profile_mutation_refresh_status("Mixer profile save", &Ok(()));
        let merged = merge_pending_refresh_status(alert, success);
        assert!(merged.alert);
        assert!(merged.status.contains("Volume update was not confirmed"));
        assert!(merged.requires_profile_readback);
    }

    #[test]
    fn monitor_result_started_before_a_refresh_cannot_overwrite_it() {
        let coordinator = RefreshCoordinator::new();
        let stale_success = coordinator
            .monitor_generation()
            .expect("idle coordinator should allow a monitor request");
        let failed_refresh = coordinator.begin(None);

        assert!(coordinator.lock_if_current(stale_success).is_none());
        let mut ordering = coordinator
            .lock_if_current(failed_refresh)
            .expect("failed full refresh should present authoritative offline state");
        ordering.last_applied_generation = failed_refresh;
        drop(ordering);

        let accepted_success = coordinator
            .monitor_generation()
            .expect("monitor should resume after failed refresh is presented");
        assert_eq!(accepted_success, failed_refresh);
        assert!(monitor_connection_recovered(true, false));
        assert!(!monitor_connection_recovered(true, true));
    }

    #[test]
    fn monitor_cannot_start_while_a_full_refresh_is_in_flight() {
        let coordinator = RefreshCoordinator::new();
        let refresh_generation = coordinator.begin(None);
        assert!(coordinator.monitor_generation().is_none());

        let mut ordering = coordinator
            .lock_if_current(refresh_generation)
            .expect("refresh should still be current");
        ordering.last_applied_generation = refresh_generation;
        drop(ordering);
        assert_eq!(coordinator.monitor_generation(), Some(refresh_generation));
    }

    #[test]
    fn window_upgrade_precedes_pending_status_access() {
        let source = include_str!("main.rs");
        let refresh = source
            .split("fn refresh_all_with_status")
            .nth(1)
            .expect("refresh implementation should exist");
        let upgrade = refresh
            .find("window.upgrade()")
            .expect("refresh should upgrade the window");
        let lock = refresh
            .find("REFRESH_COORDINATOR.lock_if_current")
            .expect("refresh should claim its generation");
        let pending = refresh
            .find("pending_status.clone()")
            .expect("refresh should clone pending status");
        assert!(upgrade < lock && lock < pending);
    }

    #[test]
    fn profile_mutations_require_profile_list_readback() {
        let status = profile_mutation_refresh_status("Mixer profile save", &Ok(()));
        assert!(status.requires_profile_readback);
        assert_eq!(status.preferred_default, None);
    }

    #[test]
    fn preferred_defaults_are_only_staged_for_matching_readback() {
        let status = default_mutation_refresh_status(
            "Default playback device update",
            &Ok(()),
            false,
            "headphones",
        );
        assert_eq!(status.preferred_default, Some((false, "headphones".into())));
        assert!(default_selection_matches(Some("headphones"), "headphones"));
        assert!(!default_selection_matches(Some("system"), "headphones"));
        assert!(!default_selection_matches(None, "headphones"));

        let source = include_str!("main.rs");
        let callback_start = source
            .find("window.on_set_default_input")
            .expect("default input callback should exist");
        let callback_end = source[callback_start..]
            .find("window.on_set_source_mute")
            .map(|offset| callback_start + offset)
            .expect("default callback block should end before mute callback");
        let callbacks = &source[callback_start..callback_end];
        assert!(!callbacks.contains("remember_preferred_default"));
        assert!(source.contains("default_readback_matches(snapshot, input, &requested_id)"));
    }

    #[test]
    fn daemon_not_ready_never_reuses_ready_banner_copy() {
        let status = daemon_not_ready_status("Refresh failed");
        assert_eq!(status.kind, "not_ready");
        assert!(status.title.contains("NOT READY"));
        assert!(!status.title.ends_with("IS READY"));
        assert!(status.status.contains("reported not ready"));
    }

    #[test]
    fn every_daemon_write_callback_has_a_central_ready_guard() {
        let source = include_str!("main.rs");
        for callback in [
            "on_set_volume",
            "on_set_target_volume",
            "on_set_target_mute",
            "on_set_target_device",
            "on_set_source_device",
            "on_set_link_output_assignment",
            "on_set_default_input",
            "on_set_default_output",
            "on_set_source_mute",
            "on_set_volume_linked",
            "on_set_route",
            "on_create_source",
            "on_remove_source",
            "on_reorder_source",
            "on_create_mixer_profile",
            "on_save_mixer_profile",
            "on_duplicate_mixer_profile",
            "on_load_mixer_profile",
            "on_delete_mixer_profile",
            "on_assign_linux_application",
            "on_assign_link_application",
            "on_disarm_dsp",
        ] {
            let start = source.find(callback).expect("callback should exist");
            let guarded_prefix = &source[start..(start + 420).min(source.len())];
            assert!(
                guarded_prefix.contains("daemon_write_allowed"),
                "{callback} must refuse writes while the daemon is unavailable"
            );
        }
        assert!(source.contains("fn arm_dsp("));
        assert!(source.contains("daemon_write_allowed(&window, \"DSP arm\")"));
        assert!(source.contains("daemon_write_allowed(&window, \"DSP change\")"));
        assert!(source.contains("DAEMON_READY.load(Ordering::SeqCst)\n                && preferences.automatic_default_reset"));
    }
}
