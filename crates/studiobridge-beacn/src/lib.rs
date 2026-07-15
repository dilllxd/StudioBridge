mod dsp;

use async_trait::async_trait;
use beacn_lib::audio::messages::Message;
use beacn_lib::audio::messages::headphones::{HeadphoneTypes, Headphones};
use beacn_lib::audio::messages::mic_setup::{MicSetup, StudioMicGain};
use beacn_lib::audio::{
    BeacnAudioDevice, LinkChannel as BeacnLinkChannel, LinkedApp, open_audio_device,
};
use beacn_lib::manager::get_beacn_studio_devices;
use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use studiobridge_core::{
    BackendStatus, BridgeError, BridgeResult, DspWriteModule, HeadphoneOutputMode, HeadphoneState,
    LinkChannel, LinkedApplication, MicrophoneDspSnapshot, MicrophoneDspUpdate, MicrophoneState,
    SetMicrophoneRequest, StudioBackend, StudioIdentity, StudioSnapshot,
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
    link_control_enabled: bool,
    enabled_dsp_writes: HashSet<DspWriteModule>,
    leased_dsp_writes: DspWriteGate,
}

#[derive(Clone, Default)]
pub struct DspWriteGate {
    lease: Arc<Mutex<Option<(DspWriteModule, Instant)>>>,
}

impl DspWriteGate {
    pub fn arm_exclusive(&self, module: DspWriteModule, duration: Duration) -> BridgeResult<()> {
        let mut lease = self
            .lease
            .lock()
            .map_err(|_| BridgeError::Backend("DSP write gate lock was poisoned".into()))?;
        *lease = Some((module, Instant::now() + duration));
        Ok(())
    }

    pub fn disarm(&self) -> BridgeResult<()> {
        let mut lease = self
            .lease
            .lock()
            .map_err(|_| BridgeError::Backend("DSP write gate lock was poisoned".into()))?;
        *lease = None;
        Ok(())
    }

    pub fn enabled_module(&self) -> BridgeResult<Option<DspWriteModule>> {
        Ok(self.active_lease()?.map(|(module, _)| module))
    }

    pub fn active_lease(&self) -> BridgeResult<Option<(DspWriteModule, Duration)>> {
        let mut lease = self
            .lease
            .lock()
            .map_err(|_| BridgeError::Backend("DSP write gate lock was poisoned".into()))?;
        if lease.is_some_and(|(_, expires)| Instant::now() >= expires) {
            *lease = None;
        }
        Ok(lease
            .map(|(module, expires)| (module, expires.saturating_duration_since(Instant::now()))))
    }
}

