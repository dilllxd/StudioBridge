use async_trait::async_trait;
use pipeweaver_ipc::client::Client;
use pipeweaver_ipc::clients::web::web_client::WebClient;
use pipeweaver_ipc::commands::{
    APICommand, DaemonRequest, DaemonResponse, DaemonStatus, PWCommandResponse,
};
use pipeweaver_profile::{PhysicalSourceDevice, VirtualSourceDevice};
use pipeweaver_shared::{
    AppDefinition, DeviceType, Mix, MuteState as PwMuteState, MuteTarget, NodeType,
};
use std::collections::HashMap;
use studiobridge_core::{
    BackendStatus, BridgeError, BridgeResult, MixBus, MixerApplication, MixerBackend, MixerChannel,
    MixerDeviceChoice, MixerLinkOutputAssignment, MixerPhysicalDeviceChoice,
    MixerPhysicalDeviceDescriptor, MixerRoute, MixerSnapshot, MixerSourceKind, MixerTarget,
    MuteState, beacn_link_output_slot,
};
use tokio::sync::Mutex;
use ulid::Ulid;

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

    async fn set_target_volume(&self, target_id: &str, volume: u8) -> BridgeResult<()> {
        if volume > 100 {
            return Err(BridgeError::InvalidValue(
                "volume must be between 0 and 100".into(),
            ));
        }
        self.send(APICommand::SetVolumeByName(
            target_id.to_string(),
            None,
            volume,
        ))
        .await
    }

    async fn set_target_mute(&self, target_id: &str, muted: bool) -> BridgeResult<()> {
        self.send(APICommand::SetTargetMuteStatesByName(
            target_id.to_string(),
            target_mute_state(muted),
        ))
        .await
    }

    async fn set_target_device(
        &self,
        target_id: &str,
        device_node_id: Option<u32>,
    ) -> BridgeResult<()> {
        let status = self
            .client
            .lock()
            .await
            .get_status()
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;
        let snapshot = map_status(&status);
        let target = snapshot
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown target: {target_id}")))?;
        let selected = device_node_id
            .map(|node_id| {
                snapshot
                    .physical_outputs
                    .iter()
                    .find(|device| device.node_id == node_id)
                    .ok_or_else(|| {
                        BridgeError::InvalidValue(format!(
                            "unknown or unusable physical output node: {node_id}"
                        ))
                    })
            })
            .transpose()?;

        let matching_index = selected.and_then(|device| {
            target
                .attached_devices
                .iter()
                .position(|attached| physical_device_matches(attached, &device.descriptor))
        });
        if selected.is_some() && target.attached_devices.len() == 1 && matching_index == Some(0) {
            return Ok(());
        }
        if selected.is_none() && target.attached_devices.is_empty() {
            return Ok(());
        }

        if let Some(device) = selected {
            if let Some(keep_index) = matching_index {
                for index in (0..target.attached_devices.len()).rev() {
                    if index != keep_index {
                        self.send(APICommand::RemovePhysicalNodeByName(
                            target_id.to_owned(),
                            index,
                        ))
                        .await?;
                    }
                }
            } else {
                // Attach first so a failed attach leaves the current output path intact.
                self.send(APICommand::AttachPhysicalNodeByName(
                    target_id.to_owned(),
                    device.node_id,
                ))
                .await?;
                for index in (0..target.attached_devices.len()).rev() {
                    self.send(APICommand::RemovePhysicalNodeByName(
                        target_id.to_owned(),
                        index,
                    ))
                    .await?;
                }
            }
        } else {
            for index in (0..target.attached_devices.len()).rev() {
                self.send(APICommand::RemovePhysicalNodeByName(
                    target_id.to_owned(),
                    index,
                ))
                .await?;
            }
        }
        Ok(())
    }

    async fn attach_target_output(&self, target_id: &str, device_node_id: u32) -> BridgeResult<()> {
        let status = self
            .client
            .lock()
            .await
            .get_status()
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;
        let snapshot = map_status(&status);
        let target = snapshot
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown target: {target_id}")))?;
        let output = snapshot
            .physical_outputs
            .iter()
            .find(|output| output.node_id == device_node_id)
            .filter(|output| beacn_link_output_slot(&output.name, &output.descriptor).is_none())
            .ok_or_else(|| {
                BridgeError::InvalidValue(format!(
                    "unknown, unusable, or Link-reserved physical output node: {device_node_id}"
                ))
            })?;
        let count = target
            .attached_devices
            .iter()
            .filter(|attached| physical_device_matches(attached, &output.descriptor))
            .count();
        if count != 0 {
            return Err(BridgeError::InvalidValue(format!(
                "target output is already attached or ambiguous: {target_id}; found {count} matches"
            )));
        }
        self.send(APICommand::AttachPhysicalNodeByName(
            target_id.to_owned(),
            device_node_id,
        ))
        .await
    }

    async fn detach_target_output(
        &self,
        target_id: &str,
        descriptor: &MixerPhysicalDeviceDescriptor,
    ) -> BridgeResult<()> {
        let status = self
            .client
            .lock()
            .await
            .get_status()
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;
        let snapshot = map_status(&status);
        let target = snapshot
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown target: {target_id}")))?;
        let endpoints = snapshot
            .physical_outputs
            .iter()
            .filter(|output| physical_device_matches(&output.descriptor, descriptor))
            .filter(|output| beacn_link_output_slot(&output.name, &output.descriptor).is_none())
            .count();
        if endpoints != 1 {
            return Err(BridgeError::InvalidValue(format!(
                "target output descriptor must resolve to exactly one non-Link endpoint; found {endpoints}"
            )));
        }
        let matching = target
            .attached_devices
            .iter()
            .enumerate()
            .filter_map(|(index, attached)| {
                physical_device_matches(attached, descriptor).then_some(index)
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            return Err(BridgeError::InvalidValue(format!(
                "target output descriptor must match exactly one attachment: {target_id}; found {}",
                matching.len()
            )));
        }
        // PipeWeaver removes by attachment index rather than stable descriptor.
        // This preflight fails closed for missing/ambiguous descriptors, but an
        // external writer could still reorder attachments between this status
        // read and the command. That protocol-level index race remains an HIL
        // validation boundary; do not guess another index or remove broadly.
        self.send(APICommand::RemovePhysicalNodeByName(
            target_id.to_owned(),
            matching[0],
        ))
        .await
    }

    async fn set_source_device(
        &self,
        channel_id: &str,
        device_node_id: Option<u32>,
    ) -> BridgeResult<()> {
        let status = self
            .client
            .lock()
            .await
            .get_status()
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;
        let snapshot = map_status(&status);
        let channel = snapshot
            .channels
            .iter()
            .find(|channel| channel.id == channel_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown channel: {channel_id}")))?;
        if channel.source_kind != MixerSourceKind::Physical {
            return Err(BridgeError::InvalidValue(format!(
                "channel does not accept physical inputs: {channel_id}"
            )));
        }
        let selected = device_node_id
            .map(|node_id| {
                snapshot
                    .physical_inputs
                    .iter()
                    .find(|device| device.node_id == node_id)
                    .ok_or_else(|| {
                        BridgeError::InvalidValue(format!(
                            "unknown or unusable physical input node: {node_id}"
                        ))
                    })
            })
            .transpose()?;

        let matching_index = selected.and_then(|device| {
            channel
                .attached_devices
                .iter()
                .position(|attached| physical_device_matches(attached, &device.descriptor))
        });
        if selected.is_some() && channel.attached_devices.len() == 1 && matching_index == Some(0) {
            return Ok(());
        }
        if selected.is_none() && channel.attached_devices.is_empty() {
            return Ok(());
        }

        if let Some(device) = selected {
            if let Some(keep_index) = matching_index {
                for index in (0..channel.attached_devices.len()).rev() {
                    if index != keep_index {
                        self.send(APICommand::RemovePhysicalNodeByName(
                            channel_id.to_owned(),
                            index,
                        ))
                        .await?;
                    }
                }
            } else {
                // Attach first so a failed attach leaves the current input path intact.
                self.send(APICommand::AttachPhysicalNodeByName(
                    channel_id.to_owned(),
                    device.node_id,
                ))
                .await?;
                for index in (0..channel.attached_devices.len()).rev() {
                    self.send(APICommand::RemovePhysicalNodeByName(
                        channel_id.to_owned(),
                        index,
                    ))
                    .await?;
                }
            }
        } else {
            for index in (0..channel.attached_devices.len()).rev() {
                self.send(APICommand::RemovePhysicalNodeByName(
                    channel_id.to_owned(),
                    index,
                ))
                .await?;
            }
        }
        Ok(())
    }

    async fn set_link_output_assignment(
        &self,
        output_node_id: u32,
        target_id: Option<&str>,
    ) -> BridgeResult<()> {
        let status = self
            .client
            .lock()
            .await
            .get_status()
            .await
            .map_err(|error| BridgeError::BackendUnavailable(error.to_string()))?;
        let snapshot = map_status(&status);
        let output = snapshot
            .physical_outputs
            .iter()
            .find(|output| output.node_id == output_node_id)
            .filter(|output| beacn_link_output_slot(&output.name, &output.descriptor).is_some())
            .ok_or_else(|| {
                BridgeError::InvalidValue(format!(
                    "unknown or unusable BEACN Link output node: {output_node_id}"
                ))
            })?;
        let selected_target = target_id
            .map(|target_id| {
                snapshot
                    .targets
                    .iter()
                    .find(|target| target.id == target_id)
                    .ok_or_else(|| {
                        BridgeError::InvalidValue(format!("unknown target: {target_id}"))
                    })
            })
            .transpose()?;

        if let Some(target) = selected_target
            && !target
                .attached_devices
                .iter()
                .any(|attached| physical_device_matches(attached, &output.descriptor))
        {
            // Preserve the selected target's other outputs and the current Link path
            // until the replacement attachment has succeeded.
            self.send(APICommand::AttachPhysicalNodeByName(
                target.id.clone(),
                output.node_id,
            ))
            .await?;
        }

        for target in &snapshot.targets {
            let keep_first = selected_target.is_some_and(|selected| selected.id == target.id);
            let matching_indices = target
                .attached_devices
                .iter()
                .enumerate()
                .filter_map(|(index, attached)| {
                    physical_device_matches(attached, &output.descriptor).then_some(index)
                })
                .collect::<Vec<_>>();
            let matching_count = matching_indices.len();
            for (position, index) in matching_indices.into_iter().rev().enumerate() {
                if keep_first && position + 1 == matching_count {
                    continue;
                }
                self.send(APICommand::RemovePhysicalNodeByName(
                    target.id.clone(),
                    index,
                ))
                .await?;
            }
        }
        Ok(())
    }

    async fn set_default_input(&self, device_id: &str) -> BridgeResult<()> {
        self.send(APICommand::SetDefaultInput(parse_device_id(device_id)?))
            .await
    }

    async fn set_default_output(&self, device_id: &str) -> BridgeResult<()> {
        self.send(APICommand::SetDefaultOutput(parse_device_id(device_id)?))
            .await
    }

    async fn set_volume_linked(&self, channel_id: &str, linked: bool) -> BridgeResult<()> {
        self.send(APICommand::SetSourceVolumeLinkedByName(
            channel_id.to_owned(),
            linked,
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

    async fn create_source(&self, name: &str) -> BridgeResult<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(BridgeError::InvalidValue(
                "source name must contain 1 to 64 characters".into(),
            ));
        }
        self.send(APICommand::CreateNode(
            NodeType::VirtualSource,
            name.to_owned(),
        ))
        .await
    }

    async fn set_source_name(&self, source_id: &str, name: &str) -> BridgeResult<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(BridgeError::InvalidValue(
                "source name must contain 1 to 64 characters".into(),
            ));
        }
        self.send(APICommand::RenameNodeByName(
            source_id.to_owned(),
            name.to_owned(),
        ))
        .await
    }

    async fn set_source_colour(&self, source_id: &str, colour: &str) -> BridgeResult<()> {
        let colour = colour.parse().map_err(|_| {
            BridgeError::InvalidValue("source colour must be #RGB or #RRGGBB".into())
        })?;
        self.send(APICommand::SetNodeColourByName(
            source_id.to_owned(),
            colour,
        ))
        .await
    }

    async fn remove_source(&self, source_id: &str) -> BridgeResult<()> {
        self.send(APICommand::RemoveNodeByName(source_id.to_owned()))
            .await
    }

    async fn set_source_order(&self, source_id: &str, position: usize) -> BridgeResult<()> {
        let position = u8::try_from(position).map_err(|_| {
            BridgeError::InvalidValue("source position must be between 0 and 255".into())
        })?;
        self.send(APICommand::SetOrderByName(source_id.to_owned(), position))
            .await
    }

    async fn set_application_route(
        &self,
        process: &str,
        application: &str,
        channel_id: Option<&str>,
    ) -> BridgeResult<()> {
        let definition = AppDefinition {
            device_type: DeviceType::Source,
            process: process.to_owned(),
            name: application.to_owned(),
        };
        let command = match channel_id {
            Some(channel_id) => {
                APICommand::SetApplicationRouteByName(definition, channel_id.to_owned())
            }
            None => APICommand::ClearApplicationRoute(definition),
        };
        self.send(command).await
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
            meter_id: device.description.id.to_string(),
            name: device.description.name.clone(),
            mix: from_pipeweaver_mix(device.mix),
            volume: device.volume,
            muted: device.mute_state == PwMuteState::Muted,
            attached_devices: map_attached_devices(&device.attached_devices),
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
                    meter_id: device.description.id.to_string(),
                    name: device.description.name.clone(),
                    mix: from_pipeweaver_mix(device.mix),
                    volume: device.volume,
                    muted: device.mute_state == PwMuteState::Muted,
                    attached_devices: map_attached_devices(&device.attached_devices),
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
    let (default_inputs, default_outputs) = default_device_choices(status);
    let physical_outputs = physical_output_choices(status);
    let physical_inputs = physical_input_choices(status);
    let link_outputs = link_output_assignments(&targets, &physical_outputs);

    MixerSnapshot {
        status: BackendStatus::Connected,
        error: None,
        engine: "PipeWeaver".into(),
        channels,
        targets,
        routes,
        applications: mixer_applications(status),
        default_input: status.audio.defaults_id[DeviceType::Source].map(|id| id.to_string()),
        default_output: status.audio.defaults_id[DeviceType::Target].map(|id| id.to_string()),
        default_inputs,
        default_outputs,
        physical_outputs,
        physical_inputs,
        link_outputs,
        copy_outputs: Vec::new(),
    }
}

fn link_output_assignments(
    targets: &[MixerTarget],
    outputs: &[MixerPhysicalDeviceChoice],
) -> Vec<MixerLinkOutputAssignment> {
    let mut assignments = outputs
        .iter()
        .filter_map(|output| {
            let slot = beacn_link_output_slot(&output.name, &output.descriptor)?;
            let target_id = targets.iter().find_map(|target| {
                target
                    .attached_devices
                    .iter()
                    .any(|attached| physical_device_matches(attached, &output.descriptor))
                    .then(|| target.id.clone())
            });
            Some(MixerLinkOutputAssignment {
                slot,
                node_id: output.node_id,
                name: if slot == 1 {
                    "Link Out".into()
                } else {
                    format!("Link {slot} Out")
                },
                target_id,
            })
        })
        .collect::<Vec<_>>();
    assignments.sort_by_key(|assignment| assignment.slot);
    assignments.dedup_by_key(|assignment| assignment.slot);
    assignments
}

fn map_attached_devices(
    devices: &[pipeweaver_profile::PhysicalDeviceDescriptor],
) -> Vec<MixerPhysicalDeviceDescriptor> {
    devices
        .iter()
        .map(|device| MixerPhysicalDeviceDescriptor {
            name: device.name.clone(),
            description: device.description.clone(),
        })
        .collect()
}

fn physical_output_choices(status: &DaemonStatus) -> Vec<MixerPhysicalDeviceChoice> {
    let mut outputs = status.audio.devices[DeviceType::Target]
        .iter()
        .filter(|device| device.is_usable)
        .map(|device| MixerPhysicalDeviceChoice {
            node_id: device.node_id,
            name: device
                .description
                .clone()
                .or_else(|| device.name.clone())
                .unwrap_or_else(|| format!("Output node {}", device.node_id)),
            descriptor: MixerPhysicalDeviceDescriptor {
                name: device.name.clone(),
                description: device.description.clone(),
            },
        })
        .collect::<Vec<_>>();
    outputs.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    outputs.dedup_by_key(|device| device.node_id);
    outputs
}

fn physical_input_choices(status: &DaemonStatus) -> Vec<MixerPhysicalDeviceChoice> {
    let mut inputs = status.audio.devices[DeviceType::Source]
        .iter()
        .filter(|device| device.is_usable)
        .map(|device| MixerPhysicalDeviceChoice {
            node_id: device.node_id,
            name: device
                .description
                .clone()
                .or_else(|| device.name.clone())
                .unwrap_or_else(|| format!("Input node {}", device.node_id)),
            descriptor: MixerPhysicalDeviceDescriptor {
                name: device.name.clone(),
                description: device.description.clone(),
            },
        })
        .collect::<Vec<_>>();
    inputs.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    inputs.dedup_by_key(|device| device.node_id);
    inputs
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

fn parse_device_id(device_id: &str) -> BridgeResult<Ulid> {
    device_id.parse::<Ulid>().map_err(|_| {
        BridgeError::InvalidValue(format!("invalid PipeWeaver device id: {device_id}"))
    })
}

fn default_device_choices(
    status: &DaemonStatus,
) -> (Vec<MixerDeviceChoice>, Vec<MixerDeviceChoice>) {
    let profile = &status.audio.profile.devices;
    let mut inputs = Vec::new();
    let mut outputs = Vec::new();

    for device in &profile.targets.virtual_devices {
        push_device_choice(
            &mut inputs,
            device.description.id,
            device.description.name.clone(),
        );
    }
    for device in &profile.sources.physical_devices {
        push_device_choice(
            &mut inputs,
            device.description.id,
            device.description.name.clone(),
        );
    }
    for device in &status.audio.devices[DeviceType::Source] {
        if device.is_usable {
            push_device_choice(
                &mut inputs,
                device.id,
                device
                    .description
                    .clone()
                    .or_else(|| device.name.clone())
                    .unwrap_or_else(|| "Unnamed input".into()),
            );
        }
    }

    for device in &profile.sources.virtual_devices {
        push_device_choice(
            &mut outputs,
            device.description.id,
            device.description.name.clone(),
        );
    }
    for device in &profile.targets.physical_devices {
        push_device_choice(
            &mut outputs,
            device.description.id,
            device.description.name.clone(),
        );
    }
    for device in &status.audio.devices[DeviceType::Target] {
        if device.is_usable {
            push_device_choice(
                &mut outputs,
                device.id,
                device
                    .description
                    .clone()
                    .or_else(|| device.name.clone())
                    .unwrap_or_else(|| "Unnamed output".into()),
            );
        }
    }

    inputs.sort_by_key(|device| device.name.to_lowercase());
    outputs.sort_by_key(|device| device.name.to_lowercase());
    (inputs, outputs)
}

fn push_device_choice(choices: &mut Vec<MixerDeviceChoice>, id: Ulid, name: String) {
    let id = id.to_string();
    if !choices.iter().any(|choice| choice.id == id) {
        choices.push(MixerDeviceChoice { id, name });
    }
}

fn mixer_applications(status: &DaemonStatus) -> Vec<MixerApplication> {
    let sources = &status.audio.profile.devices.sources;
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
    let mut result = Vec::new();
    for (process, named_applications) in &status.audio.applications[DeviceType::Source] {
        for (name, instances) in named_applications {
            let title = instances
                .iter()
                .find_map(|application| application.title.clone());
            let channel_id = instances
                .iter()
                .find_map(|application| application.target_id)
                .and_then(|target| source_names.get(&target).cloned());
            result.push(MixerApplication {
                process: process.clone(),
                name: name.clone(),
                title,
                channel_id,
            });
        }
    }
    result.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.process.cmp(&right.process))
    });
    result
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
    let mut channel = make_channel(
        &device.description,
        device.volumes.volume[Mix::A],
        device.volumes.volume[Mix::B],
        &device.mute_states.mute_state,
        applications,
        MixerSourceKind::Physical,
        device.volumes.volumes_linked.is_some(),
    );
    channel.attached_devices = map_attached_devices(&device.attached_devices);
    channel
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
        MixerSourceKind::Virtual,
        device.volumes.volumes_linked.is_some(),
    )
}

