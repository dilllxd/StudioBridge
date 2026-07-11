use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use studiobridge_core::{
    AppSnapshot, CreateMixerSourceRequest, DspWriteModule, LinkChannel, MicrophoneDspSnapshot,
    MicrophoneDspUpdate, MicrophoneDspWriteResult, MixBus, MuteState, RemoveMixerSourceRequest,
    SetLinkAssignmentRequest, SetMixerApplicationRequest, SetMuteRequest, SetRouteRequest,
    SetTargetVolumeRequest, SetVolumeLinkedRequest, SetVolumeRequest,
};

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeHealth {
    pub ok: bool,
    pub studio_mode: String,
    pub mixer_mode: String,
    pub hardware_writes_enabled: bool,
    pub link_control_enabled: bool,
    #[serde(default)]
    pub dsp_write_modules: Vec<String>,
    pub dsp_write_lease_seconds: Option<u64>,
}

#[derive(Serialize)]
struct ArmDspRequest {
    module: DspWriteModule,
    acknowledgement: &'static str,
    lease_seconds: u64,
}

#[derive(Clone)]
pub struct DaemonClient {
    base_url: String,
    client: Client,
}

impl DaemonClient {
    pub fn localhost() -> Result<Self, reqwest::Error> {
        Ok(Self {
            base_url: "http://127.0.0.1:17840".into(),
            client: Client::builder().timeout(Duration::from_secs(5)).build()?,
        })
    }

    pub fn health(&self) -> Result<RuntimeHealth, reqwest::Error> {
        self.client
            .get(format!("{}/api/health", self.base_url))
            .send()?
            .error_for_status()?
            .json()
    }

    pub fn snapshot(&self) -> Result<AppSnapshot, reqwest::Error> {
        self.client
            .get(format!("{}/api/state", self.base_url))
            .send()?
            .error_for_status()?
            .json()
    }

    pub fn set_volume(
        &self,
        channel_id: &str,
        mix: MixBus,
        volume: u8,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/volume", self.base_url))
            .json(&SetVolumeRequest {
                channel_id: channel_id.into(),
                mix,
                volume,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_target_volume(&self, target_id: &str, volume: u8) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/target-volume", self.base_url))
            .json(&SetTargetVolumeRequest {
                target_id: target_id.into(),
                volume,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_mute(&self, channel_id: &str, state: MuteState) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/mute", self.base_url))
            .json(&SetMuteRequest {
                channel_id: channel_id.into(),
                state,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_volume_linked(&self, channel_id: &str, linked: bool) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/volume-link", self.base_url))
            .json(&SetVolumeLinkedRequest {
                channel_id: channel_id.into(),
                linked,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn microphone_dsp(&self) -> Result<MicrophoneDspSnapshot, reqwest::Error> {
        self.client
            .get(format!("{}/api/studio/microphone-dsp", self.base_url))
            .send()?
            .error_for_status()?
            .json()
    }

    pub fn arm_dsp(
        &self,
        module: DspWriteModule,
        lease_seconds: u64,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/studio/dsp-arm", self.base_url))
            .json(&ArmDspRequest {
                module,
                acknowledgement: "I_UNDERSTAND_THIS_CHANGES_AUDIO",
                lease_seconds,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn disarm_dsp(&self) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/studio/dsp-disarm", self.base_url))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_microphone_dsp(
        &self,
        update: &MicrophoneDspUpdate,
    ) -> Result<MicrophoneDspWriteResult, reqwest::Error> {
        self.client
            .post(format!("{}/api/studio/microphone-dsp", self.base_url))
            .json(update)
            .send()?
            .error_for_status()?
            .json()
    }

    pub fn set_route(
        &self,
        source_id: &str,
        target_id: &str,
        enabled: bool,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/route", self.base_url))
            .json(&SetRouteRequest {
                source_id: source_id.into(),
                target_id: target_id.into(),
                enabled,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn create_source(&self, name: &str) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/source", self.base_url))
            .json(&CreateMixerSourceRequest { name: name.into() })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn remove_source(&self, source_id: &str) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/source/remove", self.base_url))
            .json(&RemoveMixerSourceRequest {
                source_id: source_id.into(),
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_mixer_application(
        &self,
        process: &str,
        name: &str,
        channel_id: Option<String>,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/mixer/application", self.base_url))
            .json(&SetMixerApplicationRequest {
                process: process.into(),
                name: name.into(),
                channel_id,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn set_link_application(
        &self,
        application: &str,
        channel: LinkChannel,
    ) -> Result<(), reqwest::Error> {
        self.client
            .post(format!("{}/api/studio/link-assignment", self.base_url))
            .json(&SetLinkAssignmentRequest {
                application: application.into(),
                channel,
            })
            .send()?
            .error_for_status()?;
        Ok(())
    }
}
