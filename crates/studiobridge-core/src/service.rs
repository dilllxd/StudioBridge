use crate::{
    AppSnapshot, BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel,
    MicrophoneDspSnapshot, MicrophoneDspUpdate, MicrophoneDspWriteResult, MicrophoneState, MixBus,
    MixerBackend, MixerSnapshot, MuteState, SetMicrophoneRequest, StudioBackend, StudioIdentity,
    StudioSnapshot,
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

const BACKEND_TIMEOUT: Duration = Duration::from_secs(5);
const DSP_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct StudioBridgeService {
    studio: Arc<dyn StudioBackend>,
    mixer: Arc<dyn MixerBackend>,
    profile_apply: Arc<tokio::sync::Mutex<()>>,
}

impl StudioBridgeService {
    pub fn new(studio: Arc<dyn StudioBackend>, mixer: Arc<dyn MixerBackend>) -> Self {
        Self {
            studio,
            mixer,
            profile_apply: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn snapshot(&self) -> BridgeResult<AppSnapshot> {
        let (studio, mixer) = tokio::join!(
            backend_call("BEACN Studio", self.studio.snapshot()),
            backend_call("PipeWeaver", self.mixer.snapshot())
        );
        let studio = studio.unwrap_or_else(disconnected_studio);
        let mixer = mixer.unwrap_or_else(disconnected_mixer);
        Ok(AppSnapshot { studio, mixer })
    }

    pub async fn set_microphone(&self, request: SetMicrophoneRequest) -> BridgeResult<()> {
        backend_call("BEACN Studio", self.studio.set_microphone(request)).await
    }

    pub async fn microphone_dsp_snapshot(&self) -> BridgeResult<MicrophoneDspSnapshot> {
        backend_call_with_timeout(
            "BEACN Studio DSP snapshot",
            DSP_SNAPSHOT_TIMEOUT,
            self.studio.microphone_dsp_snapshot(),
        )
        .await
    }

    pub async fn set_microphone_dsp(
        &self,
        update: MicrophoneDspUpdate,
    ) -> BridgeResult<MicrophoneDspWriteResult> {
        update.validate()?;
        let module = update.module();
        let snapshot = backend_call_with_timeout(
            "BEACN Studio DSP write and verification",
            DSP_SNAPSHOT_TIMEOUT,
            self.studio.set_microphone_dsp(update.clone()),
        )
        .await?;
        if !update.matches_snapshot(&snapshot) {
            return Err(BridgeError::Backend(format!(
                "{} read-back did not match the requested state",
                module.as_str()
            )));
        }
        Ok(MicrophoneDspWriteResult {
            module,
            verified: true,
            snapshot,
        })
    }

    pub async fn set_link_assignment(
        &self,
        application: &str,
        channel: LinkChannel,
    ) -> BridgeResult<()> {
        backend_call(
            "BEACN Studio",
            self.studio.set_link_assignment(application, channel),
        )
        .await
    }

    pub async fn set_application_route(
        &self,
        process: &str,
        application: &str,
        channel_id: Option<&str>,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer
                .set_application_route(process, application, channel_id),
        )
        .await
    }

    pub async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_volume(channel_id, mix, volume)).await
    }

    pub async fn set_target_volume(&self, target_id: &str, volume: u8) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_target_volume(target_id, volume),
        )
        .await
    }

    pub async fn set_target_mute(&self, target_id: &str, muted: bool) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_target_mute(target_id, muted)).await
    }

    pub async fn set_target_device(
        &self,
        target_id: &str,
        device_node_id: Option<u32>,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_target_device(target_id, device_node_id),
        )
        .await
    }

    pub async fn set_source_device(
        &self,
        channel_id: &str,
        device_node_id: Option<u32>,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_source_device(channel_id, device_node_id),
        )
        .await
    }

    pub async fn set_link_output_assignment(
        &self,
        output_node_id: u32,
        target_id: Option<&str>,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer
                .set_link_output_assignment(output_node_id, target_id),
        )
        .await
    }

    pub async fn set_default_input(&self, device_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_default_input(device_id)).await
    }

    pub async fn set_default_output(&self, device_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_default_output(device_id)).await
    }

    pub async fn set_volume_linked(&self, channel_id: &str, linked: bool) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_volume_linked(channel_id, linked),
        )
        .await
    }

    pub async fn set_mute(&self, channel_id: &str, state: MuteState) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_mute(channel_id, state)).await
    }

    pub async fn set_route(
        &self,
        source_id: &str,
        target_id: &str,
        enabled: bool,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_route(source_id, target_id, enabled),
        )
        .await
    }

    pub async fn create_source(&self, name: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.create_source(name)).await
    }

    pub async fn set_source_name(&self, source_id: &str, name: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_source_name(source_id, name)).await
    }

    pub async fn set_source_colour(&self, source_id: &str, colour: &str) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_source_colour(source_id, colour),
        )
        .await
    }

    pub async fn remove_source(&self, source_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.remove_source(source_id)).await
    }

    pub async fn set_source_order(&self, source_id: &str, position: usize) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_source_order(source_id, position),
        )
        .await
    }

    /// Applies a saved mixer-only profile without touching BEACN hardware.
    pub async fn apply_mixer_profile(&self, desired: &MixerSnapshot) -> BridgeResult<()> {
        // Cancellation while waiting for the lock removes this request from the
        // queue. Once acquired, transfer the owned guard into a shielded task so
        // cancellation cannot abandon a transaction which may have mutated state.
        let transaction = self.profile_apply.clone().lock_owned().await;
        let service = self.clone();
        let desired = desired.clone();
        tokio::spawn(async move {
            let _transaction = transaction;
            service.apply_mixer_profile_transaction(&desired).await
        })
        .await
        .map_err(|error| {
            BridgeError::Backend(format!("profile transaction task failed: {error}"))
        })?
    }

    async fn apply_mixer_profile_transaction(&self, desired: &MixerSnapshot) -> BridgeResult<()> {
        let before = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        validate_mixer_profile(desired, &before)?;

        let primary_error = match self.apply_mixer_profile_once(desired, &before).await {
            Ok(()) => match backend_call("PipeWeaver", self.mixer.snapshot()).await {
                Ok(applied) => match verify_mixer_profile(desired, &applied) {
                    Ok(()) => return Ok(()),
                    Err(error) => error,
                },
                Err(error) => error,
            },
            Err(error) => error,
        };

        let rollback_result = self.rollback_mixer_profile(&before).await;
        match rollback_result {
            Ok(()) => Err(BridgeError::Backend(format!(
                "profile apply failed; the previous mixer state was restored: {primary_error}"
            ))),
            Err(rollback_error) => Err(BridgeError::Backend(format!(
                "profile apply failed and rollback could not restore the previous mixer state; apply error: {primary_error}; rollback error: {rollback_error}"
            ))),
        }
    }

    async fn rollback_mixer_profile(&self, before: &MixerSnapshot) -> BridgeResult<()> {
        let partial = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        validate_mixer_profile(before, &partial)?;
        self.apply_mixer_profile_once(before, &partial).await?;
        let restored = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        verify_mixer_profile(before, &restored)
    }

    async fn apply_mixer_profile_once(
        &self,
        desired: &MixerSnapshot,
        current: &MixerSnapshot,
    ) -> BridgeResult<()> {
        // Remove sources which are not part of the desired profile before
        // renaming or creating sources, so stale names cannot block either.
        for channel in &current.channels {
            if channel.source_kind == crate::MixerSourceKind::Virtual
                && !desired
                    .channels
                    .iter()
                    .any(|desired| mixer_channels_match(desired, channel))
            {
                backend_call("PipeWeaver", self.mixer.remove_source(&channel.id)).await?;
            }
        }
        for channel in &desired.channels {
            if let Some(existing) = current
                .channels
                .iter()
                .find(|existing| mixer_channels_match(channel, existing))
                && existing.name != channel.name
            {
                self.set_source_name(&existing.id, &channel.name).await?;
            }
        }
        for channel in &desired.channels {
            if channel.source_kind == crate::MixerSourceKind::Virtual
                && !current
                    .channels
                    .iter()
                    .any(|item| mixer_channels_match(channel, item))
            {
                backend_call("PipeWeaver", self.mixer.create_source(&channel.name)).await?;
            }
        }

        let current = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        for (position, channel) in desired.channels.iter().enumerate() {
            let current_channel = current_channel_for_desired(channel, &current.channels)
                .ok_or_else(|| {
                    BridgeError::Backend(format!(
                        "profile source was not available after reconciliation: {}",
                        channel.name
                    ))
                })?;
            self.set_source_order(&current_channel.id, position).await?;
            if !mixer_colours_match(&current_channel.colour, &channel.colour) {
                self.set_source_colour(&current_channel.id, &channel.colour)
                    .await?;
            }
            if current_channel.personal_volume != channel.personal_volume {
                self.set_volume(
                    &current_channel.id,
                    MixBus::Personal,
                    channel.personal_volume,
                )
                .await?;
            }
            if current_channel.audience_volume != channel.audience_volume {
                self.set_volume(
                    &current_channel.id,
                    MixBus::Audience,
                    channel.audience_volume,
                )
                .await?;
            }
            if current_channel.volumes_linked != channel.volumes_linked {
                self.set_volume_linked(&current_channel.id, channel.volumes_linked)
                    .await?;
            }
            if current_channel.mute_state != channel.mute_state {
                self.set_mute(&current_channel.id, channel.mute_state)
                    .await?;
            }
            if channel.source_kind == crate::MixerSourceKind::Physical
                && !descriptors_match(&channel.attached_devices, &current_channel.attached_devices)
            {
                if channel.attached_devices.is_empty() {
                    self.set_source_device(&current_channel.id, None).await?;
                } else {
                    let input = current
                        .physical_inputs
                        .iter()
                        .find(|input| {
                            physical_device_matches(&channel.attached_devices[0], &input.descriptor)
                        })
                        .expect("physical input was validated before profile mutation");
                    self.set_source_device(&current_channel.id, Some(input.node_id))
                        .await?;
                }
            }
        }
        for target in &desired.targets {
            if let Some(current_target) = current.targets.iter().find(|item| item.id == target.id) {
                if current_target.volume != target.volume {
                    self.set_target_volume(&target.id, target.volume).await?;
                }
                if current_target.muted != target.muted {
                    self.set_target_mute(&target.id, target.muted).await?;
                }
                if descriptors_match(&target.attached_devices, &current_target.attached_devices) {
                    continue;
                }
                if target.attached_devices.is_empty() {
                    self.set_target_device(&target.id, None).await?;
                } else if let Some(output) = current.physical_outputs.iter().find(|output| {
                    crate::beacn_link_output_slot(&output.name, &output.descriptor).is_none()
                        && target.attached_devices.iter().any(|attached| {
                            crate::beacn_link_output_slot("", attached).is_none()
                                && physical_device_matches(attached, &output.descriptor)
                        })
                }) {
                    self.set_target_device(&target.id, Some(output.node_id))
                        .await?;
                } else if target
                    .attached_devices
                    .iter()
                    .all(|attached| crate::beacn_link_output_slot("", attached).is_some())
                {
                    // Clear non-Link attachments first. Fixed Link assignments are
                    // restored below without replacing a target's other outputs.
                    self.set_target_device(&target.id, None).await?;
                }
            }
        }
        let current = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        for link in &current.link_outputs {
            let desired_target = desired_link_target(desired, link.slot);
            if link.target_id.as_deref() != desired_target {
                self.set_link_output_assignment(link.node_id, desired_target)
                    .await?;
            }
        }

        let current = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        let current_routes = current
            .routes
            .iter()
            .map(|route| (route.source_id.as_str(), route.target_id.as_str()))
            .collect::<HashSet<_>>();
        let desired_routes = desired
            .routes
            .iter()
            .filter_map(|route| {
                let desired_channel = desired
                    .channels
                    .iter()
                    .find(|channel| channel.id == route.source_id)?;
                let current_channel =
                    current_channel_for_desired(desired_channel, &current.channels)?;
                Some((current_channel.id.as_str(), route.target_id.as_str()))
            })
            .collect::<HashSet<_>>();
        for source in &current.channels {
            for target in &current.targets {
                let was_enabled =
                    current_routes.contains(&(source.id.as_str(), target.id.as_str()));
                let should_enable =
                    desired_routes.contains(&(source.id.as_str(), target.id.as_str()));
                if was_enabled != should_enable {
                    self.set_route(&source.id, &target.id, should_enable)
                        .await?;
                }
            }
        }
        for application in &current.applications {
            let desired_channel = desired
                .applications
                .iter()
                .find(|item| item.process == application.process && item.name == application.name)
                .and_then(|item| item.channel_id.as_deref())
                .and_then(|desired_id| {
                    let desired_channel = desired
                        .channels
                        .iter()
                        .find(|channel| channel.id == desired_id)?;
                    current_channel_for_desired(desired_channel, &current.channels)
                        .map(|channel| channel.id.as_str())
                });
            if application.channel_id.as_deref() != desired_channel {
                self.set_application_route(
                    &application.process,
                    &application.name,
                    desired_channel,
                )
                .await?;
            }
        }
        if let Some(default_input) = desired.default_input.as_deref()
            && current.default_input.as_deref() != Some(default_input)
        {
            self.set_default_input(default_input).await?;
        }
        if let Some(default_output) = desired.default_output.as_deref()
            && current.default_output.as_deref() != Some(default_output)
        {
            self.set_default_output(default_output).await?;
        }
        Ok(())
    }
}

