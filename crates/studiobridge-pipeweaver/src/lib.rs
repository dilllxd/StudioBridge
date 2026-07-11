use async_trait::async_trait;
use pipeweaver_ipc::client::Client;
use pipeweaver_ipc::clients::web::web_client::WebClient;
use pipeweaver_ipc::commands::{
    APICommand, DaemonRequest, DaemonResponse, DaemonStatus, PWCommandResponse,
};
use pipeweaver_profile::{PhysicalSourceDevice, VirtualSourceDevice};
use pipeweaver_shared::{DeviceType, Mix, MuteState as PwMuteState, MuteTarget};
use std::collections::HashMap;
use studiobridge_core::{
    BackendStatus, BridgeError, BridgeResult, MixBus, MixerBackend, MixerChannel, MixerRoute,
    MixerSnapshot, MixerTarget, MuteState,
};
use tokio::sync::Mutex;

/// Adapter for an independently running PipeWeaver daemon.
///
/// It uses only PipeWeaver's public command API. StudioBridge does not embed or
/// modify PipeWeaver's PipeWire engine.
pub struct PipeweaverBackend {
    client: Mutex<WebClient>,
}

impl PipeweaverBackend {
    pub fn new(base_url: impl AsRef<str>) -> Self {
        let base = base_url.as_ref().trim_end_matches('/');
        Self {
            client: Mutex::new(WebClient::new(format!("{base}/api/command"))),
        }
    }

    async fn send(&self, command: APICommand) -> BridgeResult<()> {
        let response = self
            .client
            .lock()
            .await
            .send(&DaemonRequest::Pipewire(command))
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;

        match response {
            DaemonResponse::Ok | DaemonResponse::Pipewire(PWCommandResponse::Ok) => Ok(()),
            DaemonResponse::Err(error)
            | DaemonResponse::Pipewire(PWCommandResponse::Err(error)) => {
                Err(BridgeError::Backend(error))
            }
            other => Err(BridgeError::Backend(format!(
                "unexpected PipeWeaver response: {other:?}"
            ))),
        }
    }
}

#[async_trait]
impl MixerBackend for PipeweaverBackend {
    async fn snapshot(&self) -> BridgeResult<MixerSnapshot> {
        let status = self
            .client
            .lock()
            .await
            .get_status()
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;
        Ok(map_status(&status))
    }

    async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()> {
        if volume > 100 {
            return Err(BridgeError::InvalidValue(
                "volume must be between 0 and 100".into(),
            ));
        }
        self.send(APICommand::SetVolumeByName(
            channel_id.to_string(),
            Some(to_pipeweaver_mix(mix)),
            volume,
        ))
        .await
    }

    async fn set_mute(&self, channel_id: &str, state: MuteState) -> BridgeResult<()> {
        let (personal, audience) = mute_targets(state);
        self.set_mute_target(channel_id, MuteTarget::TargetA, personal)
            .await?;
        self.set_mute_target(channel_id, MuteTarget::TargetB, audience)
            .await
    }

    async fn set_route(&self, source_id: &str, target_id: &str, enabled: bool) -> BridgeResult<()> {
        self.send(APICommand::SetRouteByNames(
            source_id.to_string(),
            target_id.to_string(),
            enabled,
        ))
        .await
    }
}

impl PipeweaverBackend {
    async fn set_mute_target(
        &self,
        channel_id: &str,
        target: MuteTarget,
        muted: bool,
    ) -> BridgeResult<()> {
        let command = if muted {
            APICommand::AddSourceMuteTargetByName(channel_id.to_string(), target)
        } else {
            APICommand::DelSourceMuteTargetByName(channel_id.to_string(), target)
        };
        self.send(command).await
    }
}

