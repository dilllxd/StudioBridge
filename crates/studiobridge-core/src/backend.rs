use crate::dsp::*;
use crate::{
    BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel, LinkedApplication,
    MicrophoneState, MixBus, MixerApplication, MixerChannel, MixerRoute, MixerSnapshot,
    MixerSourceKind, MixerTarget, MuteState, SetMicrophoneRequest, StudioIdentity, StudioSnapshot,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

#[async_trait]
pub trait StudioBackend: Send + Sync {
    async fn snapshot(&self) -> BridgeResult<StudioSnapshot>;
    async fn microphone_dsp_snapshot(&self) -> BridgeResult<MicrophoneDspSnapshot>;
    async fn set_microphone_dsp(
        &self,
        update: MicrophoneDspUpdate,
    ) -> BridgeResult<MicrophoneDspSnapshot>;
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
    async fn set_target_volume(&self, target_id: &str, volume: u8) -> BridgeResult<()>;
    async fn set_volume_linked(&self, channel_id: &str, linked: bool) -> BridgeResult<()>;
    async fn set_mute(&self, channel_id: &str, state: MuteState) -> BridgeResult<()>;
    async fn set_route(&self, source_id: &str, target_id: &str, enabled: bool) -> BridgeResult<()>;
    async fn set_application_route(
        &self,
        process: &str,
        application: &str,
        channel_id: Option<&str>,
    ) -> BridgeResult<()>;
}

pub struct MockStudioBackend {
    state: Arc<RwLock<StudioSnapshot>>,
    dsp: Arc<RwLock<MicrophoneDspSnapshot>>,
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
            dsp: Arc::new(RwLock::new(mock_microphone_dsp())),
        }
    }
}

#[async_trait]
impl StudioBackend for MockStudioBackend {
    async fn snapshot(&self) -> BridgeResult<StudioSnapshot> {
        Ok(self.state.read().await.clone())
    }

    async fn microphone_dsp_snapshot(&self) -> BridgeResult<MicrophoneDspSnapshot> {
        Ok(self.dsp.read().await.clone())
    }

