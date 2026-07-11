use crate::{
    BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel, LinkedApplication,
    MicrophoneState, MixBus, MixerChannel, MixerRoute, MixerSnapshot, MixerTarget, MuteState,
    SetMicrophoneRequest, StudioIdentity, StudioSnapshot,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait StudioBackend: Send + Sync {
    async fn snapshot(&self) -> BridgeResult<StudioSnapshot>;
    async fn set_microphone(&self, request: SetMicrophoneRequest) -> BridgeResult<()>;
    async fn set_link_assignment(
        &self,
        application: &str,
        channel: LinkChannel,
    ) -> BridgeResult<()>;
}

#[async_trait]
pub trait MixerBackend: Send + Sync {
    async fn snapshot(&self) -> BridgeResult<MixerSnapshot>;
    async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()>;
    async fn set_mute(&self, channel_id: &str, state: MuteState) -> BridgeResult<()>;
    async fn set_route(&self, source_id: &str, target_id: &str, enabled: bool) -> BridgeResult<()>;
}

pub struct MockStudioBackend {
    state: Arc<RwLock<StudioSnapshot>>,
}

impl Default for MockStudioBackend {
    fn default() -> Self {
        Self {
            state: Arc::new(RwLock::new(StudioSnapshot {
                identity: StudioIdentity {
                    status: BackendStatus::Mock,
                    error: None,
                    product: "BEACN Studio".into(),
                    serial: Some("MOCK-STUDIO-001".into()),
                    firmware: Some("mock-1.0.0".into()),
                    usb_port: "USB1".into(),
                },
                microphone: MicrophoneState {
                    gain_db: 48,
                    phantom_power: false,
                    muted: false,
                },
                headphones: HeadphoneState {
                    volume: 62,
                    mic_monitor: 35,
                    muted: false,
                },
                linked_applications: vec![
                    LinkedApplication {
                        name: "Game".into(),
                        channel: LinkChannel::Link1,
                    },
                    LinkedApplication {
                        name: "Steam".into(),
                        channel: LinkChannel::Link1,
                    },
                    LinkedApplication {
                        name: "Browser".into(),
                        channel: LinkChannel::Link2,
                    },
                ],
            })),
        }
    }
}

#[async_trait]
impl StudioBackend for MockStudioBackend {
    async fn snapshot(&self) -> BridgeResult<StudioSnapshot> {
        Ok(self.state.read().await.clone())
    }

    async fn set_microphone(&self, request: SetMicrophoneRequest) -> BridgeResult<()> {
        let mut state = self.state.write().await;
        if let Some(gain_db) = request.gain_db {
            if gain_db > 69 {
                return Err(BridgeError::InvalidValue(
                    "microphone gain must be between 0 and 69 dB".into(),
                ));
            }
            state.microphone.gain_db = gain_db;
        }
        if let Some(phantom_power) = request.phantom_power {
            state.microphone.phantom_power = phantom_power;
        }
        if let Some(muted) = request.muted {
            state.microphone.muted = muted;
        }
        Ok(())
    }

    async fn set_link_assignment(
        &self,
        application: &str,
        channel: LinkChannel,
    ) -> BridgeResult<()> {
        let mut state = self.state.write().await;
        let linked = state
            .linked_applications
            .iter_mut()
            .find(|item| item.name == application)
            .ok_or_else(|| {
                BridgeError::InvalidValue(format!("unknown application: {application}"))
            })?;
        linked.channel = channel;
        Ok(())
    }
}

pub struct MockMixerBackend {
    state: Arc<RwLock<MixerSnapshot>>,
}

impl Default for MockMixerBackend {
    fn default() -> Self {
        let channel = |id: &str, name: &str, colour: &str, apps: &[&str]| MixerChannel {
            id: id.into(),
            name: name.into(),
            colour: colour.into(),
            personal_volume: 72,
            audience_volume: 68,
            mute_state: MuteState::Unmuted,
            applications: apps.iter().map(|value| (*value).into()).collect(),
        };

        Self {
            state: Arc::new(RwLock::new(MixerSnapshot {
                status: BackendStatus::Mock,
                error: None,
                engine: "PipeWeaver (mock)".into(),
                channels: vec![
                    channel("game", "Game PC", "#ef6f4d", &["Game", "Steam"]),
                    channel("chat", "Chat", "#54a7d9", &["Discord"]),
                    channel("music", "Music", "#b780e0", &["Spotify"]),
                    channel("system", "System", "#e8ba58", &["Desktop Audio"]),
                    channel("alerts", "Alerts", "#64c493", &["OBS"]),
                    channel("microphone", "Microphone", "#d95b52", &["BEACN Studio Mic"]),
                ],
                targets: vec![
                    MixerTarget {
                        id: "headphones".into(),
                        name: "Headphones".into(),
                        mix: MixBus::Personal,
                        volume: 62,
                        muted: false,
                    },
                    MixerTarget {
                        id: "audience-mix".into(),
                        name: "Audience Mix".into(),
                        mix: MixBus::Audience,
                        volume: 100,
                        muted: false,
                    },
                ],
                routes: vec![
                    MixerRoute {
                        source_id: "game".into(),
                        target_id: "headphones".into(),
                    },
                    MixerRoute {
                        source_id: "game".into(),
                        target_id: "audience-mix".into(),
                    },
                ],
            })),
        }
    }
}

#[async_trait]
impl MixerBackend for MockMixerBackend {
    async fn snapshot(&self) -> BridgeResult<MixerSnapshot> {
        Ok(self.state.read().await.clone())
    }

    async fn set_volume(&self, channel_id: &str, mix: MixBus, volume: u8) -> BridgeResult<()> {
        if volume > 100 {
            return Err(BridgeError::InvalidValue(
                "volume must be between 0 and 100".into(),
            ));
        }
        let mut state = self.state.write().await;
        let channel = state
            .channels
            .iter_mut()
            .find(|channel| channel.id == channel_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown channel: {channel_id}")))?;
        match mix {
            MixBus::Personal => channel.personal_volume = volume,
            MixBus::Audience => channel.audience_volume = volume,
        }
        Ok(())
    }

    async fn set_mute(&self, channel_id: &str, mute_state: MuteState) -> BridgeResult<()> {
        let mut state = self.state.write().await;
        let channel = state
            .channels
            .iter_mut()
            .find(|channel| channel.id == channel_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown channel: {channel_id}")))?;
        channel.mute_state = mute_state;
        Ok(())
    }

    async fn set_route(&self, source_id: &str, target_id: &str, enabled: bool) -> BridgeResult<()> {
        let mut state = self.state.write().await;
        if !state.channels.iter().any(|channel| channel.id == source_id) {
            return Err(BridgeError::InvalidValue(format!(
                "unknown source channel: {source_id}"
            )));
        }
        if !state.targets.iter().any(|target| target.id == target_id) {
            return Err(BridgeError::InvalidValue(format!(
                "unknown target: {target_id}"
            )));
        }
        let existing = state
            .routes
            .iter()
            .position(|route| route.source_id == source_id && route.target_id == target_id);
        match (enabled, existing) {
            (true, None) => state.routes.push(MixerRoute {
                source_id: source_id.into(),
                target_id: target_id.into(),
            }),
            (false, Some(index)) => {
                state.routes.remove(index);
            }
            _ => {}
        }
        Ok(())
    }
}