fn map_status(status: &DaemonStatus) -> MixerSnapshot {
    let sources = &status.audio.profile.devices.sources;
    let applications = applications_by_target(status);
    let mut channels = Vec::new();

    let physical: HashMap<_, _> = sources
        .physical_devices
        .iter()
        .map(|device| (device.description.id, device))
        .collect();
    let virtuals: HashMap<_, _> = sources
        .virtual_devices
        .iter()
        .map(|device| (device.description.id, device))
        .collect();

    for group in [
        pipeweaver_shared::OrderGroup::Pinned,
        pipeweaver_shared::OrderGroup::Default,
    ] {
        for id in &sources.device_order[group] {
            if let Some(device) = physical.get(id) {
                channels.push(map_physical(device, &applications));
            } else if let Some(device) = virtuals.get(id) {
                channels.push(map_virtual(device, &applications));
            }
        }
    }

    // Profiles created by older PipeWeaver versions may not have a complete
    // order list. Include any remaining source exactly once.
    for device in &sources.physical_devices {
        if !channels
            .iter()
            .any(|channel| channel.id == device.description.name)
        {
            channels.push(map_physical(device, &applications));
        }
    }
    for device in &sources.virtual_devices {
        if !channels
            .iter()
            .any(|channel| channel.id == device.description.name)
        {
            channels.push(map_virtual(device, &applications));
        }
    }

    let targets = status
        .audio
        .profile
        .devices
        .targets
        .physical_devices
        .iter()
        .map(|device| MixerTarget {
            id: device.description.name.clone(),
            name: device.description.name.clone(),
            mix: from_pipeweaver_mix(device.mix),
            volume: device.volume,
            muted: device.mute_state == PwMuteState::Muted,
        })
        .chain(
            status
                .audio
                .profile
                .devices
                .targets
                .virtual_devices
                .iter()
                .map(|device| MixerTarget {
                    id: device.description.name.clone(),
                    name: device.description.name.clone(),
                    mix: from_pipeweaver_mix(device.mix),
                    volume: device.volume,
                    muted: device.mute_state == PwMuteState::Muted,
                }),
        )
        .collect::<Vec<_>>();

    let source_names: HashMap<_, _> = sources
        .physical_devices
        .iter()
        .map(|device| (device.description.id, device.description.name.clone()))
        .chain(
            sources
                .virtual_devices
                .iter()
                .map(|device| (device.description.id, device.description.name.clone())),
        )
        .collect();
    let target_names: HashMap<_, _> = status
        .audio
        .profile
        .devices
        .targets
        .physical_devices
        .iter()
        .map(|device| (device.description.id, device.description.name.clone()))
        .chain(
            status
                .audio
                .profile
                .devices
                .targets
                .virtual_devices
                .iter()
                .map(|device| (device.description.id, device.description.name.clone())),
        )
        .collect();
    let routes = status
        .audio
        .profile
        .routes
        .iter()
        .flat_map(|(source, targets)| {
            targets.iter().filter_map(|target| {
                Some(MixerRoute {
                    source_id: source_names.get(source)?.clone(),
                    target_id: target_names.get(target)?.clone(),
                })
            })
        })
        .collect();

    MixerSnapshot {
        status: BackendStatus::Connected,
        error: None,
        engine: "PipeWeaver".into(),
        channels,
        targets,
        routes,
    }
}

fn applications_by_target(status: &DaemonStatus) -> HashMap<String, Vec<String>> {
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    for process in status.audio.applications[DeviceType::Source].values() {
        for applications in process.values() {
            for application in applications {
                if let Some(target) = application.target_id {
                    result
                        .entry(target.to_string())
                        .or_default()
                        .push(application.name.clone());
                }
            }
        }
    }
    for names in result.values_mut() {
        names.sort();
        names.dedup();
    }
    result
}

fn map_physical(
    device: &PhysicalSourceDevice,
    applications: &HashMap<String, Vec<String>>,
) -> MixerChannel {
    make_channel(
        &device.description,
        device.volumes.volume[Mix::A],
        device.volumes.volume[Mix::B],
        &device.mute_states.mute_state,
        applications,
    )
}

fn map_virtual(
    device: &VirtualSourceDevice,
    applications: &HashMap<String, Vec<String>>,
) -> MixerChannel {
    make_channel(
        &device.description,
        device.volumes.volume[Mix::A],
        device.volumes.volume[Mix::B],
        &device.mute_states.mute_state,
        applications,
    )
}

fn make_channel(
    description: &pipeweaver_profile::DeviceDescription,
    personal_volume: u8,
    audience_volume: u8,
    mute_targets: &std::collections::HashSet<MuteTarget>,
    applications: &HashMap<String, Vec<String>>,
) -> MixerChannel {
    let personal_muted = mute_targets.contains(&MuteTarget::TargetA);
    let audience_muted = mute_targets.contains(&MuteTarget::TargetB);
    let mute_state = match (personal_muted, audience_muted) {
        (false, false) => MuteState::Unmuted,
        (true, true) => MuteState::MutedAll,
        (false, true) => MuteState::MutedAudience,
        (true, false) => MuteState::MutedPersonal,
    };

    MixerChannel {
        // Commands use PipeWeaver's stable, user-visible channel name.
        id: description.name.clone(),
        name: description.name.clone(),
        colour: format!(
            "#{:02x}{:02x}{:02x}",
            description.colour.red, description.colour.green, description.colour.blue
        ),
        personal_volume,
        audience_volume,
        mute_state,
        applications: applications
            .get(&description.id.to_string())
            .cloned()
            .unwrap_or_default(),
    }
}

