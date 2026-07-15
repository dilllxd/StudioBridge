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
    #[serde(default)]
    pub driverless_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MicrophoneState {
    pub gain_db: u8,
    pub phantom_power: bool,
    pub muted: bool,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HeadphoneOutputMode {
    InEarMonitors,
    #[default]
    LineLevel,
    NormalPower,
    HighImpedance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeadphoneState {
    pub volume: u8,
    pub mic_monitor: u8,
    pub muted: bool,
    #[serde(default)]
    pub channels_linked: bool,
    #[serde(default)]
    pub output_mode: HeadphoneOutputMode,
    #[serde(default)]
    pub mic_output_gain_tenths_db: u16,
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
    #[serde(default)]
    pub attached_devices: Vec<MixerPhysicalDeviceDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerTarget {
    pub id: String,
    pub meter_id: String,
    pub name: String,
    pub mix: MixBus,
    pub volume: u8,
    pub muted: bool,
    #[serde(default)]
    pub attached_devices: Vec<MixerPhysicalDeviceDescriptor>,
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
pub struct MixerPhysicalDeviceDescriptor {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerPhysicalDeviceChoice {
    pub node_id: u32,
    pub name: String,
    pub descriptor: MixerPhysicalDeviceDescriptor,
}

pub fn beacn_link_output_slot(
    name: &str,
    descriptor: &MixerPhysicalDeviceDescriptor,
) -> Option<u8> {
    let text = format!(
        "{} {} {}",
        name,
        descriptor.name.as_deref().unwrap_or_default(),
        descriptor.description.as_deref().unwrap_or_default()
    );
    let compact = text
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if !compact.contains("beacn") {
        return None;
    }
    if compact.contains("line4") || compact.contains("link1") {
        Some(1)
    } else if compact.contains("line3") || compact.contains("link2") {
        Some(2)
    } else if compact.contains("line2") || compact.contains("link3") {
        Some(3)
    } else if compact.contains("line1") || compact.contains("link4") {
        Some(4)
    } else {
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerLinkOutputAssignment {
    pub slot: u8,
    pub node_id: u32,
    pub name: String,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerCopyOutputAssignment {
    pub target_id: String,
    pub output: Option<MixerPhysicalDeviceDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerCopyOutputCommitEvidence {
    /// Schema version for accidental corruption/staleness detection. This is
    /// commit evidence, not a cryptographic authentication mechanism.
    pub version: u8,
    pub assignment: MixerCopyOutputAssignment,
    pub voice_chat_attachments: Vec<MixerPhysicalDeviceDescriptor>,
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
    #[serde(default)]
    pub physical_outputs: Vec<MixerPhysicalDeviceChoice>,
    #[serde(default)]
    pub physical_inputs: Vec<MixerPhysicalDeviceChoice>,
    #[serde(default)]
    pub link_outputs: Vec<MixerLinkOutputAssignment>,
    /// Empty means a legacy snapshot which does not manage copy-output state.
    /// A target entry whose output is `None` explicitly selects Nothing.
    #[serde(default)]
    pub copy_outputs: Vec<MixerCopyOutputAssignment>,
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
pub struct SetTargetMuteRequest {
    pub target_id: String,
    pub muted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetTargetDeviceRequest {
    pub target_id: String,
    pub device_node_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMixerCopyOutputRequest {
    pub target_id: String,
    pub device_node_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MixerCopyOutputWriteResult {
    pub assignment: MixerCopyOutputAssignment,
    pub commit_evidence: MixerCopyOutputCommitEvidence,
    pub verified: bool,
    pub snapshot: MixerSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetSourceDeviceRequest {
    pub channel_id: String,
    pub device_node_id: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetLinkOutputAssignmentRequest {
    pub output_node_id: u32,
    pub target_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_alsa_ucm_line_order_to_beacn_link_slots() {
        for (line, slot) in [(4, 1), (3, 2), (2, 3), (1, 4)] {
            let descriptor = MixerPhysicalDeviceDescriptor {
                name: Some(format!("alsa_output.usb-BEACN_Studio__Line{line}__sink")),
                description: Some(format!("BEACN Studio Line{line}")),
            };
            assert_eq!(beacn_link_output_slot("", &descriptor), Some(slot));
        }
    }

    #[test]
    fn rejects_non_beacn_and_non_link_outputs() {
        let unrelated = MixerPhysicalDeviceDescriptor {
            name: Some("alsa_output.interface_line4".into()),
            description: Some("Interface Line 4".into()),
        };
        let headphones = MixerPhysicalDeviceDescriptor {
            name: Some("alsa_output.beacn_studio_headphones".into()),
            description: Some("BEACN Studio Headphones".into()),
        };
        assert_eq!(beacn_link_output_slot("", &unrelated), None);
        assert_eq!(beacn_link_output_slot("", &headphones), None);
    }

    #[test]
    fn legacy_copy_output_absence_is_distinct_from_explicit_nothing() {
        let legacy: Vec<MixerCopyOutputAssignment> = Vec::new();
        let explicit = vec![MixerCopyOutputAssignment {
            target_id: "voice-chat-mic".into(),
            output: None,
        }];
        assert!(legacy.is_empty());
        assert_eq!(
            explicit,
            vec![MixerCopyOutputAssignment {
                target_id: "voice-chat-mic".into(),
                output: None,
            }]
        );
    }
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
pub struct SetMixerSourceNameRequest {
    pub source_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetMixerSourceColourRequest {
    pub source_id: String,
    pub colour: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoveMixerSourceRequest {
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReorderMixerSourceRequest {
    pub source_id: String,
    pub position: usize,
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
