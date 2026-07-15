use crate::{
    AppSnapshot, BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel,
    MicrophoneDspSnapshot, MicrophoneDspUpdate, MicrophoneDspWriteResult, MicrophoneState, MixBus,
    MixerBackend, MixerCopyOutputAssignment, MixerCopyOutputCommitEvidence,
    MixerCopyOutputWriteResult, MixerPhysicalDeviceDescriptor, MixerSnapshot, MuteState,
    SetMicrophoneRequest, SetMixerCopyOutputRequest, StudioBackend, StudioIdentity, StudioSnapshot,
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::Arc,
    time::Duration,
};

const BACKEND_TIMEOUT: Duration = Duration::from_secs(5);
const DSP_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);
const VOICE_CHAT_MIC_TARGET_ID: &str = "voice-chat-mic";

#[derive(Clone)]
pub struct StudioBridgeService {
    studio: Arc<dyn StudioBackend>,
    mixer: Arc<dyn MixerBackend>,
    mixer_mutation: Arc<tokio::sync::RwLock<()>>,
    copy_outputs: Arc<tokio::sync::RwLock<Vec<MixerCopyOutputAssignment>>>,
    copy_output_evidence: Arc<tokio::sync::RwLock<Option<MixerCopyOutputCommitEvidence>>>,
}

impl StudioBridgeService {
    pub fn new(studio: Arc<dyn StudioBackend>, mixer: Arc<dyn MixerBackend>) -> Self {
        Self::new_with_copy_outputs(studio, mixer, Vec::new())
            .expect("the built-in legacy copy-output seed is valid")
    }

    pub fn new_with_copy_outputs(
        studio: Arc<dyn StudioBackend>,
        mixer: Arc<dyn MixerBackend>,
        copy_outputs: Vec<MixerCopyOutputAssignment>,
    ) -> BridgeResult<Self> {
        Self::new_with_copy_output_state(studio, mixer, copy_outputs, None)
    }

    pub fn new_with_copy_output_state(
        studio: Arc<dyn StudioBackend>,
        mixer: Arc<dyn MixerBackend>,
        mut copy_outputs: Vec<MixerCopyOutputAssignment>,
        copy_output_evidence: Option<MixerCopyOutputCommitEvidence>,
    ) -> BridgeResult<Self> {
        if copy_outputs.is_empty() {
            copy_outputs.push(MixerCopyOutputAssignment {
                target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
                output: None,
            });
        }
        if copy_outputs.len() != 1 || copy_outputs[0].target_id != VOICE_CHAT_MIC_TARGET_ID {
            return Err(BridgeError::InvalidValue(format!(
                "live copy-output metadata must contain exactly one assignment for target: {VOICE_CHAT_MIC_TARGET_ID}"
            )));
        }
        if let Some(evidence) = copy_output_evidence.as_ref()
            && (evidence.version != 1
                || evidence.assignment.target_id != VOICE_CHAT_MIC_TARGET_ID
                || evidence.assignment != copy_outputs[0])
        {
            return Err(BridgeError::InvalidValue(
                "live copy-output commit evidence does not match its assignment".into(),
            ));
        }
        Ok(Self {
            studio,
            mixer,
            mixer_mutation: Arc::new(tokio::sync::RwLock::new(())),
            copy_outputs: Arc::new(tokio::sync::RwLock::new(copy_outputs)),
            copy_output_evidence: Arc::new(tokio::sync::RwLock::new(copy_output_evidence)),
        })
    }

    pub async fn copy_output_commit_evidence(&self) -> Option<MixerCopyOutputCommitEvidence> {
        self.copy_output_evidence.read().await.clone()
    }

    async fn mixer_snapshot_unlocked(&self) -> BridgeResult<MixerSnapshot> {
        let mut snapshot = backend_call("PipeWeaver", self.mixer.snapshot()).await?;
        snapshot.copy_outputs = self.copy_outputs.read().await.clone();
        Ok(snapshot)
    }

    async fn run_mixer_mutation<T, F, Fut>(&self, operation: F) -> BridgeResult<T>
    where
        T: Send + 'static,
        F: FnOnce(StudioBridgeService) -> Fut + Send + 'static,
        Fut: Future<Output = BridgeResult<T>> + Send + 'static,
    {
        // Cancellation while waiting for the lock removes this request from the
        // queue. Once acquired, move the owned guard into a detached-capable task:
        // dropping the caller can no longer abandon an in-flight backend write or
        // release the mutation gate before that write has reached a terminal result.
        let transaction = self.mixer_mutation.clone().write_owned().await;
        let service = self.clone();
        tokio::spawn(async move {
            let _transaction = transaction;
            operation(service).await
        })
        .await
        .map_err(|error| BridgeError::Backend(format!("mixer mutation task failed: {error}")))?
    }

    pub async fn snapshot(&self) -> BridgeResult<AppSnapshot> {
        let mixer = async {
            // Concurrent readers share this gate, but no reader may observe an
            // intermediate multi-step mutation or copy metadata that has not yet
            // committed alongside its physical attachment state. Keep the gate
            // scoped to the mixer read so a stalled Studio snapshot cannot delay
            // otherwise independent mixer controls.
            let _committed_view = self.mixer_mutation.clone().read_owned().await;
            self.mixer_snapshot_unlocked().await
        };
        let (studio, mixer) =
            tokio::join!(backend_call("BEACN Studio", self.studio.snapshot()), mixer);
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
        let process = process.to_owned();
        let application = application.to_owned();
        let channel_id = channel_id.map(str::to_owned);
        self.run_mixer_mutation(move |service| async move {
            service
                .set_application_route_unlocked(&process, &application, channel_id.as_deref())
                .await
        })
        .await
    }

