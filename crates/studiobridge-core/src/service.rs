use crate::{
    AppSnapshot, BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel,
    MicrophoneState, MixBus, MixerBackend, MixerSnapshot, MuteState, SetMicrophoneRequest,
    StudioBackend, StudioIdentity, StudioSnapshot,
};
use std::{future::Future, sync::Arc, time::Duration};

const BACKEND_TIMEOUT: Duration = Duration::from_secs(5);

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

    pub async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()> {
        backend_call("PipeWeaver", self.mixer.set_volume(channel_id, mix, volume)).await
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

        let state = service.snapshot().await.unwrap();
        assert_eq!(state.mixer.channels[0].audience_volume, 41);
        assert_eq!(
            state.studio.linked_applications[0].channel,
            LinkChannel::Link3
        );
        assert!(
            state
                .mixer
                .routes
                .iter()
                .any(|route| { route.source_id == "chat" && route.target_id == "headphones" })
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