fn verify_mixer_profile(desired: &MixerSnapshot, actual: &MixerSnapshot) -> BridgeResult<()> {
    if desired.channels.len() != actual.channels.len() {
        return Err(profile_verification_error("source count did not match"));
    }
    for (position, desired_channel) in desired.channels.iter().enumerate() {
        let Some(actual_channel) = actual.channels.get(position) else {
            return Err(profile_verification_error("source order did not match"));
        };
        if !mixer_channels_match(desired_channel, actual_channel)
            || desired_channel.name != actual_channel.name
            || !mixer_colours_match(&desired_channel.colour, &actual_channel.colour)
            || desired_channel.personal_volume != actual_channel.personal_volume
            || desired_channel.audience_volume != actual_channel.audience_volume
            || desired_channel.mute_state != actual_channel.mute_state
            || desired_channel.volumes_linked != actual_channel.volumes_linked
            || !descriptors_match(
                &desired_channel.attached_devices,
                &actual_channel.attached_devices,
            )
        {
            return Err(profile_verification_error(format!(
                "source did not match at position {position}: {}",
                desired_channel.name
            )));
        }
    }

    for desired_target in &desired.targets {
        let Some(actual_target) = actual
            .targets
            .iter()
            .find(|target| target.id == desired_target.id)
        else {
            return Err(profile_verification_error(format!(
                "target was missing: {}",
                desired_target.id
            )));
        };
        if desired_target.name != actual_target.name
            || desired_target.mix != actual_target.mix
            || desired_target.volume != actual_target.volume
            || desired_target.muted != actual_target.muted
            || !descriptors_match(
                &desired_target.attached_devices,
                &actual_target.attached_devices,
            )
        {
            return Err(profile_verification_error(format!(
                "target did not match: {}",
                desired_target.name
            )));
        }
    }

    let desired_routes = desired
        .routes
        .iter()
        .filter_map(|route| {
            let desired_channel = desired
                .channels
                .iter()
                .find(|channel| channel.id == route.source_id)?;
            let actual_channel = current_channel_for_desired(desired_channel, &actual.channels)?;
            Some((actual_channel.id.clone(), route.target_id.clone()))
        })
        .collect::<HashSet<_>>();
    let actual_routes = actual
        .routes
        .iter()
        .map(|route| (route.source_id.clone(), route.target_id.clone()))
        .collect::<HashSet<_>>();
    if desired_routes != actual_routes {
        return Err(profile_verification_error("routing matrix did not match"));
    }

    for actual_application in &actual.applications {
        let desired_channel_id = desired
            .applications
            .iter()
            .find(|application| {
                application.process == actual_application.process
                    && application.name == actual_application.name
            })
            .and_then(|application| application.channel_id.as_deref())
            .and_then(|channel_id| {
                let desired_channel = desired
                    .channels
                    .iter()
                    .find(|channel| channel.id == channel_id)?;
                current_channel_for_desired(desired_channel, &actual.channels)
                    .map(|channel| channel.id.as_str())
            });
        if desired_channel_id != actual_application.channel_id.as_deref() {
            return Err(profile_verification_error(format!(
                "application route did not match: {}/{}",
                actual_application.process, actual_application.name
            )));
        }
    }

    if desired.default_input != actual.default_input {
        return Err(profile_verification_error("default input did not match"));
    }
    if desired.default_output != actual.default_output {
        return Err(profile_verification_error("default output did not match"));
    }
    for actual_link in &actual.link_outputs {
        if desired_link_target(desired, actual_link.slot) != actual_link.target_id.as_deref() {
            return Err(profile_verification_error(format!(
                "Link output {} did not match",
                actual_link.slot
            )));
        }
    }
    Ok(())
}

