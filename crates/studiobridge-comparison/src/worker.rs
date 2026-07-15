use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::{
    Action, ActionResult, AudioDriver, ComparisonEngine, FakeAudioDriver, MAX_FRAMES, PublicState,
    SAMPLE_RATE,
};

pub const COMMAND_CAPACITY: usize = 32;
const TIMER_INTERVAL_MS: u64 = 100;
const OWNER_POLL_INTERVAL_MS: u64 = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub sequence: u64,
    pub kind: CommandKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DestinationKind {
    LocalMonitor,
    NonLocal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Destination {
    pub id: String,
    pub label: String,
    pub kind: DestinationKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DestinationSnapshot {
    pub generation: u64,
    pub endpoint_generation: u64,
    pub complete: bool,
    pub destinations: Vec<Destination>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DestinationAcknowledgement {
    pub confirmation_id: u64,
    pub snapshot: DestinationSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandKind {
    ProbeAvailability,
    UpdateDestinations(DestinationSnapshot),
    ToggleRecord,
    /// The simulated worker permits playback only against a complete metadata
    /// snapshot. Non-local destinations require an exact, protocol-issued,
    /// one-use acknowledgement. No production worker constructor exists yet.
    TogglePlay {
        destination_acknowledgement: Option<DestinationAcknowledgement>,
    },
    Cancel,
    WorkspaceExited,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendCommandError {
    Busy,
    Disconnected,
}

#[derive(Clone)]
pub struct CommandSender {
    sender: mpsc::SyncSender<Command>,
}

impl CommandSender {
    pub fn try_send(&self, command: Command) -> Result<(), SendCommandError> {
        self.sender.try_send(command).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => SendCommandError::Busy,
            mpsc::TrySendError::Disconnected(_) => SendCommandError::Disconnected,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Controls {
    pub record_enabled: bool,
    pub play_enabled: bool,
    pub record_label: &'static str,
    pub play_label: &'static str,
}

impl Controls {
    fn from_state(state: &PublicState) -> Self {
        match state {
            PublicState::Unavailable { .. } | PublicState::Shutdown => Self {
                record_enabled: false,
                play_enabled: false,
                record_label: "Record comparison",
                play_label: "Play comparison loop",
            },
            PublicState::Empty => Self {
                record_enabled: true,
                play_enabled: false,
                record_label: "Record comparison",
                play_label: "Play comparison loop",
            },
            PublicState::Ready { .. } => Self {
                record_enabled: true,
                play_enabled: true,
                record_label: "Record comparison",
                play_label: "Play comparison loop",
            },
            PublicState::Recording { draft_frames } => Self {
                record_enabled: true,
                play_enabled: *draft_frames >= crate::MIN_COMMIT_FRAMES,
                record_label: "Pause recording",
                play_label: "Play comparison loop",
            },
            PublicState::RecordingPaused { draft_frames } => Self {
                record_enabled: true,
                play_enabled: *draft_frames >= crate::MIN_COMMIT_FRAMES,
                record_label: "Resume recording",
                play_label: "Play comparison loop",
            },
            PublicState::Playing { .. } => Self {
                record_enabled: true,
                play_enabled: true,
                record_label: "Record comparison",
                play_label: "Stop comparison playback",
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub revision: u64,
    pub state: PublicState,
    pub timer_tenths: u32,
    pub controls: Controls,
    pub reason: Option<String>,
    pub mode: ComparisonMode,
    pub availability_label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonMode {
    Simulated,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    CommandRejected(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalEvent {
    Snapshot(Snapshot),
    DestinationConfirmationRequired {
        revision: u64,
        confirmation_id: u64,
        snapshot: DestinationSnapshot,
    },
    Diagnostic {
        revision: u64,
        kind: DiagnosticKind,
    },
    ShutdownComplete {
        revision: u64,
    },
}

#[derive(Clone, Default)]
struct LatestSnapshot(Arc<Mutex<Option<Snapshot>>>);

impl LatestSnapshot {
    fn publish(&self, snapshot: Snapshot) {
        *self.0.lock().expect("latest snapshot lock poisoned") = Some(snapshot);
    }

    fn take(&self) -> Option<Snapshot> {
        self.0.lock().expect("latest snapshot lock poisoned").take()
    }
}

struct ProtocolCore<D: AudioDriver> {
    engine: ComparisonEngine<D>,
    latest_timer: LatestSnapshot,
    terminal: mpsc::Sender<TerminalEvent>,
    last_timer_publish_ms: Option<u64>,
    last_command_sequence: Option<u64>,
    destinations: Option<DestinationSnapshot>,
    pending_confirmation: Option<(u64, DestinationSnapshot)>,
    next_confirmation_id: Option<u64>,
    destination_fault: Option<String>,
}

impl<D: AudioDriver> ProtocolCore<D> {
    fn simulated(
        driver: D,
        endpoint_generation: u64,
        latest_timer: LatestSnapshot,
        terminal: mpsc::Sender<TerminalEvent>,
    ) -> Self {
        let core = Self {
            engine: ComparisonEngine::new_simulated(driver, endpoint_generation),
            latest_timer,
            terminal,
            last_timer_publish_ms: None,
            last_command_sequence: None,
            destinations: None,
            pending_confirmation: None,
            next_confirmation_id: Some(1),
            destination_fault: None,
        };
        core.publish_semantic_snapshot();
        core
    }

    fn process(&mut self, command: Command) -> bool {
        if matches!(command.kind, CommandKind::Shutdown) {
            self.force_shutdown();
            return true;
        }
        if self
            .last_command_sequence
            .is_some_and(|last| command.sequence <= last)
        {
            return false;
        }
        self.last_command_sequence = Some(command.sequence);
        let before = self.semantic_signature();
        let result = match command.kind {
            CommandKind::ProbeAvailability => {
                self.publish_semantic_snapshot();
                return false;
            }
            CommandKind::UpdateDestinations(snapshot) => {
                let diagnostic = self.update_destinations(snapshot);
                if before != self.semantic_signature() {
                    self.publish_semantic_snapshot();
                }
                if let Some(diagnostic) = diagnostic {
                    self.emit_diagnostic(diagnostic);
                }
                return false;
            }
            CommandKind::ToggleRecord => {
                self.pending_confirmation = None;
                self.engine.handle(command.sequence, Action::ToggleRecord)
            }
            CommandKind::TogglePlay {
                destination_acknowledgement,
            } => {
                if !self.authorize_playback(destination_acknowledgement) {
                    if before != self.semantic_signature() {
                        self.publish_semantic_snapshot();
                    }
                    return false;
                }
                self.engine.handle(command.sequence, Action::TogglePlay)
            }
            CommandKind::Cancel => {
                self.pending_confirmation = None;
                self.engine.handle(command.sequence, Action::Cancel)
            }
            CommandKind::WorkspaceExited => {
                self.pending_confirmation = None;
                self.engine
                    .handle(command.sequence, Action::WorkspaceExited)
            }
            CommandKind::Shutdown => unreachable!("shutdown handled before sequence filtering"),
        };

        let changed = before != self.semantic_signature();
        if changed {
            self.publish_semantic_snapshot();
        }
        match result {
            ActionResult::Transitioned => {
                if !changed {
                    self.publish_semantic_snapshot();
                }
            }
            ActionResult::IgnoredDuplicate => {}
            ActionResult::Rejected(reason) => {
                self.emit_diagnostic(DiagnosticKind::CommandRejected(reason));
            }
        }
        false
    }

    fn emit_diagnostic(&self, kind: DiagnosticKind) {
        // The unbounded terminal channel intentionally never drops an accepted
        // error. It contains metadata only and cannot carry samples.
        let _ = self.terminal.send(TerminalEvent::Diagnostic {
            revision: self.engine.revision(),
            kind,
        });
    }

    fn snapshot(&self) -> Snapshot {
        let state = self.engine.state();
        let mut reason = match &state {
            PublicState::Unavailable { reason } => Some(reason.clone()),
            _ => None,
        };
        let mut controls = Controls::from_state(&state);
        if controls.play_enabled && !matches!(state, PublicState::Playing { .. }) {
            match self.destination_status() {
                Ok(true) => {
                    reason = Some("Confirmation required for non-local destinations".into());
                }
                Ok(false) => {}
                Err(blocked) => {
                    controls.play_enabled = false;
                    reason = Some(blocked);
                }
            }
        }
        Snapshot {
            revision: self.engine.revision(),
            timer_tenths: timer_tenths(&state),
            controls,
            state,
            reason,
            mode: ComparisonMode::Simulated,
            availability_label: "Simulated comparison",
        }
    }

    fn publish_semantic_snapshot(&self) {
        let _ = self.terminal.send(TerminalEvent::Snapshot(self.snapshot()));
    }

    fn semantic_signature(
        &self,
    ) -> (
        u64,
        std::mem::Discriminant<PublicState>,
        Controls,
        Option<String>,
        Option<DestinationSnapshot>,
    ) {
        let snapshot = self.snapshot();
        (
            snapshot.revision,
            std::mem::discriminant(&snapshot.state),
            snapshot.controls,
            snapshot.reason,
            self.destinations.clone(),
        )
    }

    fn destination_status(&self) -> Result<bool, String> {
        if let Some(fault) = &self.destination_fault {
            return Err(fault.clone());
        }
        let Some(snapshot) = &self.destinations else {
            return Err("Comparison unavailable: destination set is unknown".into());
        };
        if !snapshot.complete {
            return Err("Comparison unavailable: destination set is incomplete".into());
        }
        if !snapshot
            .destinations
            .iter()
            .any(|destination| destination.kind == DestinationKind::LocalMonitor)
        {
            return Err("Comparison unavailable: no local audible monitor".into());
        }
        let needs_confirmation = snapshot
            .destinations
            .iter()
            .any(|destination| destination.kind == DestinationKind::NonLocal);
        if needs_confirmation
            && self.pending_confirmation.is_none()
            && self.next_confirmation_id.is_none()
        {
            return Err("Comparison unavailable: confirmation ID space exhausted".into());
        }
        Ok(needs_confirmation)
    }

    fn update_destinations(&mut self, snapshot: DestinationSnapshot) -> Option<DiagnosticKind> {
        if let Some(reason) = Self::validate_destination_snapshot(&snapshot) {
            self.fail_destination_policy(reason.clone());
            return Some(DiagnosticKind::CommandRejected(reason));
        }
        if let Some(current) = &self.destinations {
            if snapshot.endpoint_generation < current.endpoint_generation {
                let reason = "endpoint generation regressed".to_string();
                self.fail_destination_policy(format!("Comparison unavailable: {reason}"));
                return Some(DiagnosticKind::CommandRejected(reason));
            }
            if snapshot.generation < current.generation {
                return Some(DiagnosticKind::CommandRejected(
                    "stale destination snapshot ignored".into(),
                ));
            }
            if snapshot.generation == current.generation {
                if snapshot != *current {
                    self.fail_destination_policy(
                        "Comparison unavailable: destination generation content mismatch".into(),
                    );
                    return Some(DiagnosticKind::CommandRejected(
                        "destination generation was reused with different content".into(),
                    ));
                }
                return None;
            }

            let endpoint_changed = snapshot.endpoint_generation != current.endpoint_generation;
            let playback_destinations_changed =
                matches!(self.engine.state(), PublicState::Playing { .. });
            if (endpoint_changed || playback_destinations_changed)
                && let Some(token) = self.engine.active_token()
            {
                self.engine.stream_error(
                    token,
                    "comparison destinations changed during an active stream",
                );
            }
        }
        self.pending_confirmation = None;
        self.destination_fault = None;
        self.destinations = Some(snapshot);
        None
    }

    fn validate_destination_snapshot(snapshot: &DestinationSnapshot) -> Option<String> {
        if !snapshot.complete {
            return None;
        }
        if snapshot.destinations.is_empty() {
            return Some("complete destination snapshot cannot be empty".into());
        }
        if snapshot
            .destinations
            .iter()
            .any(|destination| destination.id.trim().is_empty())
        {
            return Some("complete destination snapshot contains a blank ID".into());
        }
        let mut ids = HashSet::with_capacity(snapshot.destinations.len());
        if snapshot
            .destinations
            .iter()
            .any(|destination| !ids.insert(destination.id.as_str()))
        {
            return Some("complete destination snapshot contains duplicate IDs".into());
        }
        None
    }

    fn fail_destination_policy(&mut self, reason: String) {
        self.destination_fault = Some(reason.clone());
        self.pending_confirmation = None;
        if let Some(token) = self.engine.active_token() {
            self.engine.stream_error(token, reason);
        }
    }

    fn authorize_playback(&mut self, acknowledgement: Option<DestinationAcknowledgement>) -> bool {
        if matches!(self.engine.state(), PublicState::Playing { .. }) {
            if acknowledgement.is_some() {
                self.emit_diagnostic(DiagnosticKind::CommandRejected(
                    "destination acknowledgement cannot be replayed".into(),
                ));
                return false;
            }
            return true;
        }
        if !matches!(
            self.engine.state(),
            PublicState::Ready { .. }
                | PublicState::Recording { .. }
                | PublicState::RecordingPaused { .. }
        ) {
            return true;
        }
        let needs_confirmation = match self.destination_status() {
            Ok(needs_confirmation) => needs_confirmation,
            Err(reason) => {
                self.emit_diagnostic(DiagnosticKind::CommandRejected(reason));
                return false;
            }
        };
        if !needs_confirmation {
            if acknowledgement.is_some() {
                self.emit_diagnostic(DiagnosticKind::CommandRejected(
                    "destination acknowledgement was not requested".into(),
                ));
                return false;
            }
            return true;
        }

        let current = self.destinations.clone().expect("status requires snapshot");
        if let Some(acknowledgement) = acknowledgement {
            if self.pending_confirmation.as_ref()
                == Some(&(
                    acknowledgement.confirmation_id,
                    acknowledgement.snapshot.clone(),
                ))
                && acknowledgement.snapshot == current
            {
                self.pending_confirmation = None;
                return true;
            }
            self.emit_diagnostic(DiagnosticKind::CommandRejected(
                "destination acknowledgement is stale or does not match".into(),
            ));
        }

        let confirmation_id = match &self.pending_confirmation {
            Some((confirmation_id, snapshot)) if *snapshot == current => *confirmation_id,
            _ => {
                let Some(confirmation_id) = self.next_confirmation_id.take() else {
                    self.emit_diagnostic(DiagnosticKind::CommandRejected(
                        "confirmation ID space exhausted".into(),
                    ));
                    return false;
                };
                self.next_confirmation_id = confirmation_id.checked_add(1);
                self.pending_confirmation = Some((confirmation_id, current.clone()));
                confirmation_id
            }
        };
        let _ = self
            .terminal
            .send(TerminalEvent::DestinationConfirmationRequired {
                revision: self.engine.revision(),
                confirmation_id,
                snapshot: current,
            });
        false
    }

    fn force_shutdown(&mut self) {
        self.engine.shutdown();
        self.publish_semantic_snapshot();
        let _ = self.terminal.send(TerminalEvent::ShutdownComplete {
            revision: self.engine.revision(),
        });
    }

    fn timer_tick(&mut self, now_ms: u64) {
        if !matches!(
            self.engine.state(),
            PublicState::Recording { .. }
                | PublicState::RecordingPaused { .. }
                | PublicState::Playing { .. }
        ) {
            return;
        }
        if self
            .last_timer_publish_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < TIMER_INTERVAL_MS)
        {
            return;
        }
        self.last_timer_publish_ms = Some(now_ms);
        self.latest_timer.publish(self.snapshot());
    }

    fn accept_capture(&mut self, input: &[f32]) {
        let before = self.semantic_signature();
        if let Some(token) = self.engine.active_token() {
            self.engine.accept_capture(token, input);
        }
        if before != self.semantic_signature() {
            self.publish_semantic_snapshot();
        }
    }
}

fn timer_tenths(state: &PublicState) -> u32 {
    let frames = match state {
        PublicState::Recording { draft_frames } | PublicState::RecordingPaused { draft_frames } => {
            MAX_FRAMES.saturating_sub(*draft_frames)
        }
        PublicState::Playing { elapsed_frames, .. } => *elapsed_frames,
        _ => 0,
    };
    ((frames.saturating_mul(10) + SAMPLE_RATE / 2) / SAMPLE_RATE) as u32
}

/// Owns the simulated policy worker and its bounded protocol.
///
/// This type cannot construct a production audio driver. It is Windows-safe
/// interaction scaffolding, not evidence that live-chain playback is valid.
#[derive(Clone, Default)]
struct WorkerTestProbe {
    exited: Arc<AtomicBool>,
    deactivated: Arc<AtomicBool>,
    scrubbed: Arc<AtomicBool>,
}

impl WorkerTestProbe {
    fn record(&self, core: &ProtocolCore<FakeAudioDriver>) {
        self.deactivated.store(
            core.engine
                .driver()
                .calls()
                .contains(&crate::DriverCall::DeactivateAll),
            Ordering::SeqCst,
        );
        self.scrubbed
            .store(core.engine.buffers_are_scrubbed(), Ordering::SeqCst);
        self.exited.store(true, Ordering::SeqCst);
    }
}

pub struct ComparisonWorker {
    commands: CommandSender,
    latest_timer: LatestSnapshot,
    terminal: mpsc::Receiver<TerminalEvent>,
    deferred_terminal: VecDeque<TerminalEvent>,
    owner_teardown: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl ComparisonWorker {
    pub fn spawn_simulated(driver: FakeAudioDriver, endpoint_generation: u64) -> Self {
        Self::spawn_inner(driver, endpoint_generation, None, None, None)
    }

    fn spawn_inner(
        driver: FakeAudioDriver,
        endpoint_generation: u64,
        start_gate: Option<mpsc::Receiver<()>>,
        probe: Option<WorkerTestProbe>,
        test_capture: Option<Box<[f32]>>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (terminal_tx, terminal_rx) = mpsc::channel();
        let (teardown_tx, teardown_rx) = mpsc::channel();
        let latest_timer = LatestSnapshot::default();
        let worker_latest_timer = latest_timer.clone();
        let thread = thread::Builder::new()
            .name("studiobridge-comparison-simulated".into())
            .spawn(move || {
                let mut core = ProtocolCore::simulated(
                    driver,
                    endpoint_generation,
                    worker_latest_timer,
                    terminal_tx,
                );
                if let Some(samples) = test_capture {
                    core.process(Command {
                        sequence: 1,
                        kind: CommandKind::ToggleRecord,
                    });
                    core.accept_capture(&samples);
                }
                if let Some(gate) = start_gate {
                    let _ = gate.recv();
                }
                let started = Instant::now();
                loop {
                    if teardown_rx.try_recv().is_ok() {
                        core.force_shutdown();
                        break;
                    }
                    match command_rx.recv_timeout(Duration::from_millis(OWNER_POLL_INTERVAL_MS)) {
                        Ok(command) => {
                            if core.process(command) {
                                break;
                            }
                            core.timer_tick(started.elapsed().as_millis() as u64);
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            core.timer_tick(started.elapsed().as_millis() as u64);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                if !matches!(core.engine.state(), PublicState::Shutdown) {
                    core.force_shutdown();
                }
                if let Some(probe) = probe {
                    probe.record(&core);
                }
            })
            .expect("failed to spawn simulated comparison worker");
        Self {
            commands: CommandSender { sender: command_tx },
            latest_timer,
            terminal: terminal_rx,
            deferred_terminal: VecDeque::new(),
            owner_teardown: teardown_tx,
            thread: Some(thread),
        }
    }

    pub fn command_sender(&self) -> CommandSender {
        self.commands.clone()
    }

    pub fn take_latest_snapshot(&self) -> Option<Snapshot> {
        self.latest_timer.take()
    }

    pub fn try_terminal_event(&mut self) -> Option<TerminalEvent> {
        self.deferred_terminal
            .pop_front()
            .or_else(|| self.terminal.try_recv().ok())
    }

    pub fn shutdown(&mut self, _sequence: u64, _timeout: Duration) -> bool {
        if self.thread.is_none() {
            return true;
        }
        let _ = self.owner_teardown.send(());
        self.thread
            .take()
            .is_none_or(|handle| handle.join().is_ok())
    }
}

impl Drop for ComparisonWorker {
    fn drop(&mut self) {
        let _ = self.shutdown(u64::MAX, Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MIN_COMMIT_FRAMES;

    #[derive(Default)]
    struct FakeClock(u64);

    impl FakeClock {
        fn advance(&mut self, millis: u64) -> u64 {
            self.0 += millis;
            self.0
        }
    }

    fn core() -> (
        ProtocolCore<FakeAudioDriver>,
        LatestSnapshot,
        mpsc::Receiver<TerminalEvent>,
    ) {
        let latest = LatestSnapshot::default();
        let (terminal_tx, terminal_rx) = mpsc::channel();
        let core =
            ProtocolCore::simulated(FakeAudioDriver::default(), 1, latest.clone(), terminal_tx);
        (core, latest, terminal_rx)
    }

    fn next_semantic(terminal: &mpsc::Receiver<TerminalEvent>) -> Snapshot {
        loop {
            if let TerminalEvent::Snapshot(snapshot) = terminal.recv().unwrap() {
                return snapshot;
            }
        }
    }

    fn next_confirmation(terminal: &mpsc::Receiver<TerminalEvent>) -> (u64, DestinationSnapshot) {
        loop {
            if let TerminalEvent::DestinationConfirmationRequired {
                confirmation_id,
                snapshot,
                ..
            } = terminal.recv().unwrap()
            {
                return (confirmation_id, snapshot);
            }
        }
    }

    fn local_destinations(generation: u64) -> DestinationSnapshot {
        DestinationSnapshot {
            generation,
            endpoint_generation: 1,
            complete: true,
            destinations: vec![Destination {
                id: "headphones".into(),
                label: "Headphones".into(),
                kind: DestinationKind::LocalMonitor,
            }],
        }
    }

    fn non_local_destinations(generation: u64) -> DestinationSnapshot {
        let mut snapshot = local_destinations(generation);
        snapshot.destinations.push(Destination {
            id: "voice-chat".into(),
            label: "Voice Chat Mic".into(),
            kind: DestinationKind::NonLocal,
        });
        snapshot
    }

    fn set_destinations(
        core: &mut ProtocolCore<FakeAudioDriver>,
        sequence: u64,
        snapshot: DestinationSnapshot,
    ) {
        core.process(Command {
            sequence,
            kind: CommandKind::UpdateDestinations(snapshot),
        });
    }

    fn prepare_ready(
        core: &mut ProtocolCore<FakeAudioDriver>,
        terminal: &mpsc::Receiver<TerminalEvent>,
        destinations: DestinationSnapshot,
    ) {
        next_semantic(terminal);
        set_destinations(core, 1, destinations);
        next_semantic(terminal);
        core.process(Command {
            sequence: 2,
            kind: CommandKind::ToggleRecord,
        });
        next_semantic(terminal);
        core.accept_capture(&[0.2; MIN_COMMIT_FRAMES]);
        next_semantic(terminal);
        core.process(Command {
            sequence: 3,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        next_semantic(terminal);
        core.process(Command {
            sequence: 4,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        next_semantic(terminal);
    }

    #[test]
    fn command_queue_is_exactly_bounded_and_reports_busy() {
        let (sender, _receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let sender = CommandSender { sender };
        for sequence in 0..COMMAND_CAPACITY as u64 {
            assert_eq!(
                sender.try_send(Command {
                    sequence,
                    kind: CommandKind::ProbeAvailability,
                }),
                Ok(())
            );
        }
        assert_eq!(
            sender.try_send(Command {
                sequence: 99,
                kind: CommandKind::ProbeAvailability,
            }),
            Err(SendCommandError::Busy)
        );
    }

    #[test]
    fn timer_snapshots_coalesce_to_ten_hz_and_latest_overwrites() {
        let (mut core, latest, _) = core();
        latest.take();
        core.process(Command {
            sequence: 1,
            kind: CommandKind::ToggleRecord,
        });
        latest.take();
        core.accept_capture(&[0.5; SAMPLE_RATE / 2]);
        let mut clock = FakeClock::default();
        core.timer_tick(clock.advance(0));
        let first = latest.take().unwrap();
        assert_eq!(first.timer_tenths, 95);
        core.accept_capture(&[0.5; SAMPLE_RATE / 2]);
        core.timer_tick(clock.advance(50));
        assert!(latest.take().is_none());
        core.timer_tick(clock.advance(50));
        assert_eq!(latest.take().unwrap().timer_tenths, 90);
        core.timer_tick(clock.advance(100));
        core.timer_tick(clock.advance(100));
        assert!(latest.take().is_some());
        assert!(latest.take().is_none());
    }

    #[test]
    fn transitions_are_lossless_and_separate_from_timer_coalescing() {
        let (mut core, latest, terminal) = core();
        let initial = next_semantic(&terminal);
        assert_eq!(initial.mode, ComparisonMode::Simulated);
        assert_eq!(initial.availability_label, "Simulated comparison");
        core.timer_tick(100);
        assert!(latest.take().is_none());
        core.process(Command {
            sequence: 1,
            kind: CommandKind::ToggleRecord,
        });
        core.process(Command {
            sequence: 2,
            kind: CommandKind::ToggleRecord,
        });
        let recording = next_semantic(&terminal);
        let paused = next_semantic(&terminal);
        assert!(matches!(recording.state, PublicState::Recording { .. }));
        assert!(matches!(paused.state, PublicState::RecordingPaused { .. }));
        core.timer_tick(100);
        assert!(latest.take().is_some());
    }

    #[test]
    fn timer_value_comes_from_frames_not_fake_clock_elapsed() {
        let (mut core, latest, _) = core();
        latest.take();
        core.process(Command {
            sequence: 1,
            kind: CommandKind::ToggleRecord,
        });
        latest.take();
        core.accept_capture(&[0.5; SAMPLE_RATE / 2]);
        let mut clock = FakeClock::default();
        core.timer_tick(clock.advance(0));
        assert_eq!(latest.take().unwrap().timer_tenths, 95);
        core.timer_tick(clock.advance(10_000));
        assert_eq!(latest.take().unwrap().timer_tenths, 95);
    }

    #[test]
    fn terminal_errors_are_not_coalesced_or_dropped() {
        let (mut core, _, terminal) = core();
        next_semantic(&terminal);
        for sequence in 1..=8 {
            core.process(Command {
                sequence,
                kind: CommandKind::TogglePlay {
                    destination_acknowledgement: None,
                },
            });
        }
        let events: Vec<_> = terminal
            .try_iter()
            .filter(|event| matches!(event, TerminalEvent::Diagnostic { .. }))
            .collect();
        assert_eq!(events.len(), 8);
        assert!(
            events
                .iter()
                .all(|event| matches!(event, TerminalEvent::Diagnostic { .. }))
        );
    }

    #[test]
    fn unexpected_acknowledgement_cannot_authorize_local_only_playback() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        core.process(Command {
            sequence: 5,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(DestinationAcknowledgement {
                    confirmation_id: 4,
                    snapshot: local_destinations(1),
                }),
            },
        });
        assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
        assert!(matches!(
            terminal.try_recv().unwrap(),
            TerminalEvent::Diagnostic {
                kind: DiagnosticKind::CommandRejected(_),
                ..
            }
        ));
    }

    #[test]
    fn complete_local_only_snapshot_allows_play_without_confirmation() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        core.process(Command {
            sequence: 5,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        assert!(matches!(
            next_semantic(&terminal).state,
            PublicState::Playing { .. }
        ));
        assert!(terminal.try_recv().is_err());
    }

    #[test]
    fn non_local_destinations_require_exact_one_time_confirmation() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        let destinations = non_local_destinations(2);
        set_destinations(&mut core, 5, destinations.clone());
        next_semantic(&terminal);
        core.process(Command {
            sequence: 6,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        let (confirmation_id, required) = next_confirmation(&terminal);
        assert_eq!(required, destinations);
        let acknowledgement = DestinationAcknowledgement {
            confirmation_id,
            snapshot: required,
        };
        core.process(Command {
            sequence: 7,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(acknowledgement.clone()),
            },
        });
        assert!(matches!(
            next_semantic(&terminal).state,
            PublicState::Playing { .. }
        ));
        core.process(Command {
            sequence: 8,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        next_semantic(&terminal);
        core.process(Command {
            sequence: 9,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(acknowledgement),
            },
        });
        assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
        assert!(matches!(
            terminal.recv().unwrap(),
            TerminalEvent::Diagnostic { .. }
        ));
        let (new_confirmation_id, _) = next_confirmation(&terminal);
        assert_ne!(new_confirmation_id, confirmation_id);
    }

    #[test]
    fn mismatched_and_stale_acknowledgements_fail_closed() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        let destinations = non_local_destinations(2);
        set_destinations(&mut core, 5, destinations.clone());
        next_semantic(&terminal);
        core.process(Command {
            sequence: 6,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        let (confirmation_id, _) = next_confirmation(&terminal);
        let mut mismatched = destinations.clone();
        mismatched.destinations[1].label = "Changed label".into();
        core.process(Command {
            sequence: 7,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(DestinationAcknowledgement {
                    confirmation_id,
                    snapshot: mismatched,
                }),
            },
        });
        assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
        assert!(matches!(
            terminal.recv().unwrap(),
            TerminalEvent::Diagnostic { .. }
        ));
        assert_eq!(next_confirmation(&terminal).0, confirmation_id);

        let newer = non_local_destinations(3);
        set_destinations(&mut core, 8, newer.clone());
        next_semantic(&terminal);
        core.process(Command {
            sequence: 9,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(DestinationAcknowledgement {
                    confirmation_id,
                    snapshot: destinations,
                }),
            },
        });
        assert!(matches!(
            terminal.recv().unwrap(),
            TerminalEvent::Diagnostic { .. }
        ));
        let (new_id, required) = next_confirmation(&terminal);
        assert_ne!(new_id, confirmation_id);
        assert_eq!(required, newer);
    }

    #[test]
    fn incomplete_unknown_and_no_local_destination_sets_disable_play() {
        for snapshot in [
            DestinationSnapshot {
                generation: 2,
                endpoint_generation: 1,
                complete: false,
                destinations: local_destinations(1).destinations,
            },
            DestinationSnapshot {
                generation: 2,
                endpoint_generation: 1,
                complete: true,
                destinations: vec![Destination {
                    id: "voice-chat".into(),
                    label: "Voice Chat Mic".into(),
                    kind: DestinationKind::NonLocal,
                }],
            },
        ] {
            let (mut core, _, terminal) = core();
            prepare_ready(&mut core, &terminal, local_destinations(1));
            set_destinations(&mut core, 5, snapshot);
            let blocked = next_semantic(&terminal);
            assert!(!blocked.controls.play_enabled);
            assert!(
                blocked
                    .reason
                    .unwrap()
                    .starts_with("Comparison unavailable:")
            );
            core.process(Command {
                sequence: 6,
                kind: CommandKind::TogglePlay {
                    destination_acknowledgement: None,
                },
            });
            assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
            assert!(matches!(
                terminal.recv().unwrap(),
                TerminalEvent::Diagnostic { .. }
            ));
        }

        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        core.destinations = None;
        core.publish_semantic_snapshot();
        assert!(!next_semantic(&terminal).controls.play_enabled);
    }

    #[test]
    fn endpoint_or_destination_generation_change_stops_active_playback() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        core.process(Command {
            sequence: 5,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        next_semantic(&terminal);
        let mut changed = local_destinations(2);
        changed.endpoint_generation = 2;
        set_destinations(&mut core, 6, changed);
        assert!(matches!(
            next_semantic(&terminal).state,
            PublicState::Unavailable { .. }
        ));
        assert_eq!(core.engine.driver().active_stream_count(), 0);
    }

    #[test]
    fn rapid_confirmation_commands_reuse_one_pending_id_without_starting_audio() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        set_destinations(&mut core, 5, non_local_destinations(2));
        next_semantic(&terminal);
        for sequence in 6..=7 {
            core.process(Command {
                sequence,
                kind: CommandKind::TogglePlay {
                    destination_acknowledgement: None,
                },
            });
        }
        let first = next_confirmation(&terminal).0;
        let second = next_confirmation(&terminal).0;
        assert_eq!(first, second);
        assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
        assert_eq!(core.engine.driver().active_stream_count(), 0);
    }

    #[test]
    fn max_confirmation_id_is_issued_once_then_exhaustion_is_sticky() {
        let (mut core, _, terminal) = core();
        prepare_ready(&mut core, &terminal, local_destinations(1));
        let destinations = non_local_destinations(2);
        set_destinations(&mut core, 5, destinations.clone());
        next_semantic(&terminal);
        core.next_confirmation_id = Some(u64::MAX);
        core.process(Command {
            sequence: 6,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        let (confirmation_id, snapshot) = next_confirmation(&terminal);
        assert_eq!(confirmation_id, u64::MAX);
        let acknowledgement = DestinationAcknowledgement {
            confirmation_id,
            snapshot,
        };
        core.process(Command {
            sequence: 7,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(acknowledgement.clone()),
            },
        });
        next_semantic(&terminal);
        core.process(Command {
            sequence: 8,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        let blocked = next_semantic(&terminal);
        assert!(!blocked.controls.play_enabled);
        assert!(blocked.reason.unwrap().contains("ID space exhausted"));
        core.process(Command {
            sequence: 9,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: Some(acknowledgement),
            },
        });
        assert!(matches!(
            terminal.recv().unwrap(),
            TerminalEvent::Diagnostic { .. }
        ));
        assert!(terminal.try_recv().is_err());
        assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
    }

    #[test]
    fn endpoint_generation_regression_faults_inactive_and_active_sessions() {
        for active in [false, true] {
            let (mut core, _, terminal) = core();
            prepare_ready(&mut core, &terminal, local_destinations(1));
            let mut current = local_destinations(2);
            current.endpoint_generation = 5;
            set_destinations(&mut core, 5, current.clone());
            next_semantic(&terminal);
            let mut next_sequence = 6;
            if active {
                core.process(Command {
                    sequence: next_sequence,
                    kind: CommandKind::TogglePlay {
                        destination_acknowledgement: None,
                    },
                });
                next_semantic(&terminal);
                next_sequence += 1;
            }
            let mut regressed = local_destinations(3);
            regressed.endpoint_generation = 4;
            set_destinations(&mut core, next_sequence, regressed);
            let blocked = next_semantic(&terminal);
            if active {
                assert!(matches!(blocked.state, PublicState::Unavailable { .. }));
                assert_eq!(core.engine.driver().active_stream_count(), 0);
            } else {
                assert!(matches!(blocked.state, PublicState::Ready { .. }));
                assert!(!blocked.controls.play_enabled);
            }
            assert!(matches!(
                terminal.recv().unwrap(),
                TerminalEvent::Diagnostic { .. }
            ));
            assert_eq!(core.destinations.as_ref(), Some(&current));
        }
    }

    #[test]
    fn complete_snapshot_rejects_empty_and_ambiguous_duplicate_ids() {
        let invalid_sets = [
            Vec::new(),
            vec![
                Destination {
                    id: "duplicate".into(),
                    label: "Headphones A".into(),
                    kind: DestinationKind::LocalMonitor,
                },
                Destination {
                    id: "duplicate".into(),
                    label: "Headphones B".into(),
                    kind: DestinationKind::LocalMonitor,
                },
            ],
            vec![
                Destination {
                    id: "ambiguous".into(),
                    label: "Headphones".into(),
                    kind: DestinationKind::LocalMonitor,
                },
                Destination {
                    id: "ambiguous".into(),
                    label: "Voice Chat".into(),
                    kind: DestinationKind::NonLocal,
                },
            ],
        ];
        for destinations in invalid_sets {
            let (mut core, _, terminal) = core();
            prepare_ready(&mut core, &terminal, local_destinations(1));
            set_destinations(
                &mut core,
                5,
                DestinationSnapshot {
                    generation: 2,
                    endpoint_generation: 1,
                    complete: true,
                    destinations,
                },
            );
            let blocked = next_semantic(&terminal);
            assert!(!blocked.controls.play_enabled);
            assert!(matches!(blocked.state, PublicState::Ready { .. }));
            assert!(matches!(
                terminal.recv().unwrap(),
                TerminalEvent::Diagnostic { .. }
            ));
        }
    }

    #[test]
    fn complete_snapshot_rejects_empty_and_whitespace_destination_ids() {
        for id in ["", " \t\r\n"] {
            let (mut core, _, terminal) = core();
            prepare_ready(&mut core, &terminal, local_destinations(1));
            set_destinations(
                &mut core,
                5,
                DestinationSnapshot {
                    generation: 2,
                    endpoint_generation: 1,
                    complete: true,
                    destinations: vec![Destination {
                        id: id.into(),
                        label: "Headphones".into(),
                        kind: DestinationKind::LocalMonitor,
                    }],
                },
            );
            let blocked = next_semantic(&terminal);
            assert!(matches!(blocked.state, PublicState::Ready { .. }));
            assert!(!blocked.controls.play_enabled);
            assert!(blocked.reason.unwrap().contains("blank ID"));
            assert!(matches!(
                terminal.recv().unwrap(),
                TerminalEvent::Diagnostic { .. }
            ));
            core.process(Command {
                sequence: 6,
                kind: CommandKind::TogglePlay {
                    destination_acknowledgement: None,
                },
            });
            assert!(matches!(core.engine.state(), PublicState::Ready { .. }));
            assert_eq!(core.engine.driver().active_stream_count(), 0);
        }
    }

    #[test]
    fn same_generation_conflict_publishes_blocked_state_before_diagnostic() {
        for active in [false, true] {
            let (mut core, _, terminal) = core();
            prepare_ready(&mut core, &terminal, local_destinations(1));
            let mut next_sequence = 5;
            if active {
                core.process(Command {
                    sequence: next_sequence,
                    kind: CommandKind::TogglePlay {
                        destination_acknowledgement: None,
                    },
                });
                next_semantic(&terminal);
                next_sequence += 1;
            }
            let mut conflicting = local_destinations(1);
            conflicting.destinations[0].label = "Conflicting label".into();
            set_destinations(&mut core, next_sequence, conflicting);
            let blocked = terminal.recv().unwrap();
            let TerminalEvent::Snapshot(blocked) = blocked else {
                panic!("blocked snapshot must precede diagnostic")
            };
            if active {
                assert!(matches!(blocked.state, PublicState::Unavailable { .. }));
            } else {
                assert!(!blocked.controls.play_enabled);
                assert!(matches!(blocked.state, PublicState::Ready { .. }));
            }
            assert!(matches!(
                terminal.recv().unwrap(),
                TerminalEvent::Diagnostic { .. }
            ));
        }
    }

    #[test]
    fn duplicate_sequences_do_not_publish_or_emit_errors() {
        let (mut core, latest, terminal) = core();
        next_semantic(&terminal);
        core.process(Command {
            sequence: 4,
            kind: CommandKind::ToggleRecord,
        });
        next_semantic(&terminal);
        core.process(Command {
            sequence: 4,
            kind: CommandKind::Cancel,
        });
        assert!(latest.take().is_none());
        assert!(terminal.try_recv().is_err());
    }

    #[test]
    fn controls_follow_authoritative_state_without_optimism() {
        let (mut core, _, terminal) = core();
        let empty = next_semantic(&terminal);
        assert!(empty.controls.record_enabled);
        assert!(!empty.controls.play_enabled);
        set_destinations(&mut core, 1, local_destinations(1));
        next_semantic(&terminal);
        core.process(Command {
            sequence: 2,
            kind: CommandKind::ToggleRecord,
        });
        core.accept_capture(&[0.2; MIN_COMMIT_FRAMES]);
        let recording = next_semantic(&terminal);
        let controls_changed = next_semantic(&terminal);
        assert!(!recording.controls.play_enabled);
        let recording = controls_changed;
        assert!(recording.controls.play_enabled);
        assert_eq!(recording.controls.record_label, "Pause recording");
    }

    #[test]
    fn worker_acknowledges_shutdown_and_joins() {
        let mut worker = ComparisonWorker::spawn_simulated(FakeAudioDriver::default(), 1);
        assert!(worker.shutdown(1, Duration::from_secs(1)));
        assert!(worker.thread.is_none());
        let events: Vec<_> = std::iter::from_fn(|| worker.try_terminal_event()).collect();
        assert!(events.iter().any(
            |event| matches!(event, TerminalEvent::Snapshot(snapshot) if snapshot.state == PublicState::Shutdown)
        ));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TerminalEvent::ShutdownComplete { .. }))
        );
    }

    #[test]
    fn shutdown_preserves_prior_terminal_errors_for_consumer() {
        let mut worker = ComparisonWorker::spawn_simulated(FakeAudioDriver::default(), 1);
        worker
            .commands
            .try_send(Command {
                sequence: 1,
                kind: CommandKind::TogglePlay {
                    destination_acknowledgement: None,
                },
            })
            .unwrap();
        loop {
            let event = worker
                .terminal
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            let is_diagnostic = matches!(event, TerminalEvent::Diagnostic { .. });
            worker.deferred_terminal.push_back(event);
            if is_diagnostic {
                break;
            }
        }
        assert!(worker.shutdown(2, Duration::from_secs(1)));
        assert!(
            std::iter::from_fn(|| worker.try_terminal_event())
                .any(|event| matches!(event, TerminalEvent::Diagnostic { .. }))
        );
    }

    #[test]
    fn rejected_stop_failure_publishes_unavailable_before_diagnostic() {
        let (mut core, _, terminal) = core();
        next_semantic(&terminal);
        set_destinations(&mut core, 1, local_destinations(1));
        next_semantic(&terminal);
        core.process(Command {
            sequence: 2,
            kind: CommandKind::ToggleRecord,
        });
        next_semantic(&terminal);
        core.accept_capture(&[0.2; MIN_COMMIT_FRAMES]);
        next_semantic(&terminal);
        core.engine.driver.fail_next(crate::DriverCall::StopStream);
        core.process(Command {
            sequence: 3,
            kind: CommandKind::TogglePlay {
                destination_acknowledgement: None,
            },
        });
        assert!(matches!(
            terminal.recv().unwrap(),
            TerminalEvent::Snapshot(Snapshot {
                state: PublicState::Unavailable { .. },
                ..
            })
        ));
        assert!(matches!(
            terminal.recv().unwrap(),
            TerminalEvent::Diagnostic { .. }
        ));
    }

    #[test]
    fn rejected_pause_and_resume_failures_publish_authoritative_unavailable() {
        for failure in [
            crate::DriverCall::PauseCapture,
            crate::DriverCall::ResumeCapture,
        ] {
            let (mut core, _, terminal) = core();
            next_semantic(&terminal);
            core.process(Command {
                sequence: 1,
                kind: CommandKind::ToggleRecord,
            });
            next_semantic(&terminal);
            if failure == crate::DriverCall::ResumeCapture {
                core.process(Command {
                    sequence: 2,
                    kind: CommandKind::ToggleRecord,
                });
                next_semantic(&terminal);
            }
            core.engine.driver.fail_next(failure);
            core.process(Command {
                sequence: 3,
                kind: CommandKind::ToggleRecord,
            });
            assert!(matches!(
                next_semantic(&terminal).state,
                PublicState::Unavailable { .. }
            ));
            assert!(matches!(
                terminal.recv().unwrap(),
                TerminalEvent::Diagnostic { .. }
            ));
        }
    }

    #[test]
    fn capture_cap_publishes_auto_ready_semantic_transition() {
        let (mut core, _, terminal) = core();
        next_semantic(&terminal);
        core.process(Command {
            sequence: 1,
            kind: CommandKind::ToggleRecord,
        });
        next_semantic(&terminal);
        core.accept_capture(&vec![0.2; MAX_FRAMES]);
        assert_eq!(
            next_semantic(&terminal).state,
            PublicState::Ready { frames: MAX_FRAMES }
        );
    }

    #[test]
    fn failure_timer_and_shutdown_events_remain_semantically_ordered() {
        let (mut core, latest, terminal) = core();
        next_semantic(&terminal);
        core.process(Command {
            sequence: 1,
            kind: CommandKind::ToggleRecord,
        });
        core.timer_tick(0);
        assert!(latest.take().is_some());
        core.engine
            .driver
            .fail_next(crate::DriverCall::PauseCapture);
        core.process(Command {
            sequence: 2,
            kind: CommandKind::ToggleRecord,
        });
        core.force_shutdown();
        let events: Vec<_> = terminal.try_iter().collect();
        assert!(matches!(events[0], TerminalEvent::Snapshot(_)));
        assert!(matches!(
            events[1],
            TerminalEvent::Snapshot(Snapshot {
                state: PublicState::Unavailable { .. },
                ..
            })
        ));
        assert!(matches!(events[2], TerminalEvent::Diagnostic { .. }));
        assert!(matches!(
            events[3],
            TerminalEvent::Snapshot(Snapshot {
                state: PublicState::Shutdown,
                ..
            })
        ));
        assert!(matches!(events[4], TerminalEvent::ShutdownComplete { .. }));
    }

    #[test]
    fn owner_teardown_ignores_max_sequence_and_zero_timeout() {
        let mut worker = ComparisonWorker::spawn_simulated(FakeAudioDriver::default(), 1);
        let sender = worker.command_sender();
        sender
            .try_send(Command {
                sequence: u64::MAX,
                kind: CommandKind::ProbeAvailability,
            })
            .unwrap();
        thread::sleep(Duration::from_millis(20));
        assert!(worker.shutdown(0, Duration::ZERO));
        assert_eq!(
            sender.try_send(Command {
                sequence: 0,
                kind: CommandKind::ProbeAvailability,
            }),
            Err(SendCommandError::Disconnected)
        );
    }

    #[test]
    fn saturated_queue_and_surviving_clone_cannot_block_owner_teardown() {
        let probe = WorkerTestProbe::default();
        let (gate_tx, gate_rx) = mpsc::channel();
        let mut worker = ComparisonWorker::spawn_inner(
            FakeAudioDriver::default(),
            1,
            Some(gate_rx),
            Some(probe.clone()),
            Some(vec![0.7; MIN_COMMIT_FRAMES].into_boxed_slice()),
        );
        let clone = worker.command_sender();
        for sequence in 2..2 + COMMAND_CAPACITY as u64 {
            clone
                .try_send(Command {
                    sequence,
                    kind: CommandKind::ProbeAvailability,
                })
                .unwrap();
        }
        assert_eq!(
            clone.try_send(Command {
                sequence: 100,
                kind: CommandKind::ProbeAvailability,
            }),
            Err(SendCommandError::Busy)
        );
        let releaser = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            gate_tx.send(()).unwrap();
        });
        assert!(worker.shutdown(0, Duration::ZERO));
        releaser.join().unwrap();
        assert!(probe.exited.load(Ordering::SeqCst));
        assert!(probe.deactivated.load(Ordering::SeqCst));
        assert!(probe.scrubbed.load(Ordering::SeqCst));
        assert_eq!(
            clone.try_send(Command {
                sequence: 101,
                kind: CommandKind::ProbeAvailability,
            }),
            Err(SendCommandError::Disconnected)
        );
    }

    #[test]
    fn owner_drop_joins_and_tears_down_even_while_clone_survives() {
        let probe = WorkerTestProbe::default();
        let worker = ComparisonWorker::spawn_inner(
            FakeAudioDriver::default(),
            1,
            None,
            Some(probe.clone()),
            Some(vec![0.6; MIN_COMMIT_FRAMES].into_boxed_slice()),
        );
        let clone = worker.command_sender();
        drop(worker);
        assert!(probe.exited.load(Ordering::SeqCst));
        assert!(probe.deactivated.load(Ordering::SeqCst));
        assert!(probe.scrubbed.load(Ordering::SeqCst));
        assert_eq!(
            clone.try_send(Command {
                sequence: 2,
                kind: CommandKind::ProbeAvailability,
            }),
            Err(SendCommandError::Disconnected)
        );
    }
}
