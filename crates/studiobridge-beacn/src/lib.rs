use async_trait::async_trait;
use beacn_lib::audio::messages::Message;
use beacn_lib::audio::messages::headphones::Headphones;
use beacn_lib::audio::messages::mic_setup::{MicSetup, StudioMicGain};
use beacn_lib::audio::{
    BeacnAudioDevice, LinkChannel as BeacnLinkChannel, LinkedApp, open_audio_device,
};
use beacn_lib::manager::get_beacn_studio_devices;
use std::sync::mpsc::{self, Receiver, Sender};
use studiobridge_core::{
    BackendStatus, BridgeError, BridgeResult, HeadphoneState, LinkChannel, LinkedApplication,
    MicrophoneState, SetMicrophoneRequest, StudioBackend, StudioIdentity, StudioSnapshot,
};
use tokio::sync::oneshot;

/// Real BEACN Studio USB1 adapter.
///
/// `beacn-lib` intentionally exposes its device as a non-Send trait object. A
/// dedicated worker thread therefore owns the USB handle for its entire
/// lifetime, and the async daemon communicates with it through typed commands.
pub struct BeacnStudioBackend {
    commands: Sender<DeviceCommand>,
    allow_writes: bool,
}

impl BeacnStudioBackend {
    pub fn spawn(allow_writes: bool) -> BridgeResult<Self> {
        let (commands, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("studiobridge-beacn-usb".into())
            .spawn(move || device_worker(receiver))
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        Ok(Self {
            commands,
            allow_writes,
        })
    }

    async fn snapshot_request(&self) -> BridgeResult<StudioSnapshot> {
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(DeviceCommand::Snapshot(response_tx))
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?;
        response_rx
            .await
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?
    }
}

#[async_trait]
impl StudioBackend for BeacnStudioBackend {
    async fn snapshot(&self) -> BridgeResult<StudioSnapshot> {
        self.snapshot_request().await
    }

    async fn set_microphone(&self, request: SetMicrophoneRequest) -> BridgeResult<()> {
        self.ensure_writes_enabled()?;
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(DeviceCommand::SetMicrophone(request, response_tx))
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?;
        response_rx
            .await
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?
    }

    async fn set_link_assignment(
        &self,
        application: &str,
        channel: LinkChannel,
    ) -> BridgeResult<()> {
        self.ensure_writes_enabled()?;
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(DeviceCommand::SetLink(
                application.to_string(),
                channel,
                response_tx,
            ))
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?;
        response_rx
            .await
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?
    }
}

impl BeacnStudioBackend {
    fn ensure_writes_enabled(&self) -> BridgeResult<()> {
        if self.allow_writes {
            Ok(())
        } else {
            Err(BridgeError::Backend(
                "hardware writes are disabled; restart with --allow-hardware-writes after read-only validation"
                    .into(),
            ))
        }
    }
}

enum DeviceCommand {
    Snapshot(oneshot::Sender<BridgeResult<StudioSnapshot>>),
    SetMicrophone(SetMicrophoneRequest, oneshot::Sender<BridgeResult<()>>),
    SetLink(String, LinkChannel, oneshot::Sender<BridgeResult<()>>),
}

fn device_worker(receiver: Receiver<DeviceCommand>) {
    let mut device: Option<Box<dyn BeacnAudioDevice>> = None;
    while let Ok(command) = receiver.recv() {
        // A service timeout drops the oneshot receiver. Do not let an expired
        // request—especially a write queued behind a stalled USB operation—run
        // later when the device becomes available again.
        if command.is_cancelled() {
            continue;
        }
        if device.is_none() {
            match connect() {
                Ok(connected) => device = Some(connected),
                Err(error) => {
                    reject_command(command, error);
                    continue;
                }
            }
        }
        if command.is_cancelled() {
            continue;
        }

        match command {
            DeviceCommand::Snapshot(response) => {
                let result = with_device(&mut device, read_snapshot);
                let _ = response.send(result);
            }
            DeviceCommand::SetMicrophone(request, response) => {
                let result = with_device(&mut device, |device| set_microphone(device, request));
                let _ = response.send(result);
            }
            DeviceCommand::SetLink(application, channel, response) => {
                let result = with_device(&mut device, |device| {
                    device
                        .set_linked_app(LinkedApp {
                            name: application,
                            channel: to_beacn_channel(channel),
                        })
                        .map_err(beacn_error)
                });
                let _ = response.send(result);
            }
        }
    }
}

impl DeviceCommand {
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Snapshot(response) => response.is_closed(),
            Self::SetMicrophone(_, response) | Self::SetLink(_, _, response) => {
                response.is_closed()
            }
        }
    }
}