fn desired_link_target(desired: &MixerSnapshot, slot: u8) -> Option<&str> {
    match desired.link_outputs.iter().find(|link| link.slot == slot) {
        Some(link) => link.target_id.as_deref(),
        None => desired.targets.iter().find_map(|target| {
            target
                .attached_devices
                .iter()
                .any(|attached| crate::beacn_link_output_slot("", attached) == Some(slot))
                .then_some(target.id.as_str())
        }),
    }
}

fn descriptors_match(
    desired: &[crate::MixerPhysicalDeviceDescriptor],
    actual: &[crate::MixerPhysicalDeviceDescriptor],
) -> bool {
    desired.len() == actual.len()
        && desired.iter().all(|descriptor| {
            actual
                .iter()
                .any(|item| physical_device_matches(descriptor, item))
        })
}

fn mixer_colours_match(left: &str, right: &str) -> bool {
    fn normalized(colour: &str) -> Option<String> {
        let hex = colour.strip_prefix('#').unwrap_or(colour);
        match hex.len() {
            6 if hex.chars().all(|character| character.is_ascii_hexdigit()) => {
                Some(hex.to_ascii_lowercase())
            }
            3 if hex.chars().all(|character| character.is_ascii_hexdigit()) => Some(
                hex.chars()
                    .flat_map(|character| [character, character])
                    .collect::<String>()
                    .to_ascii_lowercase(),
            ),
            _ => None,
        }
    }
    normalized(left).is_some_and(|left| normalized(right).as_deref() == Some(left.as_str()))
}

fn profile_verification_error(message: impl Into<String>) -> BridgeError {
    BridgeError::Backend(format!("profile verification failed: {}", message.into()))
}

