use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendStatus {
    Connected,
    Disconnected,
    Mock,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StudioIdentity {
    pub status: BackendStatus,
    pub error: Option<String>,
    pub product: String,
    pub serial: Option<String>,
    pub firmware: Option<String>,
    pub usb_port: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MicrophoneState {
    pub gain_db: u8,
    pub phantom_power: bool,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeadphoneState {
    pub volume: u8,
    pub mic_monitor: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkChannel {
    System,
    Link1,
    Link2,
    Link3,
    Link4,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkedApplication {
    pub name: String,
    pub channel: LinkChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StudioSnapshot {
    pub identity: StudioIdentity,
    pub microphone: MicrophoneState,
    pub headphones: HeadphoneState,
    pub linked_applications: Vec<LinkedApplication>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MuteState {
    Unmuted,
    MutedAll,
    MutedAudience,
    MutedPersonal,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MixerSourceKind {
    Physical,
    Virtual,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerChannel {
    pub id: String,
    pub meter_id: String,
    pub name: String,
    pub colour: String,
    pub personal_volume: u8,
    pub audience_volume: u8,
    pub mute_state: MuteState,
    pub applications: Vec<String>,
    pub source_kind: MixerSourceKind,
    pub volumes_linked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerTarget {
    pub id: String,
    pub meter_id: String,
    pub name: String,
    pub mix: MixBus,
    pub volume: u8,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerRoute {
    pub source_id: String,
    pub target_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerApplication {
    pub process: String,
    pub name: String,
    pub title: Option<String>,
    pub channel_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerDeviceChoice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerSnapshot {
    pub status: BackendStatus,
    pub error: Option<String>,
    pub engine: String,
    pub channels: Vec<MixerChannel>,
    pub targets: Vec<MixerTarget>,
    pub routes: Vec<MixerRoute>,
    pub applications: Vec<MixerApplication>,
    #[serde(default)]
    pub default_input: Option<String>,
    #[serde(default)]
    pub default_output: Option<String>,
    #[serde(default)]
    pub default_inputs: Vec<MixerDeviceChoice>,
    #[serde(default)]
    pub default_outputs: Vec<MixerDeviceChoice>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppSnapshot {
    pub studio: StudioSnapshot,
    pub mixer: MixerSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMicrophoneRequest {
    pub gain_db: Option<u8>,
    pub phantom_power: Option<bool>,
    pub muted: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLinkAssignmentRequest {
    pub application: String,
    pub channel: LinkChannel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MixBus {
    Personal,
    Audience,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetVolumeRequest {
    pub channel_id: String,
    pub mix: MixBus,
    pub volume: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetTargetVolumeRequest {
    pub target_id: String,
    pub volume: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetDefaultDeviceRequest {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetVolumeLinkedRequest {
    pub channel_id: String,
    pub linked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMuteRequest {
    pub channel_id: String,
    pub state: MuteState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetRouteRequest {
    pub source_id: String,
    pub target_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMixerSourceRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveMixerSourceRequest {
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MixerProfileRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerProfileSummary {
    pub name: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerProfilesResponse {
    pub profiles: Vec<MixerProfileSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMixerApplicationRequest {
    pub process: String,
    pub name: String,
    pub channel_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("backend unavailable: {0}")]
    BackendUnavailable(String),
    #[error("backend error: {0}")]
    Backend(String),
}

pub type BridgeResult<T> = Result<T, BridgeError>;