    async fn set_microphone_dsp(
        &self,
        update: MicrophoneDspUpdate,
    ) -> BridgeResult<MicrophoneDspSnapshot> {
        update.validate()?;
        let mut dsp = self.dsp.write().await;
        match update {
            MicrophoneDspUpdate::Equalizer(state) => dsp.equalizer = state,
            MicrophoneDspUpdate::Compressor(state) => dsp.compressor = state,
            MicrophoneDspUpdate::Expander(state) => dsp.expander = state,
            MicrophoneDspUpdate::NoiseSuppression(state) => dsp.noise_suppression = state,
            MicrophoneDspUpdate::EnhancementSuite(state) => dsp.enhancement_suite = state,
            MicrophoneDspUpdate::HeadphoneEqualizer(state) => dsp.headphone_equalizer = state,
        }
        Ok(dsp.clone())
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

fn mock_microphone_dsp() -> MicrophoneDspSnapshot {
    let equalizer_profile = |mode| EqualizerProfile {
        mode,
        bands: (1..=8)
            .map(|band| EqualizerBandState {
                band,
                band_type: EqualizerBandType::Bell,
                gain_db: if band == 2 { -2.5 } else { 0.0 },
                frequency_hz: [
                    80.0, 160.0, 320.0, 640.0, 1_250.0, 2_500.0, 5_000.0, 10_000.0,
                ][(band - 1) as usize],
                q: 1.0,
                enabled: true,
            })
            .collect(),
    };
    let compressor_profile = |mode| CompressorProfile {
        mode,
        enabled: true,
        threshold_db: -18.0,
        ratio: 3.0,
        attack_ms: 10.0,
        release_ms: 120.0,
        makeup_gain_db: 3.0,
    };
    let expander_profile = |mode| ExpanderProfile {
        mode,
        enabled: true,
        threshold_db: -48.0,
        ratio: 2.0,
        attack_ms: 10.0,
        release_ms: 180.0,
    };
    MicrophoneDspSnapshot {
        equalizer: EqualizerState {
            active_mode: DspMode::Advanced,
            simple: equalizer_profile(DspMode::Simple),
            advanced: equalizer_profile(DspMode::Advanced),
        },
        compressor: CompressorState {
            active_mode: DspMode::Advanced,
            simple: compressor_profile(DspMode::Simple),
            advanced: compressor_profile(DspMode::Advanced),
        },
        expander: ExpanderState {
            active_mode: DspMode::Advanced,
            simple: expander_profile(DspMode::Simple),
            advanced: expander_profile(DspMode::Advanced),
        },
        noise_suppression: NoiseSuppressionState {
            enabled: true,
            style: NoiseSuppressionStyle::Adaptive,
            amount_percent: 70.0,
            sensitivity_db: -85.0,
            adapt_time_ms: 1_000.0,
        },
        enhancement_suite: EnhancementSuiteState {
            bass: BassEnhancementState {
                enabled: false,
                preset: 0,
                amount: 0.0,
                drive: 0.0,
                mix_percent: 0.0,
                attack_ms: 10.0,
                release_ms: 250.0,
                threshold_db: -27.0,
                knee: 2.0,
                makeup_gain_db: 0.0,
                ratio: 4.0,
                cutoff_hz: 100.0,
                q: 0.7,
                lower_cutoff_hz: 40.0,
                lower_q: 0.2,
            },
            de_esser: DeEsserState {
                enabled: true,
                amount_percent: 35.0,
            },
            exciter: ExciterState {
                enabled: false,
                amount_percent: 0.0,
                frequency_hz: 3_000.0,
            },
        },
        headphone_equalizer: HeadphoneEqualizerState {
            bands: vec![
                HeadphoneEqBandState {
                    band: HeadphoneEqBand::Bass,
                    enabled: true,
                    amount_db: 1.5,
                },
                HeadphoneEqBandState {
                    band: HeadphoneEqBand::Mids,
                    enabled: true,
                    amount_db: 0.0,
                },
                HeadphoneEqBandState {
                    band: HeadphoneEqBand::Treble,
                    enabled: true,
                    amount_db: 2.0,
                },
            ],
        },
    }
}

pub struct MockMixerBackend {
    state: Arc<RwLock<MixerSnapshot>>,
}

impl Default for MockMixerBackend {
    fn default() -> Self {
        let channel = |id: &str, name: &str, colour: &str, apps: &[&str]| MixerChannel {
            id: id.into(),
            meter_id: format!("mock-source-{id}"),
            name: name.into(),
            colour: colour.into(),
            personal_volume: 72,
            audience_volume: 68,
            mute_state: MuteState::Unmuted,
            applications: apps.iter().map(|value| (*value).into()).collect(),
            source_kind: if id == "microphone" || id == "game" {
                MixerSourceKind::Physical
            } else {
                MixerSourceKind::Virtual
            },
            volumes_linked: true,
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
                        meter_id: "mock-target-headphones".into(),
                        name: "Headphones".into(),
                        mix: MixBus::Personal,
                        volume: 62,
                        muted: false,
                    },
                    MixerTarget {
                        id: "voice-chat-mic".into(),
                        meter_id: "mock-target-voice-chat".into(),
                        name: "Voice Chat Mic".into(),
                        mix: MixBus::Audience,
                        volume: 100,
                        muted: false,
                    },
                    MixerTarget {
                        id: "vod-track".into(),
                        meter_id: "mock-target-vod".into(),
                        name: "VOD Track".into(),
                        mix: MixBus::Audience,
                        volume: 100,
                        muted: false,
                    },
                    MixerTarget {
                        id: "audience-mix".into(),
                        meter_id: "mock-target-audience".into(),
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
                applications: vec![
                    MixerApplication {
                        process: "discord".into(),
                        name: "Discord".into(),
                        title: Some("Voice chat".into()),
                        channel_id: Some("chat".into()),
                    },
                    MixerApplication {
                        process: "spotify".into(),
                        name: "Spotify".into(),
                        title: None,
                        channel_id: Some("music".into()),
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

    async fn set_target_volume(&self, target_id: &str, volume: u8) -> BridgeResult<()> {
        if volume > 100 {
            return Err(BridgeError::InvalidValue(
                "volume must be between 0 and 100".into(),
            ));
        }
        let mut state = self.state.write().await;
        let target = state
            .targets
            .iter_mut()
            .find(|target| target.id == target_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown target: {target_id}")))?;
        target.volume = volume;
        Ok(())
    }

    async fn set_volume_linked(&self, channel_id: &str, linked: bool) -> BridgeResult<()> {
        let mut state = self.state.write().await;
        let channel = state
            .channels
            .iter_mut()
            .find(|channel| channel.id == channel_id)
            .ok_or_else(|| BridgeError::InvalidValue(format!("unknown channel: {channel_id}")))?;
        channel.volumes_linked = linked;
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

    async fn set_application_route(
        &self,
        process: &str,
        application: &str,
        channel_id: Option<&str>,
    ) -> BridgeResult<()> {
        let mut state = self.state.write().await;
        if let Some(channel_id) = channel_id
            && !state
                .channels
                .iter()
                .any(|channel| channel.id == channel_id)
        {
            return Err(BridgeError::InvalidValue(format!(
                "unknown channel: {channel_id}"
            )));
        }
        let app = state
            .applications
            .iter_mut()
            .find(|item| item.process == process && item.name == application)
            .ok_or_else(|| {
                BridgeError::InvalidValue(format!(
                    "unknown mixer application: {process}/{application}"
                ))
            })?;
        app.channel_id = channel_id.map(str::to_owned);
        for channel in &mut state.channels {
            channel.applications.retain(|name| name != application);
            if Some(channel.id.as_str()) == channel_id {
                channel.applications.push(application.to_owned());
                channel.applications.sort();
                channel.applications.dedup();
            }
        }
        Ok(())
    }
}