fn validate_mixer_profile(desired: &MixerSnapshot, current: &MixerSnapshot) -> BridgeResult<()> {
    let mut channel_ids = HashSet::new();
    let mut channel_names = HashSet::new();
    for channel in &desired.channels {
        if !channel_ids.insert(channel.id.as_str()) {
            return Err(BridgeError::InvalidValue(format!(
                "profile contains duplicate source id: {}",
                channel.id
            )));
        }
        if !channel_names.insert(channel.name.to_lowercase()) {
            return Err(BridgeError::InvalidValue(format!(
                "profile contains duplicate source name: {}",
                channel.name
            )));
        }
        if channel.name.trim().is_empty() || channel.name.chars().count() > 64 {
            return Err(BridgeError::InvalidValue(
                "profile source names must contain 1 to 64 characters".into(),
            ));
        }
        if !valid_mixer_colour(&channel.colour) {
            return Err(BridgeError::InvalidValue(format!(
                "profile source has an invalid colour: {}",
                channel.name
            )));
        }
        if channel.personal_volume > 100 || channel.audience_volume > 100 {
            return Err(BridgeError::InvalidValue(format!(
                "profile source volume is outside 0 to 100: {}",
                channel.name
            )));
        }
        if channel.source_kind == crate::MixerSourceKind::Physical {
            if !current
                .channels
                .iter()
                .any(|existing| mixer_channels_match(channel, existing))
            {
                return Err(BridgeError::InvalidValue(format!(
                    "profile physical source is unavailable: {}",
                    channel.name
                )));
            }
            if channel.attached_devices.len() > 1 {
                return Err(BridgeError::InvalidValue(format!(
                    "profile physical source has multiple inputs: {}",
                    channel.name
                )));
            }
            if let Some(attached) = channel.attached_devices.first()
                && !current
                    .physical_inputs
                    .iter()
                    .any(|input| physical_device_matches(attached, &input.descriptor))
            {
                return Err(BridgeError::InvalidValue(format!(
                    "profile physical input is unavailable: {}",
                    channel.name
                )));
            }
        }

        if let Some(existing) = current
            .channels
            .iter()
            .find(|existing| mixer_channels_match(channel, existing))
            && existing.name != channel.name
            && current.channels.iter().any(|other| {
                !mixer_channels_match(channel, other)
                    && other.name.eq_ignore_ascii_case(&channel.name)
                    && desired
                        .channels
                        .iter()
                        .any(|desired| mixer_channels_match(desired, other))
            })
        {
            return Err(BridgeError::InvalidValue(format!(
                "profile source rename would collide with another retained source: {}",
                channel.name
            )));
        }
    }
    for current_physical in current
        .channels
        .iter()
        .filter(|channel| channel.source_kind == crate::MixerSourceKind::Physical)
    {
        if !desired
            .channels
            .iter()
            .any(|channel| mixer_channels_match(channel, current_physical))
        {
            return Err(BridgeError::InvalidValue(format!(
                "profile omits required physical source: {}",
                current_physical.name
            )));
        }
    }

    let target_ids = current
        .targets
        .iter()
        .map(|target| target.id.as_str())
        .collect::<HashSet<_>>();
    let mut desired_target_ids = HashSet::new();
    for target in &desired.targets {
        if !desired_target_ids.insert(target.id.as_str()) {
            return Err(BridgeError::InvalidValue(format!(
                "profile contains duplicate target id: {}",
                target.id
            )));
        }
        if !target_ids.contains(target.id.as_str()) {
            return Err(BridgeError::InvalidValue(format!(
                "profile target is unavailable: {}",
                target.id
            )));
        }
        let current_target = current
            .targets
            .iter()
            .find(|item| item.id == target.id)
            .expect("target id was validated above");
        if target.name != current_target.name || target.mix != current_target.mix {
            return Err(BridgeError::InvalidValue(format!(
                "profile attempts to change fixed target identity: {}",
                target.id
            )));
        }
        if target.volume > 100 {
            return Err(BridgeError::InvalidValue(format!(
                "profile target volume is outside 0 to 100: {}",
                target.name
            )));
        }
        let physical = target
            .attached_devices
            .iter()
            .filter(|attached| crate::beacn_link_output_slot("", attached).is_none())
            .collect::<Vec<_>>();
        if physical.len() > 1 {
            return Err(BridgeError::InvalidValue(format!(
                "profile target has multiple non-Link outputs: {}",
                target.name
            )));
        }
        if let Some(attached) = physical.first()
            && !current.physical_outputs.iter().any(|output| {
                crate::beacn_link_output_slot(&output.name, &output.descriptor).is_none()
                    && physical_device_matches(attached, &output.descriptor)
            })
        {
            return Err(BridgeError::InvalidValue(format!(
                "profile physical output is unavailable: {}",
                target.name
            )));
        }
        for slot in target
            .attached_devices
            .iter()
            .filter_map(|attached| crate::beacn_link_output_slot("", attached))
        {
            if !current.link_outputs.iter().any(|link| link.slot == slot) {
                return Err(BridgeError::InvalidValue(format!(
                    "profile Link output is unavailable: {slot}"
                )));
            }
        }
    }

    for route in &desired.routes {
        if !channel_ids.contains(route.source_id.as_str())
            || !target_ids.contains(route.target_id.as_str())
        {
            return Err(BridgeError::InvalidValue(format!(
                "profile route references an unavailable endpoint: {} -> {}",
                route.source_id, route.target_id
            )));
        }
    }
    for application in &desired.applications {
        if let Some(channel_id) = application.channel_id.as_deref()
            && !channel_ids.contains(channel_id)
        {
            return Err(BridgeError::InvalidValue(format!(
                "profile application route references an unavailable source: {}/{}",
                application.process, application.name
            )));
        }
    }

    let current_link_slots = current
        .link_outputs
        .iter()
        .map(|link| link.slot)
        .collect::<HashSet<_>>();
    let mut desired_link_slots = HashSet::new();
    let mut descriptor_link_targets = HashMap::new();
    for target in &desired.targets {
        for slot in target
            .attached_devices
            .iter()
            .filter_map(|attached| crate::beacn_link_output_slot("", attached))
        {
            if descriptor_link_targets
                .insert(slot, target.id.as_str())
                .is_some()
            {
                return Err(BridgeError::InvalidValue(format!(
                    "profile assigns Link output {slot} to multiple targets"
                )));
            }
        }
    }
    for link in &desired.link_outputs {
        if !(1..=4).contains(&link.slot) || !desired_link_slots.insert(link.slot) {
            return Err(BridgeError::InvalidValue(format!(
                "profile contains an invalid or duplicate Link output slot: {}",
                link.slot
            )));
        }
        if !current_link_slots.contains(&link.slot) {
            return Err(BridgeError::InvalidValue(format!(
                "profile Link output is unavailable: {}",
                link.slot
            )));
        }
        if let Some(target_id) = link.target_id.as_deref()
            && !target_ids.contains(target_id)
        {
            return Err(BridgeError::InvalidValue(format!(
                "profile Link output references an unavailable target: {target_id}"
            )));
        }
        if descriptor_link_targets.get(&link.slot).copied() != link.target_id.as_deref() {
            return Err(BridgeError::InvalidValue(format!(
                "profile Link output {} disagrees with saved target attachments",
                link.slot
            )));
        }
    }

    if let Some(default_input) = desired.default_input.as_deref()
        && !current
            .default_inputs
            .iter()
            .any(|device| device.id == default_input)
    {
        return Err(BridgeError::InvalidValue(format!(
            "profile default input is unavailable: {default_input}"
        )));
    }
    if desired.default_input.is_none() && current.default_input.is_some() {
        return Err(BridgeError::InvalidValue(
            "profile would require clearing the default input, which PipeWeaver does not support"
                .into(),
        ));
    }
    if let Some(default_output) = desired.default_output.as_deref()
        && !current
            .default_outputs
            .iter()
            .any(|device| device.id == default_output)
    {
        return Err(BridgeError::InvalidValue(format!(
            "profile default output is unavailable: {default_output}"
        )));
    }
    if desired.default_output.is_none() && current.default_output.is_some() {
        return Err(BridgeError::InvalidValue(
            "profile would require clearing the default output, which PipeWeaver does not support"
                .into(),
        ));
    }
    Ok(())
}

fn valid_mixer_colour(colour: &str) -> bool {
    let hex = colour.strip_prefix('#').unwrap_or(colour);
    matches!(hex.len(), 3 | 6) && hex.chars().all(|character| character.is_ascii_hexdigit())
}