fn reject_command(command: DeviceCommand, error: BridgeError) {
    match command {
        DeviceCommand::Snapshot(response) => {
            let _ = response.send(Err(error));
        }
        DeviceCommand::SetMicrophone(_, response) | DeviceCommand::SetLink(_, _, response) => {
            let _ = response.send(Err(error));
        }
    }
}

fn connect() -> BridgeResult<Box<dyn BeacnAudioDevice>> {
    let location = get_beacn_studio_devices()
        .into_iter()
        .next()
        .ok_or_else(|| BridgeError::BackendUnavailable("BEACN Studio USB1 not detected".into()))?;
    open_audio_device(location).map_err(beacn_error)
}

fn with_device<T>(
    device: &mut Option<Box<dyn BeacnAudioDevice>>,
    operation: impl FnOnce(&dyn BeacnAudioDevice) -> BridgeResult<T>,
) -> BridgeResult<T> {
    let Some(open_device) = device.as_deref() else {
        return Err(BridgeError::BackendUnavailable(
            "BEACN Studio USB1 not detected or unavailable".into(),
        ));
    };
    let result = operation(open_device);
    if matches!(result, Err(BridgeError::BackendUnavailable(_))) {
        *device = None;
    }
    result
}

fn read_snapshot(device: &dyn BeacnAudioDevice) -> BridgeResult<StudioSnapshot> {
    let gain = match execute(device, Message::MicSetup(MicSetup::GetStudioMicGain))? {
        Message::MicSetup(MicSetup::StudioMicGain(value)) => value.0 as u8,
        response => return Err(unexpected("Studio microphone gain", response)),
    };
    let phantom = match execute(device, Message::MicSetup(MicSetup::GetStudioPhantomPower))? {
        Message::MicSetup(MicSetup::StudioPhantomPower(value)) => value,
        response => return Err(unexpected("Studio phantom power", response)),
    };
    let headphone_db = match execute(device, Message::Headphones(Headphones::GetHeadphoneLevel))? {
        Message::Headphones(Headphones::HeadphoneLevel(value)) => value.0,
        response => return Err(unexpected("headphone level", response)),
    };
    let monitor_db = match execute(device, Message::Headphones(Headphones::GetStudioMicMonitor))? {
        Message::Headphones(Headphones::StudioMicMonitor(value)) => value.0,
        response => return Err(unexpected("microphone monitor level", response)),
    };
    let linked = device
        .get_linked_app_list()
        .map_err(beacn_error)?
        .unwrap_or_default()
        .into_iter()
        .map(|application| LinkedApplication {
            name: application.name,
            channel: from_beacn_channel(application.channel),
        })
        .collect();

    Ok(StudioSnapshot {
        identity: StudioIdentity {
            status: BackendStatus::Connected,
            error: None,
            product: "BEACN Studio".into(),
            serial: Some(device.get_serial()),
            firmware: Some(device.get_version().to_string()),
            usb_port: "USB1".into(),
        },
        microphone: MicrophoneState {
            gain_db: gain,
            phantom_power: phantom,
            // Hardware mute is represented by PipeWire routing in the alpha.
            muted: false,
        },
        headphones: HeadphoneState {
            volume: db_to_percent(headphone_db, -70.0, 0.0),
            mic_monitor: db_to_percent(monitor_db, -100.0, 6.0),
            muted: headphone_db <= -70.0,
        },
        linked_applications: linked,
    })
}

