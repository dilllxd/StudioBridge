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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandKind {
    ProbeAvailability,
    ToggleRecord,
    /// Production use is forbidden until this generation is checked against a
    /// complete read-only destination snapshot. The simulated worker accepts
    /// only `None`; no production worker constructor exists yet.
    TogglePlay {
        destination_ack_generation: Option<u64>,
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
    PlaybackPolicyNotBound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalEvent {
    Snapshot(Snapshot),
    Diagnostic { revision: u64, kind: DiagnosticKind },
    ShutdownComplete { revision: u64 },
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
            CommandKind::ToggleRecord => self.engine.handle(command.sequence, Action::ToggleRecord),
            CommandKind::TogglePlay {
                destination_ack_generation,
            } => {
                if destination_ack_generation.is_some() {
                    self.emit_diagnostic(DiagnosticKind::PlaybackPolicyNotBound);
                    return false;
                }
                self.engine.handle(command.sequence, Action::TogglePlay)
            }
            CommandKind::Cancel => self.engine.handle(command.sequence, Action::Cancel),
            CommandKind::WorkspaceExited => self
                .engine
                .handle(command.sequence, Action::WorkspaceExited),
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
        let reason = match &state {
            PublicState::Unavailable { reason } => Some(reason.clone()),
            _ => None,
        };
        Snapshot {
            revision: self.engine.revision(),
            timer_tenths: timer_tenths(&state),
            controls: Controls::from_state(&state),
            state,
            reason,
            mode: ComparisonMode::Simulated,
            availability_label: "Simulated comparison",
        }
    }

    fn publish_semantic_snapshot(&self) {
        let _ = self.terminal.send(TerminalEvent::Snapshot(self.snapshot()));
    }

    fn semantic_signature(&self) -> (u64, std::mem::Discriminant<PublicState>, Controls) {
        let state = self.engine.state();
        (
            self.engine.revision(),
            std::mem::discriminant(&state),
            Controls::from_state(&state),
        )
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
                    destination_ack_generation: None,
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
    fn destination_generation_cannot_be_claimed_before_policy_is_bound() {
        let (mut core, latest, terminal) = core();
        let initial = next_semantic(&terminal);
        core.process(Command {
            sequence: 1,
            kind: CommandKind::TogglePlay {
                destination_ack_generation: Some(4),
            },
        });
        assert!(latest.take().is_none());
        assert_eq!(core.engine.revision(), initial.revision);
        assert!(matches!(
            terminal.try_recv().unwrap(),
            TerminalEvent::Diagnostic {
                kind: DiagnosticKind::PlaybackPolicyNotBound,
                ..
            }
        ));
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
        core.process(Command {
            sequence: 1,
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
                    destination_ack_generation: None,
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
        core.process(Command {
            sequence: 1,
            kind: CommandKind::ToggleRecord,
        });
        next_semantic(&terminal);
        core.accept_capture(&[0.2; MIN_COMMIT_FRAMES]);
        next_semantic(&terminal);
        core.engine.driver.fail_next(crate::DriverCall::StopStream);
        core.process(Command {
            sequence: 2,
            kind: CommandKind::TogglePlay {
                destination_ack_generation: None,
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