fn mixer_channels_match(left: &crate::MixerChannel, right: &crate::MixerChannel) -> bool {
    left.source_kind == right.source_kind
        && ((!left.meter_id.is_empty() && left.meter_id == right.meter_id)
            || left.id == right.id
            || (left.source_kind == crate::MixerSourceKind::Virtual
                && right.source_kind == crate::MixerSourceKind::Virtual
                && left.name.eq_ignore_ascii_case(&right.name)))
}

fn current_channel_for_desired<'a>(
    desired: &crate::MixerChannel,
    current: &'a [crate::MixerChannel],
) -> Option<&'a crate::MixerChannel> {
    current
        .iter()
        .find(|channel| mixer_channels_match(desired, channel))
}

async fn backend_call<T>(
    label: &str,
    operation: impl Future<Output = BridgeResult<T>>,
) -> BridgeResult<T> {
    backend_call_with_timeout(label, BACKEND_TIMEOUT, operation).await
}

async fn backend_call_with_timeout<T>(
    label: &str,
    timeout: Duration,
    operation: impl Future<Output = BridgeResult<T>>,
) -> BridgeResult<T> {
    tokio::time::timeout(timeout, operation)
        .await
        .map_err(|_| BridgeError::BackendUnavailable(format!("{label} timed out")))?
}

fn disconnected_studio(error: crate::BridgeError) -> StudioSnapshot {
    StudioSnapshot {
        identity: StudioIdentity {
            status: BackendStatus::Error,
            error: Some(error.to_string()),
            product: "BEACN Studio".into(),
            serial: None,
            firmware: None,
            usb_port: "USB1".into(),
            driverless_mode: false,
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
            output_mode: crate::HeadphoneOutputMode::LineLevel,
            mic_output_gain_tenths_db: 0,
        },
        linked_applications: Vec::new(),
    }
}

fn disconnected_mixer(error: crate::BridgeError) -> MixerSnapshot {
    MixerSnapshot {
        status: BackendStatus::Error,
        error: Some(error.to_string()),
        engine: "PipeWeaver".into(),
        channels: Vec::new(),
        targets: Vec::new(),
        routes: Vec::new(),
        applications: Vec::new(),
        default_input: None,
        default_output: None,
        default_inputs: Vec::new(),
        default_outputs: Vec::new(),
        physical_outputs: Vec::new(),
        physical_inputs: Vec::new(),
        link_outputs: Vec::new(),
    }
}

