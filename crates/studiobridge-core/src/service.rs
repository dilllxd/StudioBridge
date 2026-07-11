use crate::{
    AppSnapshot, BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel,
    MicrophoneDspSnapshot, MicrophoneDspUpdate, MicrophoneDspWriteResult, MicrophoneState, MixBus,
    MixerBackend, MixerSnapshot, MuteState, SetMicrophoneRequest, StudioBackend, StudioIdentity,
    StudioSnapshot,
};
use std::{future::Future, sync::Arc, time::Duration};

const BACKEND_TIMEOUT: Duration = Duration::from_secs(5);
const DSP_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct StudioBridgeService {
    studio: Arc<dyn StudioBackend>,
    mixer: Arc<dyn MixerBackend>,
}

impl StudioBridgeService {
    pub fn new(studio: Arc<dyn StudioBackend>, mixer: Arc<dyn MixerBackend>) -> Self {
        Self { studio, mixer }
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

    pub async fn remove_source(&self, source_id: &str) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.remove_source(source_id)).await
    }
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockMixerBackend, MockStudioBackend};

    fn service() -> StudioBridgeService {
        StudioBridgeService::new(
            Arc::new(MockStudioBackend::default()),
            Arc::new(MockMixerBackend::default()),
        )
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
                .any(|target| target.id == "voice-chat-mic")
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
        let state = service.snapshot().await.unwrap();
        assert!(
            state
                .mixer
                .channels
                .iter()
                .any(|channel| channel.id == "aux-1" && channel.name == "Aux 1")
        );

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
    async fn mock_dsp_snapshot_contains_complete_profiles() {
        let dsp = service().microphone_dsp_snapshot().await.unwrap();
        assert_eq!(dsp.equalizer.simple.bands.len(), 8);
        assert_eq!(dsp.equalizer.advanced.bands.len(), 8);
        assert_eq!(dsp.headphone_equalizer.bands.len(), 3);
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