impl BeacnStudioBackend {
    pub fn spawn(
        allow_writes: bool,
        link_control_enabled: bool,
        enabled_dsp_writes: HashSet<DspWriteModule>,
        leased_dsp_writes: DspWriteGate,
    ) -> BridgeResult<Self> {
        let (commands, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("studiobridge-beacn-usb".into())
            .spawn(move || device_worker(receiver, link_control_enabled))
            .map_err(|error| BridgeError::Backend(error.to_string()))?;
        Ok(Self {
            commands,
            allow_writes,
            link_control_enabled,
            enabled_dsp_writes,
            leased_dsp_writes,
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

    async fn dsp_snapshot_request(&self) -> BridgeResult<MicrophoneDspSnapshot> {
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(DeviceCommand::DspSnapshot(response_tx))
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

    async fn microphone_dsp_snapshot(&self) -> BridgeResult<MicrophoneDspSnapshot> {
        self.dsp_snapshot_request().await
    }

    async fn set_microphone_dsp(
        &self,
        update: MicrophoneDspUpdate,
    ) -> BridgeResult<MicrophoneDspSnapshot> {
        update.validate()?;
        self.ensure_dsp_write_enabled(update.module())?;
        let (response_tx, response_rx) = oneshot::channel();
        self.commands
            .send(DeviceCommand::SetDsp(update, response_tx))
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?;
        response_rx
            .await
            .map_err(|_| BridgeError::BackendUnavailable("USB worker stopped".into()))?
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
        self.ensure_link_control_enabled()?;
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

    fn ensure_link_control_enabled(&self) -> BridgeResult<()> {
        if self.link_control_enabled || self.allow_writes {
            Ok(())
        } else {
            Err(BridgeError::Backend(
                "Link control is disabled; restart with --enable-link-host after read-only validation"
                    .into(),
            ))
        }
    }

    fn ensure_dsp_write_enabled(&self, module: DspWriteModule) -> BridgeResult<()> {
        if self.enabled_dsp_writes.contains(&module)
            || self.leased_dsp_writes.enabled_module()? == Some(module)
        {
            Ok(())
        } else {
            Err(BridgeError::Backend(format!(
                "{} writes are disabled; enable only that DSP module after read-only validation",
                module.as_str()
            )))
        }
    }
}

const LINK_HEARTBEAT: [u8; 4] = [0x00, 0x00, 0x00, 0xAC];
const LINK_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const LINK_HEARTBEAT_TIMEOUT: Duration = Duration::from_millis(500);

enum DeviceCommand {
    Snapshot(oneshot::Sender<BridgeResult<StudioSnapshot>>),
    DspSnapshot(oneshot::Sender<BridgeResult<MicrophoneDspSnapshot>>),
    SetDsp(
        MicrophoneDspUpdate,
        oneshot::Sender<BridgeResult<MicrophoneDspSnapshot>>,
    ),
    SetMicrophone(SetMicrophoneRequest, oneshot::Sender<BridgeResult<()>>),
    SetLink(String, LinkChannel, oneshot::Sender<BridgeResult<()>>),
}

fn device_worker(receiver: Receiver<DeviceCommand>, link_control_enabled: bool) {
    let mut device: Option<Box<dyn BeacnAudioDevice>> = None;
    let mut last_heartbeat = Instant::now() - LINK_HEARTBEAT_INTERVAL;
    loop {
        let command = if link_control_enabled {
            match receiver.recv_timeout(Duration::from_millis(250)) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };

        let Some(command) = command else {
            if device.is_none() {
                device = connect().ok();
            }
            if device.is_some()
                && last_heartbeat.elapsed() >= LINK_HEARTBEAT_INTERVAL
                && with_device(&mut device, send_link_heartbeat).is_ok()
            {
                last_heartbeat = Instant::now();
            }
            continue;
        };
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
            DeviceCommand::DspSnapshot(response) => {
                let result = with_device(&mut device, dsp::read_microphone_dsp);
                let _ = response.send(result);
            }
            DeviceCommand::SetDsp(update, response) => {
                let result = with_device(&mut device, |device| {
                    dsp::write_microphone_dsp(device, &update)
                });
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

        if link_control_enabled
            && device.is_some()
            && last_heartbeat.elapsed() >= LINK_HEARTBEAT_INTERVAL
            && with_device(&mut device, send_link_heartbeat).is_ok()
        {
            last_heartbeat = Instant::now();
        }
    }
}

fn send_link_heartbeat(device: &dyn BeacnAudioDevice) -> BridgeResult<()> {
    let written = device
        .get_usb_handle()
        .write_bulk(0x03, &LINK_HEARTBEAT, LINK_HEARTBEAT_TIMEOUT)
        .map_err(|error| BridgeError::BackendUnavailable(format!("BEACN USB error: {error}")))?;
    if written != LINK_HEARTBEAT.len() {
        return Err(BridgeError::Backend(format!(
            "short Link heartbeat write: {written}/{} bytes",
            LINK_HEARTBEAT.len()
        )));
    }
    Ok(())
}

impl DeviceCommand {
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Snapshot(response) => response.is_closed(),
            Self::DspSnapshot(response) => response.is_closed(),
            Self::SetDsp(_, response) => response.is_closed(),
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
        DeviceCommand::DspSnapshot(response) => {
            let _ = response.send(Err(error));
        }
        DeviceCommand::SetDsp(_, response) => {
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
    let mic_output_gain_db =
        match execute(device, Message::Headphones(Headphones::GetMicOutputGain))? {
            Message::Headphones(Headphones::MicOutputGain(value)) => value.0,
            response => return Err(unexpected("microphone output gain", response)),
        };
    let channels_linked = match execute(
        device,
        Message::Headphones(Headphones::GetStudioChannelsLinked),
    )? {
        Message::Headphones(Headphones::StudioChannelsLinked(value)) => value,
        response => return Err(unexpected("headphone channel link state", response)),
    };
    let output_mode = match execute(device, Message::Headphones(Headphones::GetHeadphoneType))? {
        Message::Headphones(Headphones::HeadphoneType(value)) => match value {
            HeadphoneTypes::InEarMonitors => HeadphoneOutputMode::InEarMonitors,
            HeadphoneTypes::LineLevel => HeadphoneOutputMode::LineLevel,
            HeadphoneTypes::NormalPower => HeadphoneOutputMode::NormalPower,
            HeadphoneTypes::HighImpedance => HeadphoneOutputMode::HighImpedance,
        },
        response => return Err(unexpected("headphone output mode", response)),
    };
    let driverless_mode =
        match execute(device, Message::Headphones(Headphones::GetStudioDriverless))? {
            Message::Headphones(Headphones::StudioDriverless(value)) => value,
            response => return Err(unexpected("USB2 driverless mode", response)),
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
            driverless_mode,
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
            channels_linked,
            output_mode,
            mic_output_gain_tenths_db: output_gain_to_tenths(mic_output_gain_db),
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

fn output_gain_to_tenths(value: f32) -> u16 {
    (value.clamp(0.0, 12.0) * 10.0).round() as u16
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
    fn dsp_write_gate_is_exclusive_and_can_be_disarmed() {
        let gate = DspWriteGate::default();
        gate.arm_exclusive(DspWriteModule::Equalizer, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            gate.enabled_module().unwrap(),
            Some(DspWriteModule::Equalizer)
        );

        gate.arm_exclusive(DspWriteModule::Compressor, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            gate.enabled_module().unwrap(),
            Some(DspWriteModule::Compressor)
        );

        gate.disarm().unwrap();
        assert_eq!(gate.enabled_module().unwrap(), None);
    }

    #[test]
    fn dsp_write_gate_expires() {
        let gate = DspWriteGate::default();
        gate.arm_exclusive(DspWriteModule::Equalizer, Duration::ZERO)
            .unwrap();
        assert_eq!(gate.enabled_module().unwrap(), None);
    }

    #[test]
    fn decibels_convert_to_ui_percentages() {
        assert_eq!(db_to_percent(-70.0, -70.0, 0.0), 0);
        assert_eq!(db_to_percent(0.0, -70.0, 0.0), 100);
        assert_eq!(db_to_percent(-35.0, -70.0, 0.0), 50);
    }

    #[test]
    fn output_gain_readback_is_bounded_and_keeps_tenths() {
        assert_eq!(output_gain_to_tenths(-1.0), 0);
        assert_eq!(output_gain_to_tenths(6.25), 63);
        assert_eq!(output_gain_to_tenths(12.5), 120);
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
        let backend =
            BeacnStudioBackend::spawn(false, false, HashSet::new(), DspWriteGate::default())
                .unwrap();
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
    async fn link_gate_is_independent_from_general_hardware_writes() {
        let backend =
            BeacnStudioBackend::spawn(false, false, HashSet::new(), DspWriteGate::default())
                .unwrap();
        let error = backend
            .set_link_assignment("Game", LinkChannel::Link1)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("Link control is disabled"));
    }

    #[test]
    fn dsp_write_gates_are_module_scoped() {
        let backend = BeacnStudioBackend::spawn(
            false,
            false,
            HashSet::from([DspWriteModule::HeadphoneEqualizer]),
            DspWriteGate::default(),
        )
        .unwrap();
        backend
            .ensure_dsp_write_enabled(DspWriteModule::HeadphoneEqualizer)
            .unwrap();
        let error = backend
            .ensure_dsp_write_enabled(DspWriteModule::Equalizer)
            .unwrap_err();
        assert!(error.to_string().contains("equalizer writes are disabled"));
    }

    #[test]
    fn link_heartbeat_is_a_zero_payload_companion_packet() {
        assert_eq!(LINK_HEARTBEAT, [0x00, 0x00, 0x00, 0xAC]);
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