fn make_channel(
    description: &pipeweaver_profile::DeviceDescription,
    personal_volume: u8,
    audience_volume: u8,
    mute_targets: &std::collections::HashSet<MuteTarget>,
    applications: &HashMap<String, Vec<String>>,
    source_kind: MixerSourceKind,
    volumes_linked: bool,
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
        meter_id: description.id.to_string(),
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
        source_kind,
        volumes_linked,
        attached_devices: Vec::new(),
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

    #[derive(Clone)]
    struct FakePipeweaver {
        requests: Arc<AsyncMutex<Vec<DaemonRequest>>>,
        status: Arc<AsyncMutex<DaemonStatus>>,
        fail_next_pipewire: Arc<AsyncMutex<bool>>,
    }

    impl Default for FakePipeweaver {
        fn default() -> Self {
            let mut status = DaemonStatus::default();
            status.audio.profile = pipeweaver_profile::Profile::base_settings();
            Self {
                requests: Arc::default(),
                status: Arc::new(AsyncMutex::new(status)),
                fail_next_pipewire: Arc::default(),
            }
        }
    }

    async fn fake_command(
        State(state): State<FakePipeweaver>,
        Json(request): Json<DaemonRequest>,
    ) -> Json<DaemonResponse> {
        state.requests.lock().await.push(request.clone());
        if matches!(request, DaemonRequest::GetStatus) {
            Json(DaemonResponse::Status(state.status.lock().await.clone()))
        } else if std::mem::take(&mut *state.fail_next_pipewire.lock().await) {
            Json(DaemonResponse::Pipewire(PWCommandResponse::Err(
                "injected command failure".into(),
            )))
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
        assert!(!snapshot.default_inputs.is_empty());
        assert!(!snapshot.default_outputs.is_empty());
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
        backend
            .set_application_route("discord", "Discord", Some("System"))
            .await
            .unwrap();
        backend
            .set_application_route("discord", "Discord", None)
            .await
            .unwrap();
        backend.set_volume_linked("System", false).await.unwrap();
        backend.set_target_volume("Headphones", 77).await.unwrap();
        backend.set_target_mute("Headphones", true).await.unwrap();
        let default_input = Ulid::new();
        let default_output = Ulid::new();
        backend
            .set_default_input(&default_input.to_string())
            .await
            .unwrap();
        backend
            .set_default_output(&default_output.to_string())
            .await
            .unwrap();
        backend.create_source("Aux 1").await.unwrap();
        backend.set_source_name("Aux 1", "Auxiliary").await.unwrap();
        backend
            .set_source_colour("Auxiliary", "#123456")
            .await
            .unwrap();
        backend.remove_source("Auxiliary").await.unwrap();

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
        assert!(matches!(requests.get(5), Some(DaemonRequest::Pipewire(
            APICommand::SetApplicationRouteByName(definition, target)
        )) if definition.device_type == DeviceType::Source
            && definition.process == "discord"
            && definition.name == "Discord"
            && target == "System"));
        assert!(matches!(requests.get(6), Some(DaemonRequest::Pipewire(
            APICommand::ClearApplicationRoute(definition)
        )) if definition.device_type == DeviceType::Source
            && definition.process == "discord"
            && definition.name == "Discord"));
        assert!(matches!(requests.get(7), Some(DaemonRequest::Pipewire(
            APICommand::SetSourceVolumeLinkedByName(name, false)
        )) if name == "System"));
        assert!(matches!(requests.get(8), Some(DaemonRequest::Pipewire(
            APICommand::SetVolumeByName(name, None, 77)
        )) if name == "Headphones"));
        assert!(matches!(requests.get(9), Some(DaemonRequest::Pipewire(
            APICommand::SetTargetMuteStatesByName(name, PwMuteState::Muted)
        )) if name == "Headphones"));
        assert!(matches!(requests.get(10), Some(DaemonRequest::Pipewire(
            APICommand::SetDefaultInput(id)
        )) if id == &default_input));
        assert!(matches!(requests.get(11), Some(DaemonRequest::Pipewire(
            APICommand::SetDefaultOutput(id)
        )) if id == &default_output));
        assert!(matches!(requests.get(12), Some(DaemonRequest::Pipewire(
            APICommand::CreateNode(NodeType::VirtualSource, name)
        )) if name == "Aux 1"));
        assert!(matches!(requests.get(13), Some(DaemonRequest::Pipewire(
            APICommand::RenameNodeByName(source, name)
        )) if source == "Aux 1" && name == "Auxiliary"));
        assert!(matches!(requests.get(14), Some(DaemonRequest::Pipewire(
            APICommand::SetNodeColourByName(source, colour)
        )) if source == "Auxiliary" && colour.red == 0x12 && colour.green == 0x34 && colour.blue == 0x56));
        assert!(matches!(requests.get(15), Some(DaemonRequest::Pipewire(
            APICommand::RemoveNodeByName(name)
        )) if name == "Auxiliary"));

        server.abort();
    }

    #[tokio::test]
    async fn target_device_replacement_attaches_before_removing_stale_outputs() {
        let (backend, fake, server) = fake_backend().await;
        let old = pipeweaver_profile::PhysicalDeviceDescriptor {
            name: Some("alsa_output.old".into()),
            description: Some("Old Output".into()),
        };
        {
            let mut status = fake.status.lock().await;
            status.audio.profile.devices.targets.physical_devices[0].attached_devices = vec![old];
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 42,
                    name: Some("alsa_output.new".into()),
                    description: Some("New Output".into()),
                    is_usable: true,
                    ..Default::default()
                },
            );
        }

        backend
            .set_target_device("Headphones", Some(42))
            .await
            .unwrap();

        let requests = fake.requests.lock().await;
        assert!(matches!(requests.first(), Some(DaemonRequest::GetStatus)));
        assert!(matches!(requests.get(1), Some(DaemonRequest::Pipewire(
            APICommand::AttachPhysicalNodeByName(name, 42)
        )) if name == "Headphones"));
        assert!(matches!(requests.get(2), Some(DaemonRequest::Pipewire(
            APICommand::RemovePhysicalNodeByName(name, 0)
        )) if name == "Headphones"));
        assert_eq!(requests.len(), 3);

        server.abort();
    }

    #[tokio::test]
    async fn failed_target_attachment_never_detaches_the_previous_output() {
        let (backend, fake, server) = fake_backend().await;
        {
            let mut status = fake.status.lock().await;
            status.audio.profile.devices.targets.physical_devices[0].attached_devices =
                vec![pipeweaver_profile::PhysicalDeviceDescriptor {
                    name: Some("alsa_output.old".into()),
                    description: Some("Old Output".into()),
                }];
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 42,
                    name: Some("alsa_output.new".into()),
                    description: Some("New Output".into()),
                    is_usable: true,
                    ..Default::default()
                },
            );
        }
        *fake.fail_next_pipewire.lock().await = true;

        let error = backend
            .set_target_device("Headphones", Some(42))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("injected command failure"));
        let requests = fake.requests.lock().await;
        assert!(matches!(
            requests.as_slice(),
            [
                DaemonRequest::GetStatus,
                DaemonRequest::Pipewire(APICommand::AttachPhysicalNodeByName(name, 42))
            ] if name == "Headphones"
        ));

        server.abort();
    }

    #[tokio::test]
    async fn target_device_selection_rejects_unusable_nodes_without_writes() {
        let (backend, fake, server) = fake_backend().await;
        fake.status.lock().await.audio.devices[DeviceType::Target].push(
            pipeweaver_ipc::commands::PhysicalDevice {
                node_id: 42,
                name: Some("alsa_output.unusable".into()),
                description: Some("Unusable Output".into()),
                is_usable: false,
                ..Default::default()
            },
        );

        let error = backend
            .set_target_device("Headphones", Some(42))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown or unusable"));
        let requests = fake.requests.lock().await;
        assert!(matches!(requests.as_slice(), [DaemonRequest::GetStatus]));

        server.abort();
    }

    #[tokio::test]
    async fn source_device_replacement_attaches_before_removing_stale_inputs() {
        let (backend, fake, server) = fake_backend().await;
        let old = pipeweaver_profile::PhysicalDeviceDescriptor {
            name: Some("alsa_input.old".into()),
            description: Some("Old Input".into()),
        };
        {
            let mut status = fake.status.lock().await;
            status.audio.profile.devices.sources.physical_devices[0].attached_devices = vec![old];
            status.audio.devices[DeviceType::Source].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 84,
                    name: Some("alsa_input.new".into()),
                    description: Some("New Input".into()),
                    is_usable: true,
                    ..Default::default()
                },
            );
        }

        backend
            .set_source_device("Microphone", Some(84))
            .await
            .unwrap();

        let requests = fake.requests.lock().await;
        assert!(matches!(requests.first(), Some(DaemonRequest::GetStatus)));
        assert!(matches!(requests.get(1), Some(DaemonRequest::Pipewire(
            APICommand::AttachPhysicalNodeByName(name, 84)
        )) if name == "Microphone"));
        assert!(matches!(requests.get(2), Some(DaemonRequest::Pipewire(
            APICommand::RemovePhysicalNodeByName(name, 0)
        )) if name == "Microphone"));
        assert_eq!(requests.len(), 3);

        server.abort();
    }

    #[tokio::test]
    async fn failed_source_attachment_never_detaches_the_previous_input() {
        let (backend, fake, server) = fake_backend().await;
        {
            let mut status = fake.status.lock().await;
            status.audio.profile.devices.sources.physical_devices[0].attached_devices =
                vec![pipeweaver_profile::PhysicalDeviceDescriptor {
                    name: Some("alsa_input.old".into()),
                    description: Some("Old Input".into()),
                }];
            status.audio.devices[DeviceType::Source].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 84,
                    name: Some("alsa_input.new".into()),
                    description: Some("New Input".into()),
                    is_usable: true,
                    ..Default::default()
                },
            );
        }
        *fake.fail_next_pipewire.lock().await = true;

        let error = backend
            .set_source_device("Microphone", Some(84))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("injected command failure"));
        let requests = fake.requests.lock().await;
        assert!(matches!(
            requests.as_slice(),
            [
                DaemonRequest::GetStatus,
                DaemonRequest::Pipewire(APICommand::AttachPhysicalNodeByName(name, 84))
            ] if name == "Microphone"
        ));

        server.abort();
    }

    #[tokio::test]
    async fn source_device_selection_rejects_virtual_channels_and_unusable_nodes() {
        let (backend, fake, server) = fake_backend().await;
        fake.status.lock().await.audio.devices[DeviceType::Source].push(
            pipeweaver_ipc::commands::PhysicalDevice {
                node_id: 84,
                name: Some("alsa_input.unusable".into()),
                description: Some("Unusable Input".into()),
                is_usable: false,
                ..Default::default()
            },
        );

        let virtual_error = backend
            .set_source_device("System", Some(84))
            .await
            .unwrap_err();
        assert!(virtual_error.to_string().contains("does not accept"));
        let unusable_error = backend
            .set_source_device("Microphone", Some(84))
            .await
            .unwrap_err();
        assert!(unusable_error.to_string().contains("unknown or unusable"));
        let requests = fake.requests.lock().await;
        assert!(matches!(
            requests.as_slice(),
            [DaemonRequest::GetStatus, DaemonRequest::GetStatus]
        ));

        server.abort();
    }

    #[tokio::test]
    async fn link_output_assignment_attaches_new_target_before_detaching_old_target() {
        let (backend, fake, server) = fake_backend().await;
        let descriptor = pipeweaver_profile::PhysicalDeviceDescriptor {
            name: Some("alsa_output.usb-BEACN_Studio__Line4__sink".into()),
            description: Some("BEACN Studio Line4".into()),
        };
        let selected_target;
        {
            let mut status = fake.status.lock().await;
            status.audio.profile.devices.targets.physical_devices[0]
                .attached_devices
                .push(descriptor.clone());
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 77,
                    name: descriptor.name.clone(),
                    description: descriptor.description.clone(),
                    is_usable: true,
                    ..Default::default()
                },
            );
            selected_target = map_status(&status)
                .targets
                .iter()
                .find(|target| target.id != "Headphones")
                .expect("base profile should contain a second target")
                .id
                .clone();
        }

        backend
            .set_link_output_assignment(77, Some(&selected_target))
            .await
            .unwrap();

        let requests = fake.requests.lock().await;
        assert!(matches!(requests.first(), Some(DaemonRequest::GetStatus)));
        assert!(matches!(requests.get(1), Some(DaemonRequest::Pipewire(
            APICommand::AttachPhysicalNodeByName(name, 77)
        )) if name == &selected_target));
        assert!(matches!(requests.get(2), Some(DaemonRequest::Pipewire(
            APICommand::RemovePhysicalNodeByName(name, 0)
        )) if name == "Headphones"));
        assert_eq!(requests.len(), 3);

        server.abort();
    }

    #[tokio::test]
    async fn failed_link_attachment_never_detaches_the_previous_target() {
        let (backend, fake, server) = fake_backend().await;
        let descriptor = pipeweaver_profile::PhysicalDeviceDescriptor {
            name: Some("alsa_output.usb-BEACN_Studio__Line4__sink".into()),
            description: Some("BEACN Studio Line4".into()),
        };
        let selected_target;
        {
            let mut status = fake.status.lock().await;
            status.audio.profile.devices.targets.physical_devices[0]
                .attached_devices
                .push(descriptor.clone());
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 77,
                    name: descriptor.name.clone(),
                    description: descriptor.description.clone(),
                    is_usable: true,
                    ..Default::default()
                },
            );
            selected_target = map_status(&status)
                .targets
                .iter()
                .find(|target| target.id != "Headphones")
                .unwrap()
                .id
                .clone();
        }
        *fake.fail_next_pipewire.lock().await = true;

        let error = backend
            .set_link_output_assignment(77, Some(&selected_target))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("injected command failure"));
        let requests = fake.requests.lock().await;
        assert!(matches!(
            requests.as_slice(),
            [
                DaemonRequest::GetStatus,
                DaemonRequest::Pipewire(APICommand::AttachPhysicalNodeByName(name, 77))
            ] if name == &selected_target
        ));

        server.abort();
    }

    #[tokio::test]
    async fn additive_target_attach_never_removes_existing_outputs() {
        let (backend, fake, server) = fake_backend().await;
        let target_id;
        {
            let mut status = fake.status.lock().await;
            target_id = map_status(&status).targets[0].id.clone();
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 77,
                    name: Some("alsa_output.capture-card".into()),
                    description: Some("Capture Card".into()),
                    is_usable: true,
                    ..Default::default()
                },
            );
        }

        backend.attach_target_output(&target_id, 77).await.unwrap();
        let requests = fake.requests.lock().await;
        assert!(matches!(
            requests.as_slice(),
            [
                DaemonRequest::GetStatus,
                DaemonRequest::Pipewire(APICommand::AttachPhysicalNodeByName(name, 77))
            ] if name == &target_id
        ));
        server.abort();
    }

    #[tokio::test]
    async fn exact_target_detach_removes_only_the_matching_attachment() {
        let (backend, fake, server) = fake_backend().await;
        let selected = pipeweaver_profile::PhysicalDeviceDescriptor {
            name: Some("alsa_output.capture-card".into()),
            description: Some("Capture Card".into()),
        };
        let target_id;
        {
            let mut status = fake.status.lock().await;
            target_id = map_status(&status).targets[0].id.clone();
            status.audio.profile.devices.targets.physical_devices[0].attached_devices = vec![
                pipeweaver_profile::PhysicalDeviceDescriptor {
                    name: Some("alsa_output.primary".into()),
                    description: Some("Primary".into()),
                },
                selected.clone(),
                pipeweaver_profile::PhysicalDeviceDescriptor {
                    name: Some("alsa_output.unrelated".into()),
                    description: Some("Unrelated".into()),
                },
            ];
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 77,
                    name: selected.name.clone(),
                    description: selected.description.clone(),
                    is_usable: true,
                    ..Default::default()
                },
            );
        }
        let descriptor = MixerPhysicalDeviceDescriptor {
            name: selected.name,
            description: selected.description,
        };

        backend
            .detach_target_output(&target_id, &descriptor)
            .await
            .unwrap();
        let requests = fake.requests.lock().await;
        assert!(matches!(
            requests.as_slice(),
            [
                DaemonRequest::GetStatus,
                DaemonRequest::Pipewire(APICommand::RemovePhysicalNodeByName(name, 1))
            ] if name == &target_id
        ));
        server.abort();
    }

    #[tokio::test]
    async fn exact_target_detach_fails_closed_for_missing_or_ambiguous_attachments() {
        let (backend, fake, server) = fake_backend().await;
        let descriptor = pipeweaver_profile::PhysicalDeviceDescriptor {
            name: Some("alsa_output.capture-card".into()),
            description: Some("Capture Card".into()),
        };
        let target_id;
        {
            let mut status = fake.status.lock().await;
            target_id = map_status(&status).targets[0].id.clone();
            status.audio.devices[DeviceType::Target].push(
                pipeweaver_ipc::commands::PhysicalDevice {
                    node_id: 77,
                    name: descriptor.name.clone(),
                    description: descriptor.description.clone(),
                    is_usable: true,
                    ..Default::default()
                },
            );
            status.audio.profile.devices.targets.physical_devices[0]
                .attached_devices
                .clear();
        }
        let stable = MixerPhysicalDeviceDescriptor {
            name: descriptor.name.clone(),
            description: descriptor.description.clone(),
        };
        assert!(
            backend
                .detach_target_output(&target_id, &stable)
                .await
                .unwrap_err()
                .to_string()
                .contains("exactly one attachment")
        );
        {
            fake.status
                .lock()
                .await
                .audio
                .profile
                .devices
                .targets
                .physical_devices[0]
                .attached_devices = vec![descriptor.clone(), descriptor];
        }
        assert!(
            backend
                .detach_target_output(&target_id, &stable)
                .await
                .unwrap_err()
                .to_string()
                .contains("found 2")
        );
        assert!(
            fake.requests
                .lock()
                .await
                .iter()
                .all(|request| matches!(request, DaemonRequest::GetStatus))
        );
        server.abort();
    }

    #[tokio::test]
    async fn link_output_assignment_rejects_non_beacn_nodes_without_writes() {
        let (backend, fake, server) = fake_backend().await;
        fake.status.lock().await.audio.devices[DeviceType::Target].push(
            pipeweaver_ipc::commands::PhysicalDevice {
                node_id: 77,
                name: Some("alsa_output.interface_line4".into()),
                description: Some("Interface Line 4".into()),
                is_usable: true,
                ..Default::default()
            },
        );

        let error = backend
            .set_link_output_assignment(77, Some("Headphones"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("BEACN Link output"));
        let requests = fake.requests.lock().await;
        assert!(matches!(requests.as_slice(), [DaemonRequest::GetStatus]));

        server.abort();
    }
}