    pub async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()> {
        let channel_id = channel_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_volume_unlocked(&channel_id, mix, volume).await
        })
        .await
    }

    pub async fn set_target_volume(&self, target_id: &str, volume: u8) -> BridgeResult<()> {
        let target_id = target_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_target_volume_unlocked(&target_id, volume).await
        })
        .await
    }

    pub async fn set_target_mute(&self, target_id: &str, muted: bool) -> BridgeResult<()> {
        let target_id = target_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_target_mute_unlocked(&target_id, muted).await
        })
        .await
    }

    pub async fn set_target_device(
        &self,
        target_id: &str,
        device_node_id: Option<u32>,
    ) -> BridgeResult<()> {
        if target_id == VOICE_CHAT_MIC_TARGET_ID {
            return Err(BridgeError::InvalidValue(
                "Voice Chat Mic outputs must be changed through the copy-output control".into(),
            ));
        }
        let target_id = target_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service
                .set_target_device_unlocked(&target_id, device_node_id)
                .await
        })
        .await
    }

    pub async fn set_mixer_copy_output(
        &self,
        request: SetMixerCopyOutputRequest,
    ) -> BridgeResult<MixerCopyOutputWriteResult> {
        self.run_mixer_mutation(move |service| async move {
            service.set_mixer_copy_output_unlocked(request).await
        })
        .await
    }

    pub async fn set_source_device(
        &self,
        channel_id: &str,
        device_node_id: Option<u32>,
    ) -> BridgeResult<()> {
        let channel_id = channel_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service
                .set_source_device_unlocked(&channel_id, device_node_id)
                .await
        })
        .await
    }

    pub async fn set_link_output_assignment(
        &self,
        output_node_id: u32,
        target_id: Option<&str>,
    ) -> BridgeResult<()> {
        let target_id = target_id.map(str::to_owned);
        self.run_mixer_mutation(move |service| async move {
            service
                .set_link_output_assignment_unlocked(output_node_id, target_id.as_deref())
                .await
        })
        .await
    }

    pub async fn set_default_input(&self, device_id: &str) -> BridgeResult<()> {
        let device_id = device_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_default_input_unlocked(&device_id).await
        })
        .await
    }

    pub async fn set_default_output(&self, device_id: &str) -> BridgeResult<()> {
        let device_id = device_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_default_output_unlocked(&device_id).await
        })
        .await
    }

    pub async fn set_volume_linked(&self, channel_id: &str, linked: bool) -> BridgeResult<()> {
        let channel_id = channel_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service
                .set_volume_linked_unlocked(&channel_id, linked)
                .await
        })
        .await
    }

    pub async fn set_mute(&self, channel_id: &str, state: MuteState) -> BridgeResult<()> {
        let channel_id = channel_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_mute_unlocked(&channel_id, state).await
        })
        .await
    }

    pub async fn set_route(
        &self,
        source_id: &str,
        target_id: &str,
        enabled: bool,
    ) -> BridgeResult<()> {
        let source_id = source_id.to_owned();
        let target_id = target_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service
                .set_route_unlocked(&source_id, &target_id, enabled)
                .await
        })
        .await
    }

    pub async fn create_source(&self, name: &str) -> BridgeResult<()> {
        let name = name.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.create_source_unlocked(&name).await
        })
        .await
    }

    pub async fn set_source_name(&self, source_id: &str, name: &str) -> BridgeResult<()> {
        let source_id = source_id.to_owned();
        let name = name.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.set_source_name_unlocked(&source_id, &name).await
        })
        .await
    }

    pub async fn set_source_colour(&self, source_id: &str, colour: &str) -> BridgeResult<()> {
        let source_id = source_id.to_owned();
        let colour = colour.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service
                .set_source_colour_unlocked(&source_id, &colour)
                .await
        })
        .await
    }

    pub async fn remove_source(&self, source_id: &str) -> BridgeResult<()> {
        let source_id = source_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service.remove_source_unlocked(&source_id).await
        })
        .await
    }

    pub async fn set_source_order(&self, source_id: &str, position: usize) -> BridgeResult<()> {
        let source_id = source_id.to_owned();
        self.run_mixer_mutation(move |service| async move {
            service
                .set_source_order_unlocked(&source_id, position)
                .await
        })
        .await
    }

    async fn set_application_route_unlocked(
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

    async fn set_volume_unlocked(
        &self,
        channel_id: &str,
        mix: MixBus,
        volume: u8,
    ) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_volume(channel_id, mix, volume)).await
    }

    async fn set_target_volume_unlocked(&self, target_id: &str, volume: u8) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_target_volume(target_id, volume),
        )
        .await
    }

    async fn set_target_mute_unlocked(&self, target_id: &str, muted: bool) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_target_mute(target_id, muted)).await
    }

    async fn set_target_device_unlocked(
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

    async fn attach_target_output_unlocked(
        &self,
        target_id: &str,
        device_node_id: u32,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.attach_target_output(target_id, device_node_id),
        )
        .await
    }

    async fn detach_target_output_unlocked(
        &self,
        target_id: &str,
        descriptor: &MixerPhysicalDeviceDescriptor,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.detach_target_output(target_id, descriptor),
        )
        .await
    }

    async fn set_source_device_unlocked(
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

    async fn set_link_output_assignment_unlocked(
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

    async fn set_default_input_unlocked(&self, device_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_default_input(device_id)).await
    }

    async fn set_default_output_unlocked(&self, device_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_default_output(device_id)).await
    }

    async fn set_volume_linked_unlocked(&self, channel_id: &str, linked: bool) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_volume_linked(channel_id, linked),
        )
        .await
    }

    async fn set_mute_unlocked(&self, channel_id: &str, state: MuteState) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_mute(channel_id, state)).await
    }

    async fn set_route_unlocked(
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

    async fn create_source_unlocked(&self, name: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.create_source(name)).await
    }

    async fn set_source_name_unlocked(&self, source_id: &str, name: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_source_name(source_id, name)).await
    }

    async fn set_source_colour_unlocked(&self, source_id: &str, colour: &str) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_source_colour(source_id, colour),
        )
        .await
    }

    async fn remove_source_unlocked(&self, source_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.remove_source(source_id)).await
    }

    async fn set_source_order_unlocked(
        &self,
        source_id: &str,
        position: usize,
    ) -> BridgeResult<()> {
        backend_call(
            "PipeWeaver",
            self.mixer.set_source_order(source_id, position),
        )
        .await
    }

    async fn set_mixer_copy_output_unlocked(
        &self,
        request: SetMixerCopyOutputRequest,
    ) -> BridgeResult<MixerCopyOutputWriteResult> {
        if request.target_id != VOICE_CHAT_MIC_TARGET_ID {
            return Err(BridgeError::InvalidValue(format!(
                "copy output is supported only for target: {VOICE_CHAT_MIC_TARGET_ID}"
            )));
        }
        let before = self.mixer_snapshot_unlocked().await?;
        let before_target = before
            .targets
            .iter()
            .find(|target| target.id == VOICE_CHAT_MIC_TARGET_ID)
            .ok_or_else(|| {
                BridgeError::InvalidValue(format!(
                    "copy-output target is unavailable: {VOICE_CHAT_MIC_TARGET_ID}"
                ))
            })?;
        let old = exact_copy_assignment(&before, VOICE_CHAT_MIC_TARGET_ID)?
            .and_then(|assignment| assignment.output.clone());
        if old.is_some() {
            let evidence = self
                .copy_output_evidence
                .read()
                .await
                .clone()
                .ok_or_else(|| {
                    BridgeError::InvalidValue(
                        "recovered copy-output ownership has no committed attachment evidence"
                            .into(),
                    )
                })?;
            if evidence.version != 1
                || evidence.assignment.target_id != VOICE_CHAT_MIC_TARGET_ID
                || !optional_descriptors_match(evidence.assignment.output.as_ref(), old.as_ref())
                || !descriptor_multisets_match(
                    &evidence.voice_chat_attachments,
                    &before_target.attached_devices,
                )
            {
                return Err(BridgeError::InvalidValue(
                    "recovered copy-output ownership does not match committed attachment evidence"
                        .into(),
                ));
            }
        }
        let selected = request
            .device_node_id
            .map(|node_id| {
                before
                    .physical_outputs
                    .iter()
                    .find(|output| output.node_id == node_id)
                    .filter(|output| {
                        crate::beacn_link_output_slot(&output.name, &output.descriptor).is_none()
                    })
                    .map(|output| output.descriptor.clone())
                    .ok_or_else(|| {
                        BridgeError::InvalidValue(format!(
                            "unknown, unusable, or Link-reserved physical output node: {node_id}"
                        ))
                    })
            })
            .transpose()?;

        if let Some(old) = old.as_ref() {
            let count = descriptor_count(&before_target.attached_devices, old);
            if count != 1 {
                return Err(BridgeError::InvalidValue(format!(
                    "tracked copy output must match exactly one Voice Chat attachment; found {count}"
                )));
            }
            let node_id = physical_output_node_id(&before, old)?;
            let output = before
                .physical_outputs
                .iter()
                .find(|output| output.node_id == node_id)
                .expect("the uniquely resolved physical output must still be present");
            if crate::beacn_link_output_slot(&output.name, &output.descriptor).is_some() {
                return Err(BridgeError::InvalidValue(
                    "tracked copy output resolves to a Link-reserved endpoint".into(),
                ));
            }
        }
        if let Some(selected) = selected.as_ref()
            && !old
                .as_ref()
                .is_some_and(|old| physical_device_matches(old, selected))
        {
            let count = descriptor_count(&before_target.attached_devices, selected);
            if count != 0 {
                return Err(BridgeError::InvalidValue(format!(
                    "selected output already belongs to another Voice Chat attachment role; found {count} matches"
                )));
            }
            physical_output_node_id(&before, selected)?;
        }

        if optional_descriptors_match(old.as_ref(), selected.as_ref()) {
            // The tracked descriptor is authoritative for this no-op. Returning
            // the freshly enumerated equivalent descriptor while leaving the
            // committed snapshot unchanged would make the result disagree with
            // its own snapshot when only descriptive metadata changed.
            let assignment = MixerCopyOutputAssignment {
                target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
                output: old,
            };
            let commit_evidence = copy_commit_evidence(&before, &assignment)?;
            *self.copy_output_evidence.write().await = Some(commit_evidence.clone());
            return Ok(MixerCopyOutputWriteResult {
                assignment,
                commit_evidence,
                verified: true,
                snapshot: before,
            });
        }

        let primary_result = self
            .apply_copy_output_attachments_unlocked(&before, old.as_ref(), selected.as_ref())
            .await;
        let mut applied = match primary_result {
            Ok(snapshot) => snapshot,
            Err(primary_error) => {
                return match self
                    .restore_copy_output_attachments_unlocked(
                        &before,
                        old.as_ref(),
                        selected.as_ref(),
                    )
                    .await
                {
                    Ok(()) => Err(BridgeError::Backend(format!(
                        "copy output change failed; the previous attachment state was restored: {primary_error}"
                    ))),
                    Err(rollback_error) => Err(BridgeError::Backend(format!(
                        "copy output change failed and rollback could not restore the previous attachment state; apply error: {primary_error}; rollback error: {rollback_error}"
                    ))),
                };
            }
        };

        let assignment = MixerCopyOutputAssignment {
            target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
            output: selected,
        };
        let commit_evidence = copy_commit_evidence(&applied, &assignment)?;
        *self.copy_outputs.write().await = vec![assignment.clone()];
        *self.copy_output_evidence.write().await = Some(commit_evidence.clone());
        applied.copy_outputs = vec![assignment.clone()];
        Ok(MixerCopyOutputWriteResult {
            assignment,
            commit_evidence,
            verified: true,
            snapshot: applied,
        })
    }

    async fn apply_copy_output_attachments_unlocked(
        &self,
        before: &MixerSnapshot,
        old: Option<&MixerPhysicalDeviceDescriptor>,
        selected: Option<&MixerPhysicalDeviceDescriptor>,
    ) -> BridgeResult<MixerSnapshot> {
        let before_target = mixer_target(before, VOICE_CHAT_MIC_TARGET_ID)?;
        if let Some(selected) = selected {
            let node_id = physical_output_node_id(before, selected)?;
            self.attach_target_output_unlocked(VOICE_CHAT_MIC_TARGET_ID, node_id)
                .await?;
            let attached = self.mixer_snapshot_unlocked().await?;
            let mut expected = before_target.attached_devices.clone();
            expected.push(selected.clone());
            verify_target_attachments(&attached, &expected, "new copy output was not attached")?;
        }
        if let Some(old) = old {
            self.detach_target_output_unlocked(VOICE_CHAT_MIC_TARGET_ID, old)
                .await?;
        }
        let applied = self.mixer_snapshot_unlocked().await?;
        let mut expected = before_target.attached_devices.clone();
        if let Some(selected) = selected {
            expected.push(selected.clone());
        }
        if let Some(old) = old {
            remove_exact_descriptor(&mut expected, old)?;
        }
        verify_target_attachments(&applied, &expected, "copy output read-back did not match")?;
        Ok(applied)
    }

    async fn restore_copy_output_attachments_unlocked(
        &self,
        before: &MixerSnapshot,
        old: Option<&MixerPhysicalDeviceDescriptor>,
        selected: Option<&MixerPhysicalDeviceDescriptor>,
    ) -> BridgeResult<()> {
        let before_target = mixer_target(before, VOICE_CHAT_MIC_TARGET_ID)?;
        let mut current = self.mixer_snapshot_unlocked().await?;
        if let Some(old) = old {
            let expected_count = descriptor_count(&before_target.attached_devices, old);
            let current_count = descriptor_count(
                &mixer_target(&current, VOICE_CHAT_MIC_TARGET_ID)?.attached_devices,
                old,
            );
            if current_count < expected_count {
                let node_id = physical_output_node_id(before, old)?;
                self.attach_target_output_unlocked(VOICE_CHAT_MIC_TARGET_ID, node_id)
                    .await?;
                current = self.mixer_snapshot_unlocked().await?;
            } else if current_count > expected_count {
                return Err(BridgeError::Backend(
                    "rollback found an ambiguous tracked copy attachment".into(),
                ));
            }
        }
        if let Some(selected) = selected
            && !old.is_some_and(|old| physical_device_matches(old, selected))
        {
            let before_count = descriptor_count(&before_target.attached_devices, selected);
            let current_count = descriptor_count(
                &mixer_target(&current, VOICE_CHAT_MIC_TARGET_ID)?.attached_devices,
                selected,
            );
            if current_count == before_count + 1 {
                self.detach_target_output_unlocked(VOICE_CHAT_MIC_TARGET_ID, selected)
                    .await?;
            } else if current_count != before_count {
                return Err(BridgeError::Backend(
                    "rollback found an ambiguous newly attached copy output".into(),
                ));
            }
        }
        let restored = self.mixer_snapshot_unlocked().await?;
        verify_target_attachments(
            &restored,
            &before_target.attached_devices,
            "rollback attachment read-back did not match",
        )
    }

    /// Applies a saved mixer-only profile without touching BEACN hardware.
    pub async fn apply_mixer_profile(&self, desired: &MixerSnapshot) -> BridgeResult<()> {
        let desired = desired.clone();
        self.run_mixer_mutation(move |service| async move {
            service.apply_mixer_profile_transaction(&desired).await
        })
        .await
    }

    async fn apply_mixer_profile_transaction(&self, desired: &MixerSnapshot) -> BridgeResult<()> {
        let before = self.mixer_snapshot_unlocked().await?;
        validate_mixer_profile(desired, &before)?;

        let primary_error = match self.apply_mixer_profile_once(desired, &before).await {
            Ok(()) => match self.mixer_snapshot_unlocked().await {
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
        let partial = self.mixer_snapshot_unlocked().await?;
        validate_mixer_profile(before, &partial)?;
        self.apply_mixer_profile_once(before, &partial).await?;
        let restored = self.mixer_snapshot_unlocked().await?;
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
                self.remove_source_unlocked(&channel.id).await?;
            }
        }
        for channel in &desired.channels {
            if let Some(existing) = current
                .channels
                .iter()
                .find(|existing| mixer_channels_match(channel, existing))
                && existing.name != channel.name
            {
                self.set_source_name_unlocked(&existing.id, &channel.name)
                    .await?;
            }
        }
        for channel in &desired.channels {
            if channel.source_kind == crate::MixerSourceKind::Virtual
                && !current
                    .channels
                    .iter()
                    .any(|item| mixer_channels_match(channel, item))
            {
                self.create_source_unlocked(&channel.name).await?;
            }
        }

        let current = self.mixer_snapshot_unlocked().await?;
        for (position, channel) in desired.channels.iter().enumerate() {
            let current_channel = current_channel_for_desired(channel, &current.channels)
                .ok_or_else(|| {
                    BridgeError::Backend(format!(
                        "profile source was not available after reconciliation: {}",
                        channel.name
                    ))
                })?;
            self.set_source_order_unlocked(&current_channel.id, position)
                .await?;
            if !mixer_colours_match(&current_channel.colour, &channel.colour) {
                self.set_source_colour_unlocked(&current_channel.id, &channel.colour)
                    .await?;
            }
            if current_channel.personal_volume != channel.personal_volume {
                self.set_volume_unlocked(
                    &current_channel.id,
                    MixBus::Personal,
                    channel.personal_volume,
                )
                .await?;
            }
            if current_channel.audience_volume != channel.audience_volume {
                self.set_volume_unlocked(
                    &current_channel.id,
                    MixBus::Audience,
                    channel.audience_volume,
                )
                .await?;
            }
            if current_channel.volumes_linked != channel.volumes_linked {
                self.set_volume_linked_unlocked(&current_channel.id, channel.volumes_linked)
                    .await?;
            }
            if current_channel.mute_state != channel.mute_state {
                self.set_mute_unlocked(&current_channel.id, channel.mute_state)
                    .await?;
            }
            if channel.source_kind == crate::MixerSourceKind::Physical
                && !descriptors_match(&channel.attached_devices, &current_channel.attached_devices)
            {
                if channel.attached_devices.is_empty() {
                    self.set_source_device_unlocked(&current_channel.id, None)
                        .await?;
                } else {
                    let input = current
                        .physical_inputs
                        .iter()
                        .find(|input| {
                            physical_device_matches(&channel.attached_devices[0], &input.descriptor)
                        })
                        .expect("physical input was validated before profile mutation");
                    self.set_source_device_unlocked(&current_channel.id, Some(input.node_id))
                        .await?;
                }
            }
        }
        for target in &desired.targets {
            if let Some(current_target) = current.targets.iter().find(|item| item.id == target.id) {
                if current_target.volume != target.volume {
                    self.set_target_volume_unlocked(&target.id, target.volume)
                        .await?;
                }
                if current_target.muted != target.muted {
                    self.set_target_mute_unlocked(&target.id, target.muted)
                        .await?;
                }
                if target.id == VOICE_CHAT_MIC_TARGET_ID {
                    // Voice Chat attachments have independent Link, primary, and
                    // copy roles. Only the explicit copy transaction below owns
                    // the copy descriptor; never run the destructive generic
                    // target-device replacement for this target.
                    continue;
                }
                if descriptors_match(&target.attached_devices, &current_target.attached_devices) {
                    continue;
                }
                if target.attached_devices.is_empty() {
                    self.set_target_device_unlocked(&target.id, None).await?;
                } else if let Some(output) = current.physical_outputs.iter().find(|output| {
                    crate::beacn_link_output_slot(&output.name, &output.descriptor).is_none()
                        && target.attached_devices.iter().any(|attached| {
                            crate::beacn_link_output_slot("", attached).is_none()
                                && physical_device_matches(attached, &output.descriptor)
                        })
                }) {
                    self.set_target_device_unlocked(&target.id, Some(output.node_id))
                        .await?;
                } else if target
                    .attached_devices
                    .iter()
                    .all(|attached| crate::beacn_link_output_slot("", attached).is_some())
                {
                    // Clear non-Link attachments first. Fixed Link assignments are
                    // restored below without replacing a target's other outputs.
                    self.set_target_device_unlocked(&target.id, None).await?;
                }
            }
        }
        let current = self.mixer_snapshot_unlocked().await?;
        for link in &current.link_outputs {
            let desired_target = desired_link_target(desired, link.slot);
            if link.target_id.as_deref() != desired_target {
                self.set_link_output_assignment_unlocked(link.node_id, desired_target)
                    .await?;
            }
        }

        if let Some(copy) = exact_copy_assignment(desired, VOICE_CHAT_MIC_TARGET_ID)? {
            let device_node_id = copy
                .output
                .as_ref()
                .map(|descriptor| physical_output_node_id(&current, descriptor))
                .transpose()?;
            self.set_mixer_copy_output_unlocked(SetMixerCopyOutputRequest {
                target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
                device_node_id,
            })
            .await?;
        }

        let current = self.mixer_snapshot_unlocked().await?;
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
                    self.set_route_unlocked(&source.id, &target.id, should_enable)
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
                self.set_application_route_unlocked(
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
            self.set_default_input_unlocked(default_input).await?;
        }
        if let Some(default_output) = desired.default_output.as_deref()
            && current.default_output.as_deref() != Some(default_output)
        {
            self.set_default_output_unlocked(default_output).await?;
        }
        Ok(())
    }
}

