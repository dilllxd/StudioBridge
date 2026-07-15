//! Cross-platform policy for microphone comparison.
//!
//! This crate deliberately contains no audio, device, control, routing, or
//! filesystem implementation. Platform code may implement [`AudioDriver`],
//! while tests and Windows development use [`FakeAudioDriver`].

use std::fmt;
use zeroize::Zeroize;

mod worker;

pub use worker::{
    COMMAND_CAPACITY, Command, CommandKind, CommandSender, ComparisonMode, ComparisonWorker,
    Controls, DiagnosticKind, SendCommandError, Snapshot, TerminalEvent,
};

pub const SAMPLE_RATE: usize = 48_000;
pub const MAX_FRAMES: usize = 480_000;
pub const MIN_COMMIT_FRAMES: usize = 960;
pub const SLOT_BYTES: usize = MAX_FRAMES * size_of::<f32>();
pub const TOTAL_SAMPLE_BYTES: usize = SLOT_BYTES * 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamToken {
    id: u64,
    callback_epoch: u64,
}

impl StreamToken {
    pub fn callback_epoch(self) -> u64 {
        self.callback_epoch
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioError(pub String);

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AudioError {}

/// Audio lifecycle seam. Implementations must not put policy in the driver.
///
/// Production playback remains forbidden until the future worker layer binds
/// a read-only destination snapshot and its generation to each Play command.
/// This crate intentionally cannot claim that confirmation has happened.
/// `begin_capture` and `begin_playback_loop` must leave no stream active when
/// returning `Err`; any later lifecycle error is handled by `deactivate_all`.
pub trait AudioDriver: Send + 'static {
    fn begin_capture(&mut self, callback_epoch: u64) -> Result<StreamToken, AudioError>;
    fn pause_capture(&mut self, token: StreamToken) -> Result<(), AudioError>;
    fn resume_capture(&mut self, token: StreamToken) -> Result<(), AudioError>;
    fn begin_playback_loop(&mut self, callback_epoch: u64) -> Result<StreamToken, AudioError>;
    fn stop_stream(&mut self, token: StreamToken) -> Result<(), AudioError>;
    /// Infallibly tear down every stream after lifecycle state becomes ambiguous.
    fn deactivate_all(&mut self);
    fn shutdown(&mut self);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DriverCall {
    BeginCapture,
    PauseCapture,
    ResumeCapture,
    BeginPlayback,
    StopStream,
    DeactivateAll,
    Shutdown,
}

/// A deterministic lifecycle-only driver. It cannot open a device or move audio.
#[derive(Debug, Default)]
pub struct FakeAudioDriver {
    next_token: u64,
    active: Option<StreamToken>,
    paused: bool,
    fail_next: Option<DriverCall>,
    calls: Vec<DriverCall>,
}

impl FakeAudioDriver {
    pub fn fail_next(&mut self, call: DriverCall) {
        self.fail_next = Some(call);
    }

    pub fn calls(&self) -> &[DriverCall] {
        &self.calls
    }

    pub fn active_stream_count(&self) -> usize {
        usize::from(self.active.is_some())
    }

    fn call(&mut self, call: DriverCall) -> Result<(), AudioError> {
        self.calls.push(call);
        if self.fail_next == Some(call) {
            self.fail_next = None;
            return Err(AudioError(format!("simulated {call:?} failure")));
        }
        Ok(())
    }

    fn start(&mut self, call: DriverCall, callback_epoch: u64) -> Result<StreamToken, AudioError> {
        self.call(call)?;
        if self.active.is_some() {
            return Err(AudioError("fake driver refuses overlapping streams".into()));
        }
        self.next_token += 1;
        let token = StreamToken {
            id: self.next_token,
            callback_epoch,
        };
        self.active = Some(token);
        self.paused = false;
        Ok(token)
    }
}

impl AudioDriver for FakeAudioDriver {
    fn begin_capture(&mut self, callback_epoch: u64) -> Result<StreamToken, AudioError> {
        self.start(DriverCall::BeginCapture, callback_epoch)
    }

    fn pause_capture(&mut self, token: StreamToken) -> Result<(), AudioError> {
        self.call(DriverCall::PauseCapture)?;
        if self.active != Some(token) || self.paused {
            return Err(AudioError("capture stream is not active".into()));
        }
        self.paused = true;
        Ok(())
    }

    fn resume_capture(&mut self, token: StreamToken) -> Result<(), AudioError> {
        self.call(DriverCall::ResumeCapture)?;
        if self.active != Some(token) || !self.paused {
            return Err(AudioError("capture stream is not paused".into()));
        }
        self.paused = false;
        Ok(())
    }

    fn begin_playback_loop(&mut self, callback_epoch: u64) -> Result<StreamToken, AudioError> {
        self.start(DriverCall::BeginPlayback, callback_epoch)
    }

    fn stop_stream(&mut self, token: StreamToken) -> Result<(), AudioError> {
        self.call(DriverCall::StopStream)?;
        if self.active != Some(token) {
            return Err(AudioError("stream token is stale".into()));
        }
        self.active = None;
        self.paused = false;
        Ok(())
    }

    fn deactivate_all(&mut self) {
        self.calls.push(DriverCall::DeactivateAll);
        self.active = None;
        self.paused = false;
    }

    fn shutdown(&mut self) {
        self.calls.push(DriverCall::Shutdown);
        self.active = None;
        self.paused = false;
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicState {
    Unavailable {
        reason: String,
    },
    Empty,
    Ready {
        frames: usize,
    },
    Recording {
        draft_frames: usize,
    },
    RecordingPaused {
        draft_frames: usize,
    },
    Playing {
        frames: usize,
        elapsed_frames: usize,
    },
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    ToggleRecord,
    TogglePlay,
    Cancel,
    WorkspaceExited,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActionResult {
    Transitioned,
    IgnoredDuplicate,
    Rejected(String),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Diagnostics {
    pub non_finite_samples: u64,
    pub stale_callbacks: u64,
}

#[derive(Debug)]
struct Slot {
    samples: Box<[f32]>,
    frames: usize,
}

impl Slot {
    fn new() -> Self {
        Self {
            samples: vec![0.0; MAX_FRAMES].into_boxed_slice(),
            frames: 0,
        }
    }

    fn scrub(&mut self) {
        self.samples.zeroize();
        self.frames = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Unavailable,
    Empty,
    Ready,
    Recording,
    RecordingPaused,
    Playing,
    Shutdown,
}

/// Single-owner policy engine. Callers must serialize access to this type.
pub struct ComparisonEngine<D: AudioDriver> {
    driver: D,
    slots: [Slot; 2],
    committed: Option<usize>,
    draft: usize,
    state: State,
    unavailable_reason: String,
    observed_generation: u64,
    callback_epoch: u64,
    active_token: Option<StreamToken>,
    playback_cursor: usize,
    last_sequence: Option<u64>,
    revision: u64,
    diagnostics: Diagnostics,
}

impl<D: AudioDriver> ComparisonEngine<D> {
    pub fn new_unavailable(driver: D, reason: impl Into<String>, generation: u64) -> Self {
        Self {
            driver,
            slots: [Slot::new(), Slot::new()],
            committed: None,
            draft: 0,
            state: State::Unavailable,
            unavailable_reason: reason.into(),
            observed_generation: generation,
            callback_epoch: 0,
            active_token: None,
            playback_cursor: 0,
            last_sequence: None,
            revision: 0,
            diagnostics: Diagnostics::default(),
        }
    }

    pub fn new_simulated(driver: D, generation: u64) -> Self {
        let mut engine = Self::new_unavailable(driver, "Simulated comparison", generation);
        engine.state = State::Empty;
        engine
    }

    pub fn state(&self) -> PublicState {
        let frames = self.committed.map_or(0, |slot| self.slots[slot].frames);
        match self.state {
            State::Unavailable => PublicState::Unavailable {
                reason: self.unavailable_reason.clone(),
            },
            State::Empty => PublicState::Empty,
            State::Ready => PublicState::Ready { frames },
            State::Recording => PublicState::Recording {
                draft_frames: self.slots[self.draft].frames,
            },
            State::RecordingPaused => PublicState::RecordingPaused {
                draft_frames: self.slots[self.draft].frames,
            },
            State::Playing => PublicState::Playing {
                frames,
                elapsed_frames: self.playback_cursor,
            },
            State::Shutdown => PublicState::Shutdown,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn diagnostics(&self) -> Diagnostics {
        self.diagnostics
    }

    pub fn driver(&self) -> &D {
        &self.driver
    }

    pub fn storage_bytes(&self) -> usize {
        self.slots
            .iter()
            .map(|slot| slot.samples.len() * size_of::<f32>())
            .sum()
    }

    pub(crate) fn active_token(&self) -> Option<StreamToken> {
        self.active_token
    }

    pub(crate) fn buffers_are_scrubbed(&self) -> bool {
        self.slots
            .iter()
            .all(|slot| slot.frames == 0 && slot.samples.iter().all(|sample| *sample == 0.0))
    }

    pub fn handle(&mut self, sequence: u64, action: Action) -> ActionResult {
        if self.last_sequence.is_some_and(|last| sequence <= last) {
            return ActionResult::IgnoredDuplicate;
        }
        self.last_sequence = Some(sequence);
        if self.state == State::Shutdown {
            return ActionResult::Rejected("comparison worker has shut down".into());
        }
        if self.state == State::Unavailable {
            return ActionResult::Rejected(self.unavailable_reason.clone());
        }

        let result = match action {
            Action::ToggleRecord => self.toggle_record(),
            Action::TogglePlay => self.toggle_play(),
            Action::Cancel => self.cancel(),
            Action::WorkspaceExited => self.workspace_exited(),
        };
        match result {
            Ok(()) => {
                self.revision += 1;
                ActionResult::Transitioned
            }
            Err(error) => ActionResult::Rejected(error.0),
        }
    }

    fn toggle_record(&mut self) -> Result<(), AudioError> {
        match self.state {
            State::Empty | State::Ready => self.start_recording(),
            State::Recording => {
                if let Err(error) = self
                    .driver
                    .pause_capture(self.active_token.expect("active capture"))
                {
                    let reason = format!("capture pause failed: {error}");
                    self.fail_closed(reason.clone());
                    return Err(AudioError(reason));
                }
                self.state = State::RecordingPaused;
                Ok(())
            }
            State::RecordingPaused => {
                if let Err(error) = self
                    .driver
                    .resume_capture(self.active_token.expect("active capture"))
                {
                    let reason = format!("capture resume failed: {error}");
                    self.fail_closed(reason.clone());
                    return Err(AudioError(reason));
                }
                self.state = State::Recording;
                Ok(())
            }
            State::Playing => {
                self.stop_active()?;
                self.state = State::Ready;
                self.start_recording()
            }
            _ => Err(AudioError(
                "record is unavailable in the current state".into(),
            )),
        }
    }

    fn start_recording(&mut self) -> Result<(), AudioError> {
        let draft = self.committed.map_or(0, |slot| 1 - slot);
        self.slots[draft].scrub();
        let epoch = self.next_callback_epoch();
        let token = self.driver.begin_capture(epoch)?;
        self.draft = draft;
        self.active_token = Some(token);
        self.state = State::Recording;
        Ok(())
    }

    fn toggle_play(&mut self) -> Result<(), AudioError> {
        match self.state {
            State::Empty => Err(AudioError("record a comparison before playing".into())),
            State::Ready => self.start_playback(),
            State::Recording | State::RecordingPaused => {
                if self.slots[self.draft].frames < MIN_COMMIT_FRAMES {
                    return Err(AudioError(format!(
                        "comparison requires at least {MIN_COMMIT_FRAMES} frames"
                    )));
                }
                self.stop_active()?;
                self.commit_draft();
                self.state = State::Ready;
                self.start_playback()
            }
            State::Playing => {
                self.stop_active()?;
                self.playback_cursor = 0;
                self.state = State::Ready;
                Ok(())
            }
            _ => Err(AudioError(
                "play is unavailable in the current state".into(),
            )),
        }
    }

    fn start_playback(&mut self) -> Result<(), AudioError> {
        let epoch = self.next_callback_epoch();
        let token = self.driver.begin_playback_loop(epoch)?;
        self.active_token = Some(token);
        self.playback_cursor = 0;
        self.state = State::Playing;
        Ok(())
    }

    fn cancel(&mut self) -> Result<(), AudioError> {
        match self.state {
            State::Recording | State::RecordingPaused => {
                self.stop_active()?;
                self.slots[self.draft].scrub();
                self.state = if self.committed.is_some() {
                    State::Ready
                } else {
                    State::Empty
                };
                Ok(())
            }
            State::Playing => {
                self.stop_active()?;
                self.playback_cursor = 0;
                self.state = State::Ready;
                Ok(())
            }
            State::Empty | State::Ready => Ok(()),
            _ => Err(AudioError(
                "cancel is unavailable in the current state".into(),
            )),
        }
    }

    fn workspace_exited(&mut self) -> Result<(), AudioError> {
        self.cancel()
    }

    fn stop_active(&mut self) -> Result<(), AudioError> {
        if let Some(token) = self.active_token {
            if let Err(error) = self.driver.stop_stream(token) {
                let reason = format!("stream stop failed: {error}");
                self.fail_closed(reason.clone());
                return Err(AudioError(reason));
            }
            self.active_token = None;
            self.next_callback_epoch();
        }
        Ok(())
    }

    fn commit_draft(&mut self) {
        if let Some(old) = self.committed.replace(self.draft) {
            self.slots[old].scrub();
            self.draft = old;
        } else {
            self.draft = 1 - self.draft;
            self.slots[self.draft].scrub();
        }
    }

    /// Accept canonical mono 48 kHz samples from the current capture callback.
    pub fn accept_capture(&mut self, token: StreamToken, input: &[f32]) {
        if self.state != State::Recording
            || Some(token) != self.active_token
            || token.callback_epoch != self.callback_epoch
        {
            self.diagnostics.stale_callbacks += 1;
            return;
        }
        let slot = &mut self.slots[self.draft];
        let accepted = input.len().min(MAX_FRAMES - slot.frames);
        for &sample in &input[..accepted] {
            let value = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                self.diagnostics.non_finite_samples += 1;
                0.0
            };
            slot.samples[slot.frames] = value;
            slot.frames += 1;
        }
        if slot.frames == MAX_FRAMES {
            if self.stop_active().is_err() {
                return;
            }
            self.commit_draft();
            self.state = State::Ready;
            self.revision += 1;
        }
    }

    /// Render from the committed clip with exact loop wrap.
    pub fn render_playback(
        &mut self,
        token: StreamToken,
        output: &mut [f32],
    ) -> Result<(), AudioError> {
        if self.state != State::Playing
            || Some(token) != self.active_token
            || token.callback_epoch != self.callback_epoch
        {
            self.diagnostics.stale_callbacks += 1;
            output.fill(0.0);
            return Err(AudioError("stale playback callback".into()));
        }
        let slot = self.committed.expect("playing requires committed clip");
        let frames = self.slots[slot].frames;
        if frames == 0 {
            output.fill(0.0);
            self.stream_error(token, "playback underrun");
            return Err(AudioError("playback underrun".into()));
        }
        for sample in output {
            *sample = self.slots[slot].samples[self.playback_cursor];
            self.playback_cursor = (self.playback_cursor + 1) % frames;
        }
        Ok(())
    }

    pub fn set_unavailable(&mut self, generation: u64, reason: impl Into<String>) {
        if self.state == State::Shutdown || generation <= self.observed_generation {
            return;
        }
        self.observed_generation = generation;
        self.fail_closed(reason.into());
    }

    pub fn set_available(&mut self, generation: u64) {
        if self.state == State::Shutdown || generation <= self.observed_generation {
            return;
        }
        self.observed_generation = generation;
        if self.active_token.is_some()
            || matches!(
                self.state,
                State::Recording | State::RecordingPaused | State::Playing
            )
        {
            self.emergency_teardown();
        }
        self.state = if self.committed.is_some() {
            State::Ready
        } else {
            State::Empty
        };
        self.revision += 1;
    }

    pub fn stream_error(&mut self, token: StreamToken, reason: impl Into<String>) {
        if Some(token) != self.active_token || token.callback_epoch != self.callback_epoch {
            self.diagnostics.stale_callbacks += 1;
            return;
        }
        self.fail_closed(reason.into());
    }

    fn next_callback_epoch(&mut self) -> u64 {
        self.callback_epoch = self.callback_epoch.saturating_add(1);
        self.callback_epoch
    }

    fn emergency_teardown(&mut self) {
        self.driver.deactivate_all();
        self.active_token = None;
        self.next_callback_epoch();
        if matches!(self.state, State::Recording | State::RecordingPaused) {
            self.slots[self.draft].scrub();
        }
        self.playback_cursor = 0;
    }

    fn fail_closed(&mut self, reason: String) {
        self.emergency_teardown();
        self.unavailable_reason = reason;
        self.state = State::Unavailable;
        self.revision += 1;
    }

    pub fn shutdown(&mut self) {
        if self.state == State::Shutdown {
            return;
        }
        self.emergency_teardown();
        self.driver.shutdown();
        self.slots[0].scrub();
        self.slots[1].scrub();
        self.committed = None;
        self.state = State::Shutdown;
        self.revision += 1;
    }
}

impl<D: AudioDriver> Drop for ComparisonEngine<D> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording() -> (ComparisonEngine<FakeAudioDriver>, StreamToken) {
        let mut engine = ComparisonEngine::new_simulated(FakeAudioDriver::default(), 7);
        assert_eq!(
            engine.handle(1, Action::ToggleRecord),
            ActionResult::Transitioned
        );
        let token = engine.active_token.unwrap();
        (engine, token)
    }

    fn ready(frames: usize) -> ComparisonEngine<FakeAudioDriver> {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &vec![0.25; frames]);
        assert_eq!(
            engine.handle(2, Action::TogglePlay),
            ActionResult::Transitioned
        );
        assert_eq!(
            engine.handle(3, Action::TogglePlay),
            ActionResult::Transitioned
        );
        engine
    }

    #[test]
    fn fixed_storage_is_exactly_two_slots() {
        let engine = ComparisonEngine::new_simulated(FakeAudioDriver::default(), 1);
        assert_eq!(engine.storage_bytes(), TOTAL_SAMPLE_BYTES);
        assert_eq!(TOTAL_SAMPLE_BYTES, 3_840_000);
    }

    #[test]
    fn record_pause_resume_preserves_cursor() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[0.1; 400]);
        assert_eq!(
            engine.handle(2, Action::ToggleRecord),
            ActionResult::Transitioned
        );
        assert_eq!(
            engine.state(),
            PublicState::RecordingPaused { draft_frames: 400 }
        );
        assert_eq!(
            engine.handle(3, Action::ToggleRecord),
            ActionResult::Transitioned
        );
        assert_eq!(engine.state(), PublicState::Recording { draft_frames: 400 });
    }

    #[test]
    fn paused_capture_can_commit_directly_to_playback() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[0.1; MIN_COMMIT_FRAMES]);
        engine.handle(2, Action::ToggleRecord);
        assert_eq!(
            engine.handle(3, Action::TogglePlay),
            ActionResult::Transitioned
        );
        assert!(matches!(engine.state(), PublicState::Playing { .. }));
        assert_eq!(engine.driver().active_stream_count(), 1);
    }

    #[test]
    fn too_short_draft_does_not_replace_prior_clip() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        assert_eq!(
            engine.handle(4, Action::ToggleRecord),
            ActionResult::Transitioned
        );
        let token = engine.active_token.unwrap();
        engine.accept_capture(token, &[0.9; MIN_COMMIT_FRAMES - 1]);
        assert!(matches!(
            engine.handle(5, Action::TogglePlay),
            ActionResult::Rejected(_)
        ));
        assert_eq!(engine.handle(6, Action::Cancel), ActionResult::Transitioned);
        assert_eq!(
            engine.state(),
            PublicState::Ready {
                frames: MIN_COMMIT_FRAMES
            }
        );
    }

    #[test]
    fn commit_swaps_slots_and_scrubs_displaced_clip() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        let old = engine.committed.unwrap();
        engine.handle(4, Action::ToggleRecord);
        let token = engine.active_token.unwrap();
        engine.accept_capture(token, &[0.8; MIN_COMMIT_FRAMES + 5]);
        engine.handle(5, Action::TogglePlay);
        assert_ne!(engine.committed, Some(old));
        assert_eq!(engine.slots[old].frames, 0);
        assert!(
            engine.slots[old]
                .samples
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn cap_auto_commits_exactly_480000_frames() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &vec![0.2; MAX_FRAMES + 100]);
        assert_eq!(engine.state(), PublicState::Ready { frames: MAX_FRAMES });
        assert_eq!(engine.driver().active_stream_count(), 0);
    }