fn set_microphone(
    device: &dyn BeacnAudioDevice,
    request: SetMicrophoneRequest,
) -> BridgeResult<()> {
    // Validate the complete request before performing any USB write. This
    // prevents a mixed request from changing gain or phantom power and then
    // failing only when it reaches an unsupported hardware-mute field.
    if request.muted.is_some() {
        return Err(BridgeError::InvalidValue(
            "Studio hardware mute is not enabled in the alpha; mute through the mixer".into(),
        ));
    }
    if let Some(gain) = request.gain_db {
        if gain > 69 {
            return Err(BridgeError::InvalidValue(
                "microphone gain must be between 0 and 69 dB".into(),
            ));
        }
        execute(
            device,
            Message::MicSetup(MicSetup::StudioMicGain(StudioMicGain(gain as u32))),
        )?;
    }
    if let Some(phantom) = request.phantom_power {
        execute(
            device,
            Message::MicSetup(MicSetup::StudioPhantomPower(phantom)),
        )?;
    }
    Ok(())
}

fn execute(device: &dyn BeacnAudioDevice, message: Message) -> BridgeResult<Message> {
    device.handle_message(message).map_err(beacn_error)
}

fn beacn_error(error: beacn_lib::BeacnError) -> BridgeError {
    match error {
        beacn_lib::BeacnError::Usb(error) => {
            BridgeError::BackendUnavailable(format!("BEACN USB error: {error}"))
        }
        beacn_lib::BeacnError::Other(error) => BridgeError::Backend(error.to_string()),
    }
}

fn unexpected(label: &str, response: Message) -> BridgeError {
    BridgeError::Backend(format!("unexpected {label} response: {response:?}"))
}

fn db_to_percent(value: f32, minimum: f32, maximum: f32) -> u8 {
    (((value.clamp(minimum, maximum) - minimum) / (maximum - minimum)) * 100.0).round() as u8
}

fn to_beacn_channel(channel: LinkChannel) -> BeacnLinkChannel {
    match channel {
        LinkChannel::System => BeacnLinkChannel::System,
        LinkChannel::Link1 => BeacnLinkChannel::Link1,
        LinkChannel::Link2 => BeacnLinkChannel::Link2,
        LinkChannel::Link3 => BeacnLinkChannel::Link3,
        LinkChannel::Link4 => BeacnLinkChannel::Link4,
    }
}

fn from_beacn_channel(channel: BeacnLinkChannel) -> LinkChannel {
    match channel {
        BeacnLinkChannel::System => LinkChannel::System,
        BeacnLinkChannel::Link1 => LinkChannel::Link1,
        BeacnLinkChannel::Link2 => LinkChannel::Link2,
        BeacnLinkChannel::Link3 => LinkChannel::Link3,
        BeacnLinkChannel::Link4 => LinkChannel::Link4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibels_convert_to_ui_percentages() {
        assert_eq!(db_to_percent(-70.0, -70.0, 0.0), 0);
        assert_eq!(db_to_percent(0.0, -70.0, 0.0), 100);
        assert_eq!(db_to_percent(-35.0, -70.0, 0.0), 50);
    }

    #[test]
    fn link_channels_round_trip() {
        for channel in [
            LinkChannel::System,
            LinkChannel::Link1,
            LinkChannel::Link2,
            LinkChannel::Link3,
            LinkChannel::Link4,
        ] {
            assert_eq!(from_beacn_channel(to_beacn_channel(channel)), channel);
        }
    }

    #[tokio::test]
    async fn write_gate_rejects_requests_before_usb_access() {
        let backend = BeacnStudioBackend::spawn(false).unwrap();
        let error = backend
            .set_microphone(SetMicrophoneRequest {
                gain_db: Some(40),
                phantom_power: None,
                muted: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("hardware writes are disabled"));
    }

    #[tokio::test]
    async fn dropped_response_cancels_a_queued_usb_command() {
        let (response, receiver) = oneshot::channel();
        let command = DeviceCommand::SetMicrophone(
            SetMicrophoneRequest {
                gain_db: Some(40),
                phantom_power: None,
                muted: None,
            },
            response,
        );
        assert!(!command.is_cancelled());
        drop(receiver);
        assert!(command.is_cancelled());
    }
}