fn to_pipeweaver_mix(mix: MixBus) -> Mix {
    match mix {
        MixBus::Personal => Mix::A,
        MixBus::Audience => Mix::B,
    }
}

fn from_pipeweaver_mix(mix: Mix) -> MixBus {
    match mix {
        Mix::A => MixBus::Personal,
        Mix::B => MixBus::Audience,
    }
}

fn mute_targets(state: MuteState) -> (bool, bool) {
    match state {
        MuteState::Unmuted => (false, false),
        MuteState::MutedAll => (true, true),
        MuteState::MutedAudience => (false, true),
        MuteState::MutedPersonal => (true, false),
    }
}

#[allow(dead_code)]
fn target_mute_state(muted: bool) -> PwMuteState {
    if muted {
        PwMuteState::Muted
    } else {
        PwMuteState::Unmuted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::Arc;
    use tokio::sync::Mutex as AsyncMutex;

    #[derive(Clone, Default)]
    struct FakePipeweaver {
        requests: Arc<AsyncMutex<Vec<DaemonRequest>>>,
    }

    async fn fake_command(
        State(state): State<FakePipeweaver>,
        Json(request): Json<DaemonRequest>,
    ) -> Json<DaemonResponse> {
        state.requests.lock().await.push(request.clone());
        if matches!(request, DaemonRequest::GetStatus) {
            let mut status = DaemonStatus::default();
            status.audio.profile = pipeweaver_profile::Profile::base_settings();
            Json(DaemonResponse::Status(status))
        } else {
            Json(DaemonResponse::Pipewire(PWCommandResponse::Ok))
        }
    }

    async fn fake_backend() -> (
        PipeweaverBackend,
        FakePipeweaver,
        tokio::task::JoinHandle<()>,
    ) {
        let state = FakePipeweaver::default();
        let app = Router::new()
            .route("/api/command", post(fake_command))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            PipeweaverBackend::new(format!("http://{address}")),
            state,
            server,
        )
    }

    #[test]
    fn mute_states_map_to_personal_and_audience_targets() {
        assert_eq!(mute_targets(MuteState::Unmuted), (false, false));
        assert_eq!(mute_targets(MuteState::MutedAll), (true, true));
        assert_eq!(mute_targets(MuteState::MutedAudience), (false, true));
        assert_eq!(mute_targets(MuteState::MutedPersonal), (true, false));
    }

    #[test]
    fn mix_a_is_personal_and_mix_b_is_audience() {
        assert_eq!(to_pipeweaver_mix(MixBus::Personal), Mix::A);
        assert_eq!(to_pipeweaver_mix(MixBus::Audience), Mix::B);
    }

    #[test]
    fn base_profile_maps_channels_targets_and_routes() {
        let mut status = DaemonStatus::default();
        status.audio.profile = pipeweaver_profile::Profile::base_settings();
        let snapshot = map_status(&status);
        assert_eq!(snapshot.channels.len(), 3);
        assert_eq!(snapshot.targets.len(), 2);
        assert_eq!(snapshot.routes.len(), 3);
        assert!(
            snapshot
                .targets
                .iter()
                .any(|target| target.name == "Headphones")
        );
    }

    #[tokio::test]
    async fn web_adapter_reads_status_and_sends_exact_commands() {
        let (backend, fake, server) = fake_backend().await;

        let snapshot = backend.snapshot().await.unwrap();
        assert_eq!(snapshot.channels.len(), 3);
        assert_eq!(snapshot.targets.len(), 2);
        assert_eq!(snapshot.routes.len(), 3);

        backend
            .set_volume("System", MixBus::Audience, 41)
            .await
            .unwrap();
        backend
            .set_route("System", "Headphones", false)
            .await
            .unwrap();
        backend
            .set_mute("System", MuteState::MutedAudience)
            .await
            .unwrap();

        let requests = fake.requests.lock().await;
        assert!(matches!(requests.get(1), Some(DaemonRequest::Pipewire(
            APICommand::SetVolumeByName(name, Some(Mix::B), 41)
        )) if name == "System"));
        assert!(matches!(requests.get(2), Some(DaemonRequest::Pipewire(
            APICommand::SetRouteByNames(source, target, false)
        )) if source == "System" && target == "Headphones"));
        assert!(matches!(requests.get(3), Some(DaemonRequest::Pipewire(
            APICommand::DelSourceMuteTargetByName(name, MuteTarget::TargetA)
        )) if name == "System"));
        assert!(matches!(requests.get(4), Some(DaemonRequest::Pipewire(
            APICommand::AddSourceMuteTargetByName(name, MuteTarget::TargetB)
        )) if name == "System"));

        server.abort();
    }
}