    #[test]
    fn samples_are_sanitized_without_exposing_values_in_diagnostics() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[f32::NAN, f32::INFINITY, -2.0, 2.0]);
        assert_eq!(engine.diagnostics().non_finite_samples, 2);
        assert_eq!(
            &engine.slots[engine.draft].samples[..4],
            &[0.0, 0.0, -1.0, 1.0]
        );
    }

    #[test]
    fn playback_wraps_at_committed_length() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        engine.handle(4, Action::TogglePlay);
        let token = engine.active_token.unwrap();
        let mut output = vec![0.0; MIN_COMMIT_FRAMES + 3];
        engine.render_playback(token, &mut output).unwrap();
        assert_eq!(engine.playback_cursor, 3);
        assert!(output.iter().all(|sample| *sample == 0.25));
    }

    #[test]
    fn endpoint_loss_scrubs_draft_retains_committed_and_invalidates_callback() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        engine.handle(4, Action::ToggleRecord);
        let stale = engine.active_token.unwrap();
        engine.accept_capture(stale, &[0.7; MIN_COMMIT_FRAMES]);
        engine.set_unavailable(8, "endpoint disappeared");
        assert_eq!(
            engine.state(),
            PublicState::Unavailable {
                reason: "endpoint disappeared".into()
            }
        );
        assert_eq!(engine.slots[engine.draft].frames, 0);
        engine.accept_capture(stale, &[1.0]);
        assert_eq!(engine.diagnostics().stale_callbacks, 1);
        engine.set_available(9);
        assert_eq!(
            engine.state(),
            PublicState::Ready {
                frames: MIN_COMMIT_FRAMES
            }
        );
    }

    #[test]
    fn stale_endpoint_events_cannot_mutate_state_revision_or_stream() {
        let (mut engine, token) = recording();
        let revision = engine.revision();
        engine.set_unavailable(6, "stale loss");
        engine.set_available(7);
        assert!(matches!(engine.state(), PublicState::Recording { .. }));
        assert_eq!(engine.revision(), revision);
        assert_eq!(engine.driver().active_stream_count(), 1);
        engine.accept_capture(token, &[0.5]);
        assert_eq!(engine.diagnostics().stale_callbacks, 0);

        engine.set_unavailable(8, "current loss");
        let unavailable_revision = engine.revision();
        engine.set_available(8);
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.revision(), unavailable_revision);
        assert_eq!(engine.driver().active_stream_count(), 0);
        engine.set_available(9);
        assert_eq!(engine.state(), PublicState::Empty);
    }

    #[test]
    fn duplicate_and_stale_sequences_are_ignored() {
        let mut engine = ComparisonEngine::new_simulated(FakeAudioDriver::default(), 1);
        assert_eq!(
            engine.handle(10, Action::ToggleRecord),
            ActionResult::Transitioned
        );
        assert_eq!(
            engine.handle(10, Action::ToggleRecord),
            ActionResult::IgnoredDuplicate
        );
        assert_eq!(
            engine.handle(9, Action::TogglePlay),
            ActionResult::IgnoredDuplicate
        );
        assert!(matches!(engine.state(), PublicState::Recording { .. }));
    }

    #[test]
    fn start_failure_leaves_prior_state_and_no_active_stream() {
        let mut driver = FakeAudioDriver::default();
        driver.fail_next(DriverCall::BeginCapture);
        let mut engine = ComparisonEngine::new_simulated(driver, 1);
        assert!(matches!(
            engine.handle(1, Action::ToggleRecord),
            ActionResult::Rejected(_)
        ));
        assert_eq!(engine.state(), PublicState::Empty);
        assert_eq!(engine.driver().active_stream_count(), 0);
    }

    #[test]
    fn playback_start_failure_keeps_new_commit_ready() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[0.3; MIN_COMMIT_FRAMES]);
        engine.driver.fail_next(DriverCall::BeginPlayback);
        assert!(matches!(
            engine.handle(2, Action::TogglePlay),
            ActionResult::Rejected(_)
        ));
        assert_eq!(
            engine.state(),
            PublicState::Ready {
                frames: MIN_COMMIT_FRAMES
            }
        );
    }

    #[test]
    fn unavailable_rejects_every_ui_action_without_driver_calls() {
        let mut engine =
            ComparisonEngine::new_unavailable(FakeAudioDriver::default(), "not validated", 1);
        for (sequence, action) in [
            Action::ToggleRecord,
            Action::TogglePlay,
            Action::Cancel,
            Action::WorkspaceExited,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                engine.handle(sequence as u64 + 1, action),
                ActionResult::Rejected("not validated".into())
            );
        }
        assert!(engine.driver().calls().is_empty());
    }

    #[test]
    fn stream_error_fails_closed_and_rejects_stale_error() {
        let (mut engine, stale) = recording();
        engine.stream_error(stale, "capture failed");
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        let revision = engine.revision();
        engine.stream_error(stale, "late error");
        assert_eq!(engine.revision(), revision);
        assert_eq!(engine.diagnostics().stale_callbacks, 1);
    }

    #[test]
    fn workspace_exit_cancels_capture_and_stops_playback_but_retains_clip() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        engine.handle(4, Action::TogglePlay);
        assert_eq!(
            engine.handle(5, Action::WorkspaceExited),
            ActionResult::Transitioned
        );
        assert_eq!(
            engine.state(),
            PublicState::Ready {
                frames: MIN_COMMIT_FRAMES
            }
        );
        engine.handle(6, Action::ToggleRecord);
        assert_eq!(
            engine.handle(7, Action::WorkspaceExited),
            ActionResult::Transitioned
        );
        assert_eq!(
            engine.state(),
            PublicState::Ready {
                frames: MIN_COMMIT_FRAMES
            }
        );
    }

    #[test]
    fn playing_to_recording_stops_playback_before_capture() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        engine.handle(4, Action::TogglePlay);
        assert_eq!(
            engine.handle(5, Action::ToggleRecord),
            ActionResult::Transitioned
        );
        assert!(matches!(
            engine.state(),
            PublicState::Recording { draft_frames: 0 }
        ));
        assert_eq!(engine.driver().active_stream_count(), 1);
        let calls = engine.driver().calls();
        assert_eq!(
            &calls[calls.len() - 2..],
            &[DriverCall::StopStream, DriverCall::BeginCapture]
        );
    }

    #[test]
    fn cancel_without_prior_clip_scrubs_draft_and_returns_empty() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[0.5; MIN_COMMIT_FRAMES]);
        assert_eq!(engine.handle(2, Action::Cancel), ActionResult::Transitioned);
        assert_eq!(engine.state(), PublicState::Empty);
        assert!(
            engine.slots[engine.draft]
                .samples
                .iter()
                .all(|sample| *sample == 0.0)
        );
        engine.accept_capture(token, &[1.0]);
        assert_eq!(engine.diagnostics().stale_callbacks, 1);
    }

    #[test]
    fn shutdown_is_idempotent_stops_stream_and_scrubs_both_slots() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        engine.handle(4, Action::TogglePlay);
        engine.shutdown();
        engine.shutdown();
        assert_eq!(engine.state(), PublicState::Shutdown);
        assert_eq!(engine.driver().active_stream_count(), 0);
        assert!(engine.slots.iter().all(|slot| slot.frames == 0));
        assert!(
            engine
                .slots
                .iter()
                .flat_map(|slot| slot.samples.iter())
                .all(|sample| *sample == 0.0)
        );
        assert_eq!(
            engine
                .driver()
                .calls()
                .iter()
                .filter(|call| **call == DriverCall::Shutdown)
                .count(),
            1
        );
    }

    #[test]
    fn fake_stop_failure_is_sticky_until_emergency_deactivation() {
        let mut driver = FakeAudioDriver::default();
        let token = driver.begin_capture(1).unwrap();
        driver.fail_next(DriverCall::StopStream);
        assert!(driver.stop_stream(token).is_err());
        assert_eq!(driver.active_stream_count(), 1);
        driver.deactivate_all();
        assert_eq!(driver.active_stream_count(), 0);
    }

    #[test]
    fn stop_failure_during_cancel_fails_closed_and_scrubs_draft() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[0.5; MIN_COMMIT_FRAMES]);
        engine.driver.fail_next(DriverCall::StopStream);
        assert!(matches!(
            engine.handle(2, Action::Cancel),
            ActionResult::Rejected(_)
        ));
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.driver().active_stream_count(), 0);
        assert_eq!(engine.slots[engine.draft].frames, 0);
        assert!(
            engine.slots[engine.draft]
                .samples
                .iter()
                .all(|sample| *sample == 0.0)
        );
    }

    #[test]
    fn stop_failure_during_record_to_play_does_not_commit_or_overlap() {
        let (mut engine, token) = recording();
        engine.accept_capture(token, &[0.5; MIN_COMMIT_FRAMES]);
        engine.driver.fail_next(DriverCall::StopStream);
        assert!(matches!(
            engine.handle(2, Action::TogglePlay),
            ActionResult::Rejected(_)
        ));
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.committed, None);
        assert_eq!(engine.driver().active_stream_count(), 0);
    }

    #[test]
    fn stop_failure_during_play_to_record_does_not_overlap() {
        let mut engine = ready(MIN_COMMIT_FRAMES);
        engine.handle(4, Action::TogglePlay);
        engine.driver.fail_next(DriverCall::StopStream);
        assert!(matches!(
            engine.handle(5, Action::ToggleRecord),
            ActionResult::Rejected(_)
        ));
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.driver().active_stream_count(), 0);
        assert_eq!(
            engine.committed.map(|slot| engine.slots[slot].frames),
            Some(MIN_COMMIT_FRAMES)
        );
    }

    #[test]
    fn stop_failure_at_capture_cap_never_auto_commits() {
        let (mut engine, token) = recording();
        engine.driver.fail_next(DriverCall::StopStream);
        engine.accept_capture(token, &vec![0.5; MAX_FRAMES]);
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.committed, None);
        assert_eq!(engine.driver().active_stream_count(), 0);
        assert_eq!(engine.slots[engine.draft].frames, 0);
    }

    #[test]
    fn endpoint_loss_uses_infallible_teardown_even_when_stop_would_fail() {
        let (mut engine, _) = recording();
        engine.driver.fail_next(DriverCall::StopStream);
        engine.set_unavailable(8, "endpoint lost");
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.driver().active_stream_count(), 0);
        assert!(engine.driver().calls().contains(&DriverCall::DeactivateAll));
    }

    #[test]
    fn stop_failure_requires_a_fresh_endpoint_generation_to_recover() {
        let (mut engine, _) = recording();
        engine.driver.fail_next(DriverCall::StopStream);
        let _ = engine.handle(2, Action::Cancel);
        let revision = engine.revision();
        engine.set_available(7);
        assert!(matches!(engine.state(), PublicState::Unavailable { .. }));
        assert_eq!(engine.revision(), revision);
        engine.set_available(8);
        assert_eq!(engine.state(), PublicState::Empty);
    }

    #[test]
    fn deterministic_command_sequences_never_overlap_streams_or_grow_storage() {
        for mask in 0_u16..256 {
            let mut engine = ComparisonEngine::new_simulated(FakeAudioDriver::default(), 1);
            for step in 0..8 {
                let action = if mask & (1 << step) == 0 {
                    Action::ToggleRecord
                } else {
                    Action::TogglePlay
                };
                let _ = engine.handle(step + 1, action);
                if matches!(engine.state(), PublicState::Recording { .. }) {
                    let token = engine.active_token.unwrap();
                    engine.accept_capture(token, &[0.1; MIN_COMMIT_FRAMES]);
                }
                assert!(engine.driver().active_stream_count() <= 1);
                assert_eq!(engine.storage_bytes(), TOTAL_SAMPLE_BYTES);
            }
            engine.shutdown();
            assert_eq!(engine.driver().active_stream_count(), 0);
        }
    }
}