fn physical_device_matches(
    left: &crate::MixerPhysicalDeviceDescriptor,
    right: &crate::MixerPhysicalDeviceDescriptor,
) -> bool {
    match (&left.name, &right.name) {
        (Some(left), Some(right)) => left == right,
        _ => left.description.is_some() && left.description == right.description,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockMixerBackend, MockStudioBackend};
    use std::sync::Mutex as StdMutex;

    #[derive(Debug, Clone, Copy)]
    enum FaultMode {
        FailBefore,
        FailAfter,
        Ignore,
    }

    #[derive(Debug)]
    struct FaultPlan {
        operation: &'static str,
        mode: FaultMode,
        remaining: usize,
    }

    #[derive(Clone)]
    struct FaultInjectingMixer {
        inner: Arc<MockMixerBackend>,
        plan: Arc<StdMutex<Option<FaultPlan>>>,
        pause_operation: Arc<StdMutex<Option<&'static str>>>,
        pause_entered: Arc<tokio::sync::Semaphore>,
        pause_release: Arc<tokio::sync::Semaphore>,
    }

    impl Default for FaultInjectingMixer {
        fn default() -> Self {
            Self {
                inner: Arc::new(MockMixerBackend::default()),
                plan: Arc::default(),
                pause_operation: Arc::default(),
                pause_entered: Arc::new(tokio::sync::Semaphore::new(0)),
                pause_release: Arc::new(tokio::sync::Semaphore::new(0)),
            }
        }
    }

    impl FaultInjectingMixer {
        fn inject(&self, operation: &'static str, mode: FaultMode, remaining: usize) {
            *self.plan.lock().unwrap() = Some(FaultPlan {
                operation,
                mode,
                remaining,
            });
        }

        fn take_fault(&self, operation: &str) -> Option<FaultMode> {
            let mut plan = self.plan.lock().unwrap();
            let current = plan.as_mut()?;
            if current.operation != operation || current.remaining == 0 {
                return None;
            }
            if current.remaining != usize::MAX {
                current.remaining -= 1;
            }
            Some(current.mode)
        }

        fn pause_once(&self, operation: &'static str) {
            *self.pause_operation.lock().unwrap() = Some(operation);
        }

        async fn wait_until_paused(&self) {
            self.pause_entered.acquire().await.unwrap().forget();
        }

        fn release_pause(&self) {
            self.pause_release.add_permits(1);
        }

        async fn mutate(
            &self,
            operation: &'static str,
            mutation: impl Future<Output = BridgeResult<()>>,
        ) -> BridgeResult<()> {
            let should_pause = {
                let mut paused = self.pause_operation.lock().unwrap();
                if paused.as_deref() == Some(operation) {
                    paused.take();
                    true
                } else {
                    false
                }
            };
            if should_pause {
                self.pause_entered.add_permits(1);
                self.pause_release.acquire().await.unwrap().forget();
            }
            match self.take_fault(operation) {
                Some(FaultMode::FailBefore) => Err(injected_failure(operation)),
                Some(FaultMode::Ignore) => Ok(()),
                Some(FaultMode::FailAfter) => {
                    mutation.await?;
                    Err(injected_failure(operation))
                }
                None => mutation.await,
            }
        }
    }

    fn injected_failure(operation: &str) -> BridgeError {
        BridgeError::Backend(format!("injected {operation} failure"))
    }

    #[async_trait::async_trait]
    impl MixerBackend for FaultInjectingMixer {
        async fn snapshot(&self) -> BridgeResult<MixerSnapshot> {
            self.inner.snapshot().await
        }

        async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()> {
            self.mutate("set_volume", self.inner.set_volume(channel_id, mix, volume))
                .await
        }

        async fn set_target_volume(&self, target_id: &str, volume: u8) -> BridgeResult<()> {
            self.mutate(
                "set_target_volume",
                self.inner.set_target_volume(target_id, volume),
            )
            .await
        }

        async fn set_target_mute(&self, target_id: &str, muted: bool) -> BridgeResult<()> {
            self.mutate(
                "set_target_mute",
                self.inner.set_target_mute(target_id, muted),
            )
            .await
        }

        async fn set_target_device(
            &self,
            target_id: &str,
            device_node_id: Option<u32>,
        ) -> BridgeResult<()> {
            self.mutate(
                "set_target_device",
                self.inner.set_target_device(target_id, device_node_id),
            )
            .await
        }

        async fn set_source_device(
            &self,
            channel_id: &str,
            device_node_id: Option<u32>,
        ) -> BridgeResult<()> {
            self.mutate(
                "set_source_device",
                self.inner.set_source_device(channel_id, device_node_id),
            )
            .await
        }

        async fn set_link_output_assignment(
            &self,
            output_node_id: u32,
            target_id: Option<&str>,
        ) -> BridgeResult<()> {
            self.mutate(
                "set_link_output_assignment",
                self.inner
                    .set_link_output_assignment(output_node_id, target_id),
            )
            .await
        }

        async fn set_default_input(&self, device_id: &str) -> BridgeResult<()> {
            self.mutate("set_default_input", self.inner.set_default_input(device_id))
                .await
        }

        async fn set_default_output(&self, device_id: &str) -> BridgeResult<()> {
            self.mutate(
                "set_default_output",
                self.inner.set_default_output(device_id),
            )
            .await
        }

        async fn set_volume_linked(&self, channel_id: &str, linked: bool) -> BridgeResult<()> {
            self.mutate(
                "set_volume_linked",
                self.inner.set_volume_linked(channel_id, linked),
            )
            .await
        }

        async fn set_mute(&self, channel_id: &str, state: MuteState) -> BridgeResult<()> {
            self.mutate("set_mute", self.inner.set_mute(channel_id, state))
                .await
        }

        async fn set_route(
            &self,
            source_id: &str,
            target_id: &str,
            enabled: bool,
        ) -> BridgeResult<()> {
            self.mutate(
                "set_route",
                self.inner.set_route(source_id, target_id, enabled),
            )
            .await
        }

        async fn create_source(&self, name: &str) -> BridgeResult<()> {
            self.mutate("create_source", self.inner.create_source(name))
                .await
        }

        async fn set_source_name(&self, source_id: &str, name: &str) -> BridgeResult<()> {
            self.mutate(
                "set_source_name",
                self.inner.set_source_name(source_id, name),
            )
            .await
        }

        async fn set_source_colour(&self, source_id: &str, colour: &str) -> BridgeResult<()> {
            self.mutate(
                "set_source_colour",
                self.inner.set_source_colour(source_id, colour),
            )
            .await
        }

        async fn remove_source(&self, source_id: &str) -> BridgeResult<()> {
            self.mutate("remove_source", self.inner.remove_source(source_id))
                .await
        }

        async fn set_source_order(&self, source_id: &str, position: usize) -> BridgeResult<()> {
            self.mutate(
                "set_source_order",
                self.inner.set_source_order(source_id, position),
            )
            .await
        }

        async fn set_application_route(
            &self,
            process: &str,
            application: &str,
            channel_id: Option<&str>,
        ) -> BridgeResult<()> {
            self.mutate(
                "set_application_route",
                self.inner
                    .set_application_route(process, application, channel_id),
            )
            .await
        }
    }

    fn service() -> StudioBridgeService {
        StudioBridgeService::new(
            Arc::new(MockStudioBackend::default()),
            Arc::new(MockMixerBackend::default()),
        )
    }

    fn fault_service() -> (StudioBridgeService, FaultInjectingMixer) {
        let mixer = FaultInjectingMixer::default();
        (
            StudioBridgeService::new(
                Arc::new(MockStudioBackend::default()),
                Arc::new(mixer.clone()),
            ),
            mixer,
        )
    }

    fn changed_profile(mut profile: MixerSnapshot) -> MixerSnapshot {
        let game_position = profile
            .channels
            .iter()
            .position(|channel| channel.id == "game")
            .unwrap();
        let mut game = profile.channels.remove(game_position);
        game.name = "Games".into();
        game.colour = "#123456".into();
        game.personal_volume = 19;
        game.audience_volume = 23;
        game.volumes_linked = false;
        game.mute_state = MuteState::MutedPersonal;
        profile.channels.insert(0, game);

        let replacement_input = profile
            .physical_inputs
            .iter()
            .find(|device| device.node_id == 202)
            .unwrap()
            .descriptor
            .clone();
        profile
            .channels
            .iter_mut()
            .find(|channel| channel.id == "microphone")
            .unwrap()
            .attached_devices = vec![replacement_input];

        let replacement_output = profile
            .physical_outputs
            .iter()
            .find(|device| device.node_id == 103)
            .unwrap()
            .descriptor
            .clone();
        let link_two_output = profile
            .physical_outputs
            .iter()
            .find(|device| device.node_id == 104)
            .unwrap()
            .descriptor
            .clone();
        let headphones = profile
            .targets
            .iter_mut()
            .find(|target| target.id == "headphones")
            .unwrap();
        headphones.volume = 17;
        headphones.muted = true;
        headphones.attached_devices = vec![replacement_output];
        profile
            .targets
            .iter_mut()
            .find(|target| target.id == "audience-mix")
            .unwrap()
            .attached_devices = vec![link_two_output];
        profile
            .link_outputs
            .iter_mut()
            .find(|link| link.slot == 2)
            .unwrap()
            .target_id = Some("audience-mix".into());

        profile.routes = vec![
            crate::MixerRoute {
                source_id: "chat".into(),
                target_id: "vod-track".into(),
            },
            crate::MixerRoute {
                source_id: "system".into(),
                target_id: "headphones".into(),
            },
        ];
        profile
            .applications
            .iter_mut()
            .find(|application| application.process == "discord")
            .unwrap()
            .channel_id = Some("system".into());
        profile.default_input = Some("vod-track".into());
        profile.default_output = Some("system".into());
        profile
    }

    #[tokio::test]
    async fn mock_state_can_be_changed() {
        let service = service();
        service
            .set_volume("game", MixBus::Audience, 41)
            .await
            .unwrap();
        service
            .set_link_assignment("Game", LinkChannel::Link3)
            .await
            .unwrap();
        service.set_route("chat", "headphones", true).await.unwrap();
        service
            .set_application_route("discord", "Discord", Some("game"))
            .await
            .unwrap();
        service.set_volume_linked("game", false).await.unwrap();
        service.set_target_volume("vod-track", 83).await.unwrap();
        service.set_target_mute("vod-track", true).await.unwrap();
        service.set_default_input("vod-track").await.unwrap();
        service.set_default_output("system").await.unwrap();
        service
            .set_target_device("audience-mix", Some(104))
            .await
            .unwrap();

        let state = service.snapshot().await.unwrap();
        let game = state
            .mixer
            .channels
            .iter()
            .find(|channel| channel.id == "game")
            .expect("mock game channel should exist");
        assert_eq!(game.audience_volume, 41);
        assert_eq!(
            state.studio.linked_applications[0].channel,
            LinkChannel::Link3
        );
        assert!(!game.volumes_linked);
        assert_eq!(
            state.studio.linked_applications[1].channel,
            LinkChannel::Link1,
            "moving one Windows application must not displace another",
        );
        assert!(
            state
                .mixer
                .routes
                .iter()
                .any(|route| { route.source_id == "chat" && route.target_id == "headphones" })
        );
        assert_eq!(
            state.mixer.applications[0].channel_id.as_deref(),
            Some("game")
        );
        assert_eq!(
            state
                .mixer
                .targets
                .iter()
                .find(|target| target.id == "vod-track")
                .unwrap()
                .volume,
            83
        );
        assert!(
            state
                .mixer
                .targets
                .iter()
                .find(|target| target.id == "vod-track")
                .unwrap()
                .muted
        );
        assert!(
            state
                .mixer
                .targets
                .iter()
                .any(|target| target.id == "voice-chat-mic")
        );
        assert_eq!(state.mixer.default_input.as_deref(), Some("vod-track"));
        assert_eq!(state.mixer.default_output.as_deref(), Some("system"));
        assert_eq!(
            state
                .mixer
                .link_outputs
                .iter()
                .find(|link| link.slot == 2)
                .and_then(|link| link.target_id.as_deref()),
            Some("audience-mix")
        );
    }

    #[tokio::test]
    async fn invalid_gain_is_rejected() {
        let error = service()
            .set_microphone(SetMicrophoneRequest {
                gain_db: Some(70),
                phantom_power: None,
                muted: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("0 and 69"));
    }

    #[tokio::test]
    async fn mixer_sources_can_be_added_and_removed() {
        let service = service();
        service.create_source("Aux 1").await.unwrap();
        service.set_source_order("aux-1", 1).await.unwrap();
        let state = service.snapshot().await.unwrap();
        assert!(
            state
                .mixer
                .channels
                .iter()
                .any(|channel| channel.id == "aux-1" && channel.name == "Aux 1")
        );
        assert_eq!(state.mixer.channels[1].id, "aux-1");

        service.remove_source("aux-1").await.unwrap();
        let state = service.snapshot().await.unwrap();
        assert!(
            !state
                .mixer
                .channels
                .iter()
                .any(|channel| channel.id == "aux-1")
        );
    }

    #[tokio::test]
    async fn saved_mixer_profile_restores_mixer_only_state() {
        let service = service();
        let saved = service.snapshot().await.unwrap().mixer;
        service.set_source_name("game", "Games").await.unwrap();
        service.set_source_colour("game", "#123456").await.unwrap();
        service
            .set_volume("game", MixBus::Personal, 13)
            .await
            .unwrap();
        service
            .set_volume("game", MixBus::Audience, 12)
            .await
            .unwrap();
        service.set_volume_linked("game", false).await.unwrap();
        service
            .set_mute("game", MuteState::MutedPersonal)
            .await
            .unwrap();
        service.set_source_order("game", 0).await.unwrap();
        service.create_source("Aux 1").await.unwrap();
        service
            .set_route("game", "headphones", false)
            .await
            .unwrap();
        service.set_route("chat", "vod-track", true).await.unwrap();
        service
            .set_application_route("discord", "Discord", Some("system"))
            .await
            .unwrap();
        service.set_target_volume("headphones", 17).await.unwrap();
        service.set_target_mute("headphones", true).await.unwrap();
        service
            .set_target_device("headphones", Some(103))
            .await
            .unwrap();
        service
            .set_source_device("microphone", Some(202))
            .await
            .unwrap();
        service
            .set_link_output_assignment(102, Some("vod-track"))
            .await
            .unwrap();
        service
            .set_link_output_assignment(104, Some("audience-mix"))
            .await
            .unwrap();
        service.set_default_input("vod-track").await.unwrap();
        service.set_default_output("system").await.unwrap();

        service.apply_mixer_profile(&saved).await.unwrap();
        let restored = service.snapshot().await.unwrap().mixer;
        assert_eq!(restored.channels, saved.channels);
        assert_eq!(restored.targets, saved.targets);
        assert_eq!(
            restored
                .routes
                .iter()
                .map(|route| (&route.source_id, &route.target_id))
                .collect::<HashSet<_>>(),
            saved
                .routes
                .iter()
                .map(|route| (&route.source_id, &route.target_id))
                .collect::<HashSet<_>>()
        );
        assert_eq!(restored.applications, saved.applications);
        assert_eq!(restored.default_input, saved.default_input);
        assert_eq!(restored.default_output, saved.default_output);
        assert_eq!(restored.link_outputs, saved.link_outputs);
    }

    #[tokio::test]
    async fn invalid_profile_is_rejected_before_any_mixer_mutation() {
        let service = service();
        let before = service.snapshot().await.unwrap().mixer;
        let mut invalid = before.clone();
        invalid
            .channels
            .iter_mut()
            .find(|channel| channel.id == "microphone")
            .unwrap()
            .attached_devices = vec![crate::MixerPhysicalDeviceDescriptor {
            name: Some("alsa_input.missing".into()),
            description: Some("Missing input".into()),
        }];

        let error = service.apply_mixer_profile(&invalid).await.unwrap_err();
        assert!(error.to_string().contains("physical input is unavailable"));
        assert_eq!(service.snapshot().await.unwrap().mixer, before);
    }

    #[tokio::test]
    async fn profile_failures_at_multiple_phases_restore_the_previous_state() {
        for (operation, mode) in [
            ("set_source_name", FaultMode::FailAfter),
            ("set_source_device", FaultMode::FailAfter),
            ("set_target_device", FaultMode::FailBefore),
            ("set_link_output_assignment", FaultMode::FailAfter),
            ("set_route", FaultMode::FailAfter),
            ("set_application_route", FaultMode::FailAfter),
            ("set_default_output", FaultMode::FailAfter),
        ] {
            let (service, mixer) = fault_service();
            let before = service.snapshot().await.unwrap().mixer;
            let desired = changed_profile(before.clone());
            mixer.inject(operation, mode, 1);

            let error = service.apply_mixer_profile(&desired).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("previous mixer state was restored"),
                "unexpected error for {operation}: {error}"
            );
            let restored = service.snapshot().await.unwrap().mixer;
            verify_mixer_profile(&before, &restored)
                .unwrap_or_else(|error| panic!("rollback after {operation} failed: {error}"));
        }
    }

    #[tokio::test]
    async fn successful_but_ignored_mutation_is_detected_and_rolled_back() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let desired = changed_profile(before.clone());
        mixer.inject("set_target_volume", FaultMode::Ignore, 1);

        let error = service.apply_mixer_profile(&desired).await.unwrap_err();
        assert!(error.to_string().contains("profile verification failed"));
        assert!(
            error
                .to_string()
                .contains("previous mixer state was restored")
        );
        verify_mixer_profile(&before, &service.snapshot().await.unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn source_reconciliation_failures_restore_the_previous_source_set() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let mut with_aux = before.clone();
        let mut aux = with_aux
            .channels
            .iter()
            .find(|channel| channel.id == "system")
            .unwrap()
            .clone();
        aux.id = "aux-1".into();
        aux.meter_id = "mock-source-aux-1".into();
        aux.name = "Aux 1".into();
        aux.applications.clear();
        with_aux.channels.push(aux);
        mixer.inject("create_source", FaultMode::FailAfter, 1);

        let error = service.apply_mixer_profile(&with_aux).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("previous mixer state was restored")
        );
        verify_mixer_profile(&before, &service.snapshot().await.unwrap().mixer).unwrap();

        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let mut without_music = before.clone();
        without_music
            .channels
            .retain(|channel| channel.id != "music");
        without_music
            .routes
            .retain(|route| route.source_id != "music");
        without_music
            .applications
            .iter_mut()
            .filter(|application| application.channel_id.as_deref() == Some("music"))
            .for_each(|application| application.channel_id = None);
        mixer.inject("remove_source", FaultMode::FailAfter, 1);

        let error = service
            .apply_mixer_profile(&without_music)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("previous mixer state was restored")
        );
        verify_mixer_profile(&before, &service.snapshot().await.unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn rollback_failure_is_never_reported_as_a_clean_restore() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let desired = changed_profile(before);
        mixer.inject("set_volume", FaultMode::FailAfter, usize::MAX);

        let error = service.apply_mixer_profile(&desired).await.unwrap_err();
        assert!(error.to_string().contains("rollback could not restore"));
        assert!(error.to_string().contains("apply error"));
        assert!(error.to_string().contains("rollback error"));
    }

    #[tokio::test]
    async fn overlapping_profile_applies_are_serialized_without_blocking_direct_controls() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let first_profile = changed_profile(before.clone());
        let mut second_profile = changed_profile(before);
        second_profile
            .channels
            .iter_mut()
            .find(|channel| channel.id == "game")
            .unwrap()
            .personal_volume = 47;
        second_profile
            .targets
            .iter_mut()
            .find(|target| target.id == "headphones")
            .unwrap()
            .volume = 31;
        mixer.pause_once("set_source_name");

        let first_service = service.clone();
        let first =
            tokio::spawn(async move { first_service.apply_mixer_profile(&first_profile).await });
        mixer.wait_until_paused().await;

        let second_service = service.clone();
        let expected = second_profile.clone();
        let second =
            tokio::spawn(async move { second_service.apply_mixer_profile(&second_profile).await });
        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "second profile bypassed transaction lock"
        );

        tokio::time::timeout(
            Duration::from_millis(100),
            service.set_volume("chat", MixBus::Personal, 72),
        )
        .await
        .expect("ordinary mixer control was blocked by profile lock")
        .unwrap();
        assert_eq!(
            service
                .snapshot()
                .await
                .unwrap()
                .mixer
                .channels
                .iter()
                .find(|channel| channel.id == "game")
                .unwrap()
                .name,
            "Game",
            "waiting profile mutated state before the active transaction resumed"
        );

        mixer.release_pause();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        verify_mixer_profile(&expected, &service.snapshot().await.unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn caller_timeout_does_not_abandon_or_unlock_a_partial_profile_transaction() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let first_profile = changed_profile(before.clone());
        let mut second_profile = changed_profile(before);
        second_profile
            .channels
            .iter_mut()
            .find(|channel| channel.id == "game")
            .unwrap()
            .audience_volume = 49;
        let expected = second_profile.clone();
        mixer.pause_once("set_source_name");

        let timed_service = service.clone();
        let timed = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(100),
                timed_service.apply_mixer_profile(&first_profile),
            )
            .await
        });
        mixer.wait_until_paused().await;
        assert!(timed.await.unwrap().is_err(), "caller did not time out");

        let second_service = service.clone();
        let second =
            tokio::spawn(async move { second_service.apply_mixer_profile(&second_profile).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(
            !second.is_finished(),
            "caller cancellation released a still-active transaction lock"
        );

        mixer.release_pause();
        tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .expect("transaction lock was not released after detached apply completed")
            .unwrap()
            .unwrap();
        verify_mixer_profile(&expected, &service.snapshot().await.unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn canceled_profile_waiter_is_removed_without_applying_later() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let active_profile = changed_profile(before.clone());
        let expected = active_profile.clone();
        let mut canceled_profile = changed_profile(before);
        canceled_profile
            .channels
            .iter_mut()
            .find(|channel| channel.id == "game")
            .unwrap()
            .personal_volume = 58;
        mixer.pause_once("set_source_name");

        let active_service = service.clone();
        let active =
            tokio::spawn(async move { active_service.apply_mixer_profile(&active_profile).await });
        mixer.wait_until_paused().await;

        assert!(
            tokio::time::timeout(
                Duration::from_millis(30),
                service.apply_mixer_profile(&canceled_profile),
            )
            .await
            .is_err(),
            "queued profile unexpectedly acquired the transaction lock"
        );

        mixer.release_pause();
        active.await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        verify_mixer_profile(&expected, &service.snapshot().await.unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn profile_transaction_lock_is_released_after_verified_error_rollback() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let desired = changed_profile(before);
        mixer.inject("set_target_device", FaultMode::FailBefore, 1);

        let error = service.apply_mixer_profile(&desired).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("previous mixer state was restored")
        );
        tokio::time::timeout(
            Duration::from_secs(2),
            service.apply_mixer_profile(&desired),
        )
        .await
        .expect("profile lock was not released after error rollback")
        .unwrap();
    }

    #[tokio::test]
    async fn mock_dsp_snapshot_contains_complete_profiles() {
        let dsp = service().microphone_dsp_snapshot().await.unwrap();
        assert_eq!(dsp.equalizer.simple.bands.len(), 8);
        assert_eq!(dsp.equalizer.advanced.bands.len(), 8);
        assert_eq!(dsp.headphone_equalizer.bands.len(), 3);
        assert!(dsp.headphone_equalizer.subwoofer.amount <= 10);
    }

    #[tokio::test]
    async fn dsp_write_is_validated_and_read_back() {
        let service = service();
        let mut headphone_eq = service
            .microphone_dsp_snapshot()
            .await
            .unwrap()
            .headphone_equalizer;
        headphone_eq.bands[0].amount_db = 2.0;
        let result = service
            .set_microphone_dsp(MicrophoneDspUpdate::HeadphoneEqualizer(
                headphone_eq.clone(),
            ))
            .await
            .unwrap();
        assert!(result.verified);
        assert_eq!(result.module, crate::DspWriteModule::HeadphoneEqualizer);
        assert_eq!(result.snapshot.headphone_equalizer, headphone_eq);

        headphone_eq.bands[0].amount_db = 13.0;
        let error = service
            .set_microphone_dsp(MicrophoneDspUpdate::HeadphoneEqualizer(headphone_eq))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("between -12 and 12"));

        let mut headphone_eq = service
            .microphone_dsp_snapshot()
            .await
            .unwrap()
            .headphone_equalizer;
        headphone_eq.subwoofer.amount = 11;
        let error = service
            .set_microphone_dsp(MicrophoneDspUpdate::HeadphoneEqualizer(headphone_eq))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("subwoofer amount"));
    }

    #[tokio::test]
    async fn stalled_backend_calls_are_bounded() {
        let error = backend_call_with_timeout(
            "test backend",
            Duration::from_millis(5),
            std::future::pending::<BridgeResult<()>>(),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("test backend timed out"));
    }
}