fn exact_copy_assignment<'a>(
    snapshot: &'a MixerSnapshot,
    target_id: &str,
) -> BridgeResult<Option<&'a MixerCopyOutputAssignment>> {
    let matching = snapshot
        .copy_outputs
        .iter()
        .filter(|assignment| assignment.target_id == target_id)
        .collect::<Vec<_>>();
    if matching.len() > 1 {
        return Err(BridgeError::InvalidValue(format!(
            "copy-output target has duplicate assignments: {target_id}"
        )));
    }
    Ok(matching.into_iter().next())
}

fn mixer_target<'a>(
    snapshot: &'a MixerSnapshot,
    target_id: &str,
) -> BridgeResult<&'a crate::MixerTarget> {
    snapshot
        .targets
        .iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| BridgeError::InvalidValue(format!("unknown target: {target_id}")))
}

fn physical_output_node_id(
    snapshot: &MixerSnapshot,
    descriptor: &MixerPhysicalDeviceDescriptor,
) -> BridgeResult<u32> {
    let matching = snapshot
        .physical_outputs
        .iter()
        .filter(|output| physical_device_matches(&output.descriptor, descriptor))
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(BridgeError::InvalidValue(format!(
            "copy output descriptor must resolve to exactly one physical endpoint; found {}",
            matching.len()
        )));
    }
    Ok(matching[0].node_id)
}

fn descriptor_count(
    descriptors: &[MixerPhysicalDeviceDescriptor],
    expected: &MixerPhysicalDeviceDescriptor,
) -> usize {
    descriptors
        .iter()
        .filter(|descriptor| physical_device_matches(descriptor, expected))
        .count()
}

fn optional_descriptors_match(
    left: Option<&MixerPhysicalDeviceDescriptor>,
    right: Option<&MixerPhysicalDeviceDescriptor>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => physical_device_matches(left, right),
        (None, None) => true,
        _ => false,
    }
}

fn remove_exact_descriptor(
    descriptors: &mut Vec<MixerPhysicalDeviceDescriptor>,
    expected: &MixerPhysicalDeviceDescriptor,
) -> BridgeResult<()> {
    let matching = descriptors
        .iter()
        .enumerate()
        .filter_map(|(index, descriptor)| {
            physical_device_matches(descriptor, expected).then_some(index)
        })
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err(BridgeError::Backend(format!(
            "expected exactly one tracked copy descriptor; found {}",
            matching.len()
        )));
    }
    descriptors.remove(matching[0]);
    Ok(())
}

fn descriptor_multisets_match(
    left: &[MixerPhysicalDeviceDescriptor],
    right: &[MixerPhysicalDeviceDescriptor],
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut used = vec![false; right.len()];
    left.iter().all(|descriptor| {
        let Some(index) = right.iter().enumerate().find_map(|(index, candidate)| {
            (!used[index] && physical_device_matches(descriptor, candidate)).then_some(index)
        }) else {
            return false;
        };
        used[index] = true;
        true
    })
}

fn verify_target_attachments(
    snapshot: &MixerSnapshot,
    expected: &[MixerPhysicalDeviceDescriptor],
    message: &str,
) -> BridgeResult<()> {
    let actual = &mixer_target(snapshot, VOICE_CHAT_MIC_TARGET_ID)?.attached_devices;
    if descriptor_multisets_match(expected, actual) {
        Ok(())
    } else {
        Err(BridgeError::Backend(message.into()))
    }
}

fn copy_commit_evidence(
    snapshot: &MixerSnapshot,
    assignment: &MixerCopyOutputAssignment,
) -> BridgeResult<MixerCopyOutputCommitEvidence> {
    Ok(MixerCopyOutputCommitEvidence {
        version: 1,
        assignment: assignment.clone(),
        voice_chat_attachments: mixer_target(snapshot, VOICE_CHAT_MIC_TARGET_ID)?
            .attached_devices
            .clone(),
    })
}

fn unmanaged_voice_attachments(
    target: &crate::MixerTarget,
    copy: Option<&MixerCopyOutputAssignment>,
) -> Vec<MixerPhysicalDeviceDescriptor> {
    target
        .attached_devices
        .iter()
        .filter(|descriptor| crate::beacn_link_output_slot("", descriptor).is_none())
        .filter(|descriptor| {
            !copy
                .and_then(|assignment| assignment.output.as_ref())
                .is_some_and(|copy| physical_device_matches(copy, descriptor))
        })
        .cloned()
        .collect()
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
        let attachments_match = if desired_target.id == VOICE_CHAT_MIC_TARGET_ID {
            let desired_copy = exact_copy_assignment(desired, VOICE_CHAT_MIC_TARGET_ID)?;
            let actual_copy = exact_copy_assignment(actual, VOICE_CHAT_MIC_TARGET_ID)?;
            desired_copy.is_none()
                || descriptor_multisets_match(
                    &unmanaged_voice_attachments(desired_target, desired_copy),
                    &unmanaged_voice_attachments(actual_target, actual_copy),
                )
        } else {
            descriptors_match(
                &desired_target.attached_devices,
                &actual_target.attached_devices,
            )
        };
        if desired_target.name != actual_target.name
            || desired_target.mix != actual_target.mix
            || desired_target.volume != actual_target.volume
            || desired_target.muted != actual_target.muted
            || !attachments_match
        {
            return Err(profile_verification_error(format!(
                "target did not match: {}",
                desired_target.name
            )));
        }
    }

    if let Some(desired_copy) = exact_copy_assignment(desired, VOICE_CHAT_MIC_TARGET_ID)? {
        let actual_copy = exact_copy_assignment(actual, VOICE_CHAT_MIC_TARGET_ID)?
            .ok_or_else(|| profile_verification_error("copy-output assignment was missing"))?;
        if !optional_descriptors_match(desired_copy.output.as_ref(), actual_copy.output.as_ref()) {
            return Err(profile_verification_error(
                "copy-output assignment did not match",
            ));
        }
        let actual_target = mixer_target(actual, VOICE_CHAT_MIC_TARGET_ID)?;
        if let Some(descriptor) = desired_copy.output.as_ref()
            && descriptor_count(&actual_target.attached_devices, descriptor) != 1
        {
            return Err(profile_verification_error(
                "copy-output attachment did not match",
            ));
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
    if desired
        .copy_outputs
        .iter()
        .any(|assignment| assignment.target_id != VOICE_CHAT_MIC_TARGET_ID)
    {
        return Err(BridgeError::InvalidValue(format!(
            "profile copy output is supported only for target: {VOICE_CHAT_MIC_TARGET_ID}"
        )));
    }
    let desired_copy = exact_copy_assignment(desired, VOICE_CHAT_MIC_TARGET_ID)?;
    let current_copy = exact_copy_assignment(current, VOICE_CHAT_MIC_TARGET_ID)?;
    if let Some(descriptor) = current_copy.and_then(|assignment| assignment.output.as_ref()) {
        let current_target = mixer_target(current, VOICE_CHAT_MIC_TARGET_ID)?;
        let count = descriptor_count(&current_target.attached_devices, descriptor);
        if count != 1 {
            return Err(BridgeError::InvalidValue(format!(
                "current tracked copy output must match exactly one Voice Chat attachment; found {count}"
            )));
        }
    }
    if let Some(descriptor) = desired_copy.and_then(|assignment| assignment.output.as_ref()) {
        let Some(output) = current
            .physical_outputs
            .iter()
            .find(|output| physical_device_matches(descriptor, &output.descriptor))
        else {
            return Err(BridgeError::InvalidValue(
                "profile copy output is unavailable".into(),
            ));
        };
        if crate::beacn_link_output_slot(&output.name, &output.descriptor).is_some() {
            return Err(BridgeError::InvalidValue(
                "profile copy output cannot use a Link-reserved endpoint".into(),
            ));
        }
        let desired_target = mixer_target(desired, VOICE_CHAT_MIC_TARGET_ID)?;
        let count = descriptor_count(&desired_target.attached_devices, descriptor);
        if count != 1 {
            return Err(BridgeError::InvalidValue(format!(
                "profile copy output must match exactly one Voice Chat attachment; found {count}"
            )));
        }
    }
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
        if target.id == VOICE_CHAT_MIC_TARGET_ID {
            if desired_copy.is_some() {
                let desired_unmanaged = unmanaged_voice_attachments(target, desired_copy);
                let current_unmanaged = unmanaged_voice_attachments(current_target, current_copy);
                if !descriptor_multisets_match(&desired_unmanaged, &current_unmanaged) {
                    return Err(BridgeError::InvalidValue(
                        "profile cannot replace unmanaged Voice Chat output attachments".into(),
                    ));
                }
            }
        } else {
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
        copy_outputs: vec![MixerCopyOutputAssignment {
            target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
            output: None,
        }],
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

        async fn attach_target_output(
            &self,
            target_id: &str,
            device_node_id: u32,
        ) -> BridgeResult<()> {
            self.mutate(
                "attach_target_output",
                self.inner.attach_target_output(target_id, device_node_id),
            )
            .await
        }

        async fn detach_target_output(
            &self,
            target_id: &str,
            descriptor: &MixerPhysicalDeviceDescriptor,
        ) -> BridgeResult<()> {
            self.mutate(
                "detach_target_output",
                self.inner.detach_target_output(target_id, descriptor),
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

    struct PausingStudio {
        inner: MockStudioBackend,
        snapshot_entered: Arc<tokio::sync::Semaphore>,
        snapshot_release: Arc<tokio::sync::Semaphore>,
    }

    impl Default for PausingStudio {
        fn default() -> Self {
            Self {
                inner: MockStudioBackend::default(),
                snapshot_entered: Arc::new(tokio::sync::Semaphore::new(0)),
                snapshot_release: Arc::new(tokio::sync::Semaphore::new(0)),
            }
        }
    }

    impl PausingStudio {
        async fn wait_until_snapshot_paused(&self) {
            self.snapshot_entered.acquire().await.unwrap().forget();
        }

        fn release_snapshot(&self) {
            self.snapshot_release.add_permits(1);
        }
    }

    #[async_trait::async_trait]
    impl StudioBackend for PausingStudio {
        async fn snapshot(&self) -> BridgeResult<StudioSnapshot> {
            self.snapshot_entered.add_permits(1);
            self.snapshot_release.acquire().await.unwrap().forget();
            self.inner.snapshot().await
        }

        async fn microphone_dsp_snapshot(&self) -> BridgeResult<MicrophoneDspSnapshot> {
            self.inner.microphone_dsp_snapshot().await
        }

        async fn set_microphone_dsp(
            &self,
            update: MicrophoneDspUpdate,
        ) -> BridgeResult<MicrophoneDspSnapshot> {
            self.inner.set_microphone_dsp(update).await
        }

        async fn set_microphone(&self, request: SetMicrophoneRequest) -> BridgeResult<()> {
            self.inner.set_microphone(request).await
        }

        async fn set_link_assignment(
            &self,
            application: &str,
            channel: LinkChannel,
        ) -> BridgeResult<()> {
            self.inner.set_link_assignment(application, channel).await
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

    fn copy_request(device_node_id: Option<u32>) -> SetMixerCopyOutputRequest {
        SetMixerCopyOutputRequest {
            target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
            device_node_id,
        }
    }

    #[tokio::test]
    async fn constructor_seeds_legacy_nothing_and_restores_durable_copy_ownership() {
        let mixer = Arc::new(MockMixerBackend::default());
        let first = StudioBridgeService::new(Arc::new(MockStudioBackend::default()), mixer.clone());
        assert!(
            first.snapshot().await.unwrap().mixer.copy_outputs[0]
                .output
                .is_none()
        );
        let applied = first
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        let durable = first.snapshot().await.unwrap().mixer.copy_outputs;

        let restarted = StudioBridgeService::new_with_copy_output_state(
            Arc::new(MockStudioBackend::default()),
            mixer,
            durable,
            Some(applied.commit_evidence),
        )
        .unwrap();
        restarted
            .set_mixer_copy_output(copy_request(None))
            .await
            .unwrap();
        let snapshot = restarted.snapshot().await.unwrap().mixer;
        assert!(snapshot.copy_outputs[0].output.is_none());
        assert_eq!(
            descriptor_count(
                &mixer_target(&snapshot, VOICE_CHAT_MIC_TARGET_ID)
                    .unwrap()
                    .attached_devices,
                &snapshot
                    .physical_outputs
                    .iter()
                    .find(|output| output.node_id == 103)
                    .unwrap()
                    .descriptor
            ),
            0
        );

        let malformed = StudioBridgeService::new_with_copy_outputs(
            Arc::new(MockStudioBackend::default()),
            Arc::new(MockMixerBackend::default()),
            vec![MixerCopyOutputAssignment {
                target_id: "headphones".into(),
                output: None,
            }],
        )
        .err()
        .expect("malformed live ownership must be rejected");
        assert!(malformed.to_string().contains("exactly one assignment"));
    }

    #[tokio::test]
    async fn tampered_durable_link_or_unmanaged_copy_role_fails_before_backend_mutation() {
        let (template_service, template_mixer) = fault_service();
        let template = template_service.snapshot().await.unwrap().mixer;
        let raw_before = template_mixer.inner.snapshot().await.unwrap();
        let link = template
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 102)
            .unwrap()
            .descriptor
            .clone();
        let link_assignment = MixerCopyOutputAssignment {
            target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
            output: Some(link),
        };
        let link_service = StudioBridgeService::new_with_copy_output_state(
            Arc::new(MockStudioBackend::default()),
            Arc::new(template_mixer.clone()),
            vec![link_assignment.clone()],
            Some(MixerCopyOutputCommitEvidence {
                version: 1,
                assignment: link_assignment,
                voice_chat_attachments: mixer_target(&template, VOICE_CHAT_MIC_TARGET_ID)
                    .unwrap()
                    .attached_devices
                    .clone(),
            }),
        )
        .unwrap();
        template_mixer.inject("detach_target_output", FaultMode::FailBefore, 1);
        let link_error = link_service
            .set_mixer_copy_output(copy_request(None))
            .await
            .unwrap_err();
        assert!(link_error.to_string().contains("Link-reserved"));
        assert!(!link_error.to_string().contains("injected"));
        assert_eq!(template_mixer.inner.snapshot().await.unwrap(), raw_before);

        let (unmanaged_template, unmanaged_mixer) = fault_service();
        unmanaged_mixer
            .inner
            .attach_target_output(VOICE_CHAT_MIC_TARGET_ID, 103)
            .await
            .unwrap();
        let unmanaged_snapshot = unmanaged_template.snapshot().await.unwrap().mixer;
        let unrelated = unmanaged_snapshot
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 103)
            .unwrap()
            .descriptor
            .clone();
        let unmanaged_service = StudioBridgeService::new_with_copy_outputs(
            Arc::new(MockStudioBackend::default()),
            Arc::new(unmanaged_mixer.clone()),
            vec![MixerCopyOutputAssignment {
                target_id: VOICE_CHAT_MIC_TARGET_ID.into(),
                output: Some(unrelated),
            }],
        )
        .unwrap();
        let unmanaged_before = unmanaged_mixer.inner.snapshot().await.unwrap();
        unmanaged_mixer.inject("detach_target_output", FaultMode::FailBefore, 1);
        let unmanaged_error = unmanaged_service
            .set_mixer_copy_output(copy_request(None))
            .await
            .unwrap_err();
        assert!(
            unmanaged_error
                .to_string()
                .contains("no committed attachment evidence")
        );
        assert!(!unmanaged_error.to_string().contains("injected"));
        assert_eq!(
            unmanaged_mixer.inner.snapshot().await.unwrap(),
            unmanaged_before
        );
    }

    #[tokio::test]
    async fn voice_chat_copy_output_is_additive_and_nothing_removes_only_the_tracked_copy() {
        let service = service();
        let before = service.snapshot().await.unwrap().mixer;
        let before_target = mixer_target(&before, VOICE_CHAT_MIC_TARGET_ID)
            .unwrap()
            .attached_devices
            .clone();
        let before_links = before.link_outputs.clone();

        let applied = service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        assert!(applied.verified);
        let descriptor = before
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 103)
            .unwrap()
            .descriptor
            .clone();
        assert!(optional_descriptors_match(
            applied.assignment.output.as_ref(),
            Some(&descriptor)
        ));
        let mut expected = before_target.clone();
        expected.push(descriptor);
        assert!(descriptor_multisets_match(
            &expected,
            &mixer_target(&applied.snapshot, VOICE_CHAT_MIC_TARGET_ID)
                .unwrap()
                .attached_devices
        ));
        assert_eq!(applied.snapshot.link_outputs, before_links);

        let cleared = service
            .set_mixer_copy_output(copy_request(None))
            .await
            .unwrap();
        assert!(cleared.assignment.output.is_none());
        assert!(descriptor_multisets_match(
            &before_target,
            &mixer_target(&cleared.snapshot, VOICE_CHAT_MIC_TARGET_ID)
                .unwrap()
                .attached_devices
        ));
        assert_eq!(cleared.snapshot.link_outputs, before_links);
    }

    #[tokio::test]
    async fn generic_target_device_cannot_replace_tracked_voice_chat_copy_output() {
        let (service, mixer) = fault_service();
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        let before = service.snapshot().await.unwrap().mixer;
        let raw_before = mixer.inner.snapshot().await.unwrap();

        let error = service
            .set_target_device(VOICE_CHAT_MIC_TARGET_ID, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("copy-output control"));
        assert_eq!(service.snapshot().await.unwrap().mixer, before);
        assert_eq!(mixer.inner.snapshot().await.unwrap(), raw_before);
    }

    #[tokio::test]
    async fn equivalent_copy_noop_returns_the_authoritative_tracked_descriptor() {
        let service = service();
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        service.copy_outputs.write().await[0]
            .output
            .as_mut()
            .unwrap()
            .description = Some("Previously enumerated capture card".into());

        let result = service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        assert_eq!(result.assignment, result.snapshot.copy_outputs[0]);
        assert_eq!(
            result.assignment.output.unwrap().description.as_deref(),
            Some("Previously enumerated capture card")
        );
    }

    #[tokio::test]
    async fn copy_output_rejects_wrong_targets_conflicts_and_ambiguous_tracking_before_writes() {
        let (service, mixer) = fault_service();
        let wrong_target = service
            .set_mixer_copy_output(SetMixerCopyOutputRequest {
                target_id: "headphones".into(),
                device_node_id: Some(103),
            })
            .await
            .unwrap_err();
        assert!(wrong_target.to_string().contains("only for target"));

        mixer
            .inner
            .attach_target_output(VOICE_CHAT_MIC_TARGET_ID, 103)
            .await
            .unwrap();
        let before = mixer.inner.snapshot().await.unwrap();
        let conflict = service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap_err();
        assert!(
            conflict
                .to_string()
                .contains("another Voice Chat attachment role")
        );
        assert_eq!(mixer.inner.snapshot().await.unwrap(), before);

        let (service, mixer) = fault_service();
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        mixer
            .inner
            .attach_target_output(VOICE_CHAT_MIC_TARGET_ID, 103)
            .await
            .unwrap();
        let ambiguous = service
            .set_mixer_copy_output(copy_request(None))
            .await
            .unwrap_err();
        assert!(
            ambiguous
                .to_string()
                .contains("does not match committed attachment evidence")
        );
    }

    #[tokio::test]
    async fn copy_replacement_attaches_first_and_public_snapshots_wait_for_commit() {
        let (service, mixer) = fault_service();
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        let before = service.snapshot().await.unwrap().mixer;
        let old = before
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 103)
            .unwrap()
            .descriptor
            .clone();
        let new = before
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 101)
            .unwrap()
            .descriptor
            .clone();
        mixer.pause_once("detach_target_output");

        let change_service = service.clone();
        let change = tokio::spawn(async move {
            change_service
                .set_mixer_copy_output(copy_request(Some(101)))
                .await
        });
        mixer.wait_until_paused().await;

        let raw = mixer.inner.snapshot().await.unwrap();
        let raw_target = mixer_target(&raw, VOICE_CHAT_MIC_TARGET_ID).unwrap();
        assert_eq!(descriptor_count(&raw_target.attached_devices, &old), 1);
        assert_eq!(descriptor_count(&raw_target.attached_devices, &new), 1);
        let snapshot_service = service.clone();
        let committed_snapshot = tokio::spawn(async move { snapshot_service.snapshot().await });
        tokio::task::yield_now().await;
        assert!(
            !committed_snapshot.is_finished(),
            "public snapshot exposed attachments before copy metadata committed"
        );

        mixer.release_pause();
        let result = change.await.unwrap().unwrap();
        let observed = committed_snapshot.await.unwrap().unwrap().mixer;
        assert_eq!(result.snapshot, observed);
        let target = mixer_target(&observed, VOICE_CHAT_MIC_TARGET_ID).unwrap();
        assert_eq!(descriptor_count(&target.attached_devices, &old), 0);
        assert_eq!(descriptor_count(&target.attached_devices, &new), 1);
        assert!(optional_descriptors_match(
            observed.copy_outputs[0].output.as_ref(),
            Some(&new)
        ));
    }

    #[tokio::test]
    async fn canceled_copy_caller_cannot_abandon_partial_attachments_or_release_the_gate() {
        let (service, mixer) = fault_service();
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        mixer.pause_once("detach_target_output");

        let timed_service = service.clone();
        let timed = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(30),
                timed_service.set_mixer_copy_output(copy_request(Some(101))),
            )
            .await
        });
        mixer.wait_until_paused().await;
        assert!(
            timed.await.unwrap().is_err(),
            "copy caller did not time out"
        );

        let newer_service = service.clone();
        let newer =
            tokio::spawn(
                async move { newer_service.set_volume("chat", MixBus::Personal, 72).await },
            );
        tokio::task::yield_now().await;
        assert!(
            !newer.is_finished(),
            "copy caller cancellation released its partial transaction"
        );
        mixer.release_pause();
        newer.await.unwrap().unwrap();

        let committed = service.snapshot().await.unwrap().mixer;
        let selected = committed
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 101)
            .unwrap();
        assert!(optional_descriptors_match(
            committed.copy_outputs[0].output.as_ref(),
            Some(&selected.descriptor)
        ));
        assert_eq!(
            committed
                .channels
                .iter()
                .find(|channel| channel.id == "chat")
                .unwrap()
                .personal_volume,
            72
        );
    }

    #[tokio::test]
    async fn copy_failures_compensate_exactly_and_report_rollback_failure() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        mixer.inject("attach_target_output", FaultMode::FailAfter, 1);
        let error = service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("attachment state was restored"));
        assert_eq!(service.snapshot().await.unwrap().mixer, before);

        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        let before_replacement = service.snapshot().await.unwrap().mixer;
        mixer.inject("detach_target_output", FaultMode::FailAfter, 1);
        let error = service
            .set_mixer_copy_output(copy_request(Some(101)))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("attachment state was restored"));
        assert_eq!(service.snapshot().await.unwrap().mixer, before_replacement);

        mixer.inject("detach_target_output", FaultMode::FailAfter, usize::MAX);
        let error = service
            .set_mixer_copy_output(copy_request(Some(101)))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("rollback could not restore"));
        assert!(error.to_string().contains("apply error"));
        assert!(error.to_string().contains("rollback error"));
    }

    #[tokio::test]
    async fn profiles_restore_explicit_copy_nothing_and_preserve_legacy_omission() {
        let service = service();
        let explicit_nothing = service.snapshot().await.unwrap().mixer;
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        service
            .apply_mixer_profile(&explicit_nothing)
            .await
            .unwrap();
        assert!(
            service.snapshot().await.unwrap().mixer.copy_outputs[0]
                .output
                .is_none()
        );

        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        let mut explicit_copy = service.snapshot().await.unwrap().mixer;
        let mut legacy = explicit_copy.clone();
        legacy.copy_outputs.clear();
        service.apply_mixer_profile(&legacy).await.unwrap();
        assert!(
            service.snapshot().await.unwrap().mixer.copy_outputs[0]
                .output
                .is_some()
        );

        service
            .set_mixer_copy_output(copy_request(Some(101)))
            .await
            .unwrap();
        service.apply_mixer_profile(&explicit_copy).await.unwrap();
        let restored = service.snapshot().await.unwrap().mixer;
        assert!(optional_descriptors_match(
            restored.copy_outputs[0].output.as_ref(),
            explicit_copy.copy_outputs[0].output.as_ref()
        ));

        explicit_copy
            .copy_outputs
            .push(explicit_copy.copy_outputs[0].clone());
        let duplicate = service
            .apply_mixer_profile(&explicit_copy)
            .await
            .unwrap_err();
        assert!(duplicate.to_string().contains("duplicate assignments"));
    }

    #[tokio::test]
    async fn profile_failure_after_copy_commit_restores_the_previous_tracked_copy() {
        let (service, mixer) = fault_service();
        service
            .set_mixer_copy_output(copy_request(Some(103)))
            .await
            .unwrap();
        let before = service.snapshot().await.unwrap().mixer;
        let old = before.copy_outputs[0].output.clone().unwrap();
        let new = before
            .physical_outputs
            .iter()
            .find(|output| output.node_id == 101)
            .unwrap()
            .descriptor
            .clone();
        let mut desired = changed_profile(before.clone());
        let voice = desired
            .targets
            .iter_mut()
            .find(|target| target.id == VOICE_CHAT_MIC_TARGET_ID)
            .unwrap();
        remove_exact_descriptor(&mut voice.attached_devices, &old).unwrap();
        voice.attached_devices.push(new.clone());
        desired.copy_outputs[0].output = Some(new);
        mixer.inject("set_route", FaultMode::FailAfter, 1);

        let error = service.apply_mixer_profile(&desired).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("previous mixer state was restored")
        );
        let restored = service.snapshot().await.unwrap().mixer;
        verify_mixer_profile(&before, &restored).unwrap();
        assert!(optional_descriptors_match(
            restored.copy_outputs[0].output.as_ref(),
            Some(&old)
        ));
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
    async fn overlapping_profiles_and_direct_controls_are_serialized_without_lost_updates() {
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
        let mut expected = second_profile.clone();
        expected
            .channels
            .iter_mut()
            .find(|channel| channel.id == "chat")
            .unwrap()
            .personal_volume = 72;
        let second =
            tokio::spawn(async move { second_service.apply_mixer_profile(&second_profile).await });
        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "second profile bypassed transaction lock"
        );

        let direct_service = service.clone();
        let direct = tokio::spawn(async move {
            direct_service
                .set_volume("chat", MixBus::Personal, 72)
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !direct.is_finished(),
            "direct mixer control bypassed the active profile transaction"
        );
        let snapshot_service = service.clone();
        let snapshot = tokio::spawn(async move { snapshot_service.snapshot().await });
        tokio::task::yield_now().await;
        assert!(
            !snapshot.is_finished(),
            "snapshot exposed a partially applied profile transaction"
        );

        mixer.release_pause();
        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        direct.await.unwrap().unwrap();
        verify_mixer_profile(&expected, &snapshot.await.unwrap().unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn stalled_studio_snapshot_does_not_hold_the_mixer_read_gate() {
        let studio = Arc::new(PausingStudio::default());
        let mixer = Arc::new(MockMixerBackend::default());
        let service = StudioBridgeService::new(studio.clone(), mixer.clone());
        let snapshot_service = service.clone();
        let snapshot = tokio::spawn(async move { snapshot_service.snapshot().await });
        studio.wait_until_snapshot_paused().await;

        let mutation = tokio::time::timeout(
            Duration::from_millis(250),
            service.set_volume("chat", MixBus::Personal, 61),
        )
        .await;
        studio.release_snapshot();
        snapshot.await.unwrap().unwrap();

        mutation
            .expect("stalled Studio snapshot held the independent mixer read gate")
            .unwrap();
        assert_eq!(
            mixer
                .snapshot()
                .await
                .unwrap()
                .channels
                .iter()
                .find(|channel| channel.id == "chat")
                .unwrap()
                .personal_volume,
            61
        );
    }

    #[tokio::test]
    async fn profile_rollback_finishes_before_a_newer_direct_mutation_runs() {
        let (service, mixer) = fault_service();
        let before = service.snapshot().await.unwrap().mixer;
        let desired = changed_profile(before.clone());
        mixer.pause_once("set_source_name");
        mixer.inject("set_route", FaultMode::FailAfter, 1);

        let profile_service = service.clone();
        let profile =
            tokio::spawn(async move { profile_service.apply_mixer_profile(&desired).await });
        mixer.wait_until_paused().await;

        let direct_service = service.clone();
        let direct = tokio::spawn(async move {
            direct_service
                .set_volume("chat", MixBus::Personal, 72)
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !direct.is_finished(),
            "newer direct mutation ran inside the profile rollback boundary"
        );

        mixer.release_pause();
        let error = profile.await.unwrap().unwrap_err();
        assert!(
            error
                .to_string()
                .contains("previous mixer state was restored")
        );
        direct.await.unwrap().unwrap();

        let mut expected = before;
        expected
            .channels
            .iter_mut()
            .find(|channel| channel.id == "chat")
            .unwrap()
            .personal_volume = 72;
        verify_mixer_profile(&expected, &service.snapshot().await.unwrap().mixer).unwrap();
    }

    #[tokio::test]
    async fn canceled_direct_caller_does_not_release_the_gate_before_its_write_finishes() {
        let (service, mixer) = fault_service();
        mixer.pause_once("set_volume");

        let timed_service = service.clone();
        let timed = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(30),
                timed_service.set_volume("chat", MixBus::Personal, 61),
            )
            .await
        });
        mixer.wait_until_paused().await;
        assert!(
            timed.await.unwrap().is_err(),
            "direct caller did not time out"
        );

        let newer_service = service.clone();
        let newer =
            tokio::spawn(
                async move { newer_service.set_volume("chat", MixBus::Personal, 72).await },
            );
        tokio::task::yield_now().await;
        assert!(
            !newer.is_finished(),
            "caller cancellation released a still-active direct mutation"
        );

        mixer.release_pause();
        tokio::time::timeout(Duration::from_secs(2), newer)
            .await
            .expect("mutation gate was not released after detached write completed")
            .unwrap()
            .unwrap();
        let state = service.snapshot().await.unwrap().mixer;
        assert_eq!(
            state
                .channels
                .iter()
                .find(|channel| channel.id == "chat")
                .unwrap()
                .personal_volume,
            72,
            "newer direct mutation was overwritten by the canceled caller's write"
        );
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
    async fn mixer_mutation_gate_is_released_after_verified_profile_rollback() {
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
        .expect("mixer mutation gate was not released after error rollback")
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
