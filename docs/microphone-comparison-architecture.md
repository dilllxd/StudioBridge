# Microphone comparison architecture

## Decision and safety boundary

StudioBridge can match BEACN's microphone comparison only if the BEACN Studio
exposes both of these real audio paths on USB1:

1. a **pre-processing microphone capture tap**; and
2. a **microphone-chain playback ingress** whose samples pass through the
   device's currently active microphone DSP before reaching already configured
   outputs.

Neither path is proven yet. Until an attended Linux hardware test proves both,
the native UI must report `Comparison unavailable: live-chain audio path has
not been validated` and keep Record and Play disabled. A recording made from
the normal `Mic` PipeWire source is post-processing. Playing that recording to
Headphones, a generic PipeWire sink, or a PipeWeaver source would replay baked
audio and would not reflect subsequent DSP edits. StudioBridge must never
present any of those substitutes as live-chain comparison.

The recorder is session-local audio transport. It must never:

- acquire or extend a DSP lease;
- send a BEACN USB control or storage command;
- enable phantom power, gain, lighting, limiter, firmware, or reset paths;
- create, enable, remove, or rewrite a PipeWeaver route;
- change a PipeWire default device;
- silently provision a monitor path; or
- save, export, upload, or log microphone samples.

Audio playback into a validated chain ingress is itself an audible action, so
it begins only after the user presses Play. Endpoint probing must be metadata
only. It must not send a tone, captured speech, or silence as a probe.

## Evidence and present blocker

The current StudioBridge PipeWeaver adapter uses PipeWeaver's public command
and status API. That API provides graph state, mutations, and meters, but not
audio samples or an insertion point inside the BEACN microphone DSP chain.
PipeWeaver's physical microphone source begins at a pass-through filter after
the attached PipeWire source, so attaching a recorder there can only capture
what the BEACN USB capture endpoint already emits.

The upstream ALSA UCM configuration provides a promising, explicitly
unverified clue:

| USB1 channel | Upstream UCM description | Required interpretation |
| --- | --- | --- |
| Capture 0/1 | Microphone left/right | Normal processed Mic output; forbidden as the comparison capture source. |
| Capture 2 | `UNKNOWN (Possible Dry Mic)` | Candidate pre-DSP mono capture tap; must be characterized. |
| Capture 3 | `UNKNOWN (Possible Dry + Expander)` | Candidate intermediate tap; must be characterized and must not be accepted as pre-DSP merely because it differs from Mic. |
| Playback 2 | `Unused Channel` | Candidate chain ingress; must be characterized. |
| Playback 0/1 | Headphones left/right | Normal output; forbidden as the comparison chain ingress. |

Sources:

- [upstream BEACN Studio USB1 channel map](https://github.com/alsa-project/alsa-ucm-conf/blob/master/ucm2/USB-Audio/Beacn/Beacn-Studio-USB1-Channels.conf)
- [PipeWire stream and explicit-target semantics](https://docs.pipewire.org/devel/page_streams.html)
- [PipeWire target properties](https://docs.pipewire.org/page_man_pipewire-props_7.html)

The channel comments are not a protocol guarantee. No production code may
select channels 2 or 3 by numeric position, create guessed UCM nodes, or play
audio into playback channel 2 until the physical validation gate below passes.

## Required production topology

After physical validation, add narrowly named split PCM devices to the BEACN
UCM profile (preferably upstream) for the one proven dry capture channel and
the one proven test-playback channel. WirePlumber may then expose two dedicated
nodes, for example `BEACN Studio Comparison Capture` and `BEACN Studio
Comparison Playback`. Names here are illustrative; production discovery must
also require the BEACN USB1 ALSA card identity, direction, channel count, and a
versioned StudioBridge capability marker. A matching name alone is not enough.

```text
XLR microphone
      |
      +-- proven pre-DSP USB1 capture tap
      |          |
      |          v
      |    StudioBridge capture stream -> bounded session-memory clip
      |
      +-- BEACN onboard microphone DSP ----------------------------+
                                                                  |
session-memory clip -> proven USB1 chain ingress -> same DSP ------+
                                                                  |
                                     existing, user-authorized outputs only
```

Capture and playback streams target the dedicated nodes with PipeWire's
`target.object`/`PW_KEY_TARGET_OBJECT` using a stable `node.name` or
`object.serial`, never a volatile object ID. `node.dont-reconnect=true` (or the
equivalent stream flag/property) prevents the session manager from moving a
stream to an unrelated device if its target disappears. Loss of either exact
endpoint tears down both streams and enters `Unavailable`.

The comparison client does not connect to Headphones, Audience Mix, Voice Chat
Mic, VOD Track, or Mic Relay. The validated hardware ingress must feed the same
onboard chain and therefore follows only output paths the user had already
configured. Before Play is enabled, a read-only snapshot should list every
currently observable Mic destination. If no local audible path exists, keep
the clip but explain that no monitor is available. If a non-local destination
is active, require a one-time-per-playback warning confirmation; never isolate
it by rewriting routes. If the complete destination set cannot be observed,
fail closed rather than claim that playback is private.

Directly opening the raw multichannel ALSA PCM is acceptable for the attended
characterization tool only. It is not a production design: it can contend with
PipeWire for the device and would bypass normal user-session graph ownership.

## Component boundary

Add a small cross-platform `studiobridge-comparison` crate with no dependency
on Slint, HTTP, BEACN control code, or PipeWeaver mutation commands. It owns the
state machine, buffers, commands, and events. Platform drivers implement this
trait-shaped seam:

```rust,ignore
trait ComparisonAudioDriver: Send + 'static {
    fn availability(&self) -> Availability;
    fn begin_capture(&mut self, sink: CaptureSink) -> Result<StreamToken, AudioError>;
    fn pause_capture(&mut self, token: StreamToken) -> Result<(), AudioError>;
    fn resume_capture(&mut self, token: StreamToken) -> Result<(), AudioError>;
    fn begin_playback_loop(
        &mut self,
        source: PlaybackSource,
    ) -> Result<StreamToken, AudioError>;
    fn stop_stream(&mut self, token: StreamToken) -> Result<(), AudioError>;
    fn shutdown(&mut self);
}
```

The exact Rust signatures may change to make the real-time callbacks borrow
preallocated storage safely, but the driver must not own policy or publish UI
state. A deterministic fake driver runs on Windows and in unit tests. It
accepts injected sample blocks, simulated clock advancement, stream failures,
and endpoint-generation changes; it never opens a Windows microphone. This
allows every interaction and Slint binding to be verified on Windows without
pretending that live-chain comparison works there.

The native desktop process owns one supervisor and one audio worker because the
clip is private to that login session and must disappear when the app exits.
The worker is the only state owner. Slint callbacks enqueue commands, and
worker events are the only authority for state, timer, and enabled state. The
daemon remains the sole owner of BEACN control and PipeWeaver mutations; the
comparison worker has no reference to either write client.

## Audio representation and real-time rules

Use one canonical internal format:

- mono, non-interleaved `f32`;
- 48,000 frames per second;
- maximum 480,000 frames (10.0 seconds);
- finite samples clamped to `[-1.0, 1.0]`; a non-finite input is replaced with
  zero and emits a diagnostic counter, never the sample value.

Allocate exactly two fixed sample slots before capture begins: one committed
clip and one draft/scratch slot. Each is 1,920,000 bytes, so the hard sample
storage ceiling is 3,840,000 bytes plus small metadata. Starting a recording
clears only the draft; the committed slot remains intact. Commit swaps slot
roles and scrubs the displaced slot. Cancel and capture failure scrub the draft
and restore the prior committed clip. Shutdown scrubs both slots before free.
No `Vec` growth, allocation, lock, logging, format conversion, or channel send
may occur in a PipeWire process callback.

The driver offers only the canonical format. PipeWire may adapt it to the
validated endpoint's native representation. If the stream cannot negotiate
mono 48 kHz audio, availability fails closed; the state machine must not infer
time from an unknown device rate.

A partial draft is commit-eligible after one complete 20 ms block (960 frames).
This explicit minimum prevents a double click from producing a one-sample click
while remaining below the UI's 100 ms timer resolution. The observed official
minimum is unknown, so hardware parity testing may justify changing this
constant, but it must remain named and unit-tested. A loop wraps at exactly the
committed frame count. It neither normalizes nor repeatedly fades the clip, so
looping cannot accumulate a gain change. Underrun output is zero-filled and is
a terminal playback error rather than a reason to skip forward silently.

## State machine

The model states are explicit:

```text
Unavailable { reason, retained_clip? }
Empty
Ready { clip }
Recording { prior_clip?, draft_frames, remaining_frames }
RecordingPaused { prior_clip?, draft_frames, remaining_frames }
Playing { clip, elapsed_in_loop_frames }
```

`retained_clip` is internal only and never enables actions while endpoints are
invalid. Availability recovery returns to `Ready` if that clip is still valid,
otherwise `Empty`. A monotonically increasing endpoint generation prevents a
late stream callback from reviving state after device loss.

| State and command | Transition and side effects |
| --- | --- |
| `Unavailable` + any UI action | No transition; emit the stable unavailable reason. |
| `Empty`/`Ready` + Record | Preserve the committed clip as `prior_clip`, scrub the draft, start capture, then enter `Recording`. A start failure leaves the prior state unchanged. |
| `Recording` + Record | Deactivate capture and enter `RecordingPaused` only after the driver acknowledges pause. |
| `RecordingPaused` + Record | Reactivate the same capture stream and return to `Recording`; never reset the draft cursor. |
| `Recording`/`RecordingPaused` + Play | If at least 960 frames exist, stop capture, atomically commit the draft, start loop playback, then enter `Playing`. If playback start fails, keep the newly committed clip in `Ready`. Below 960 frames, remain recording/paused and announce why Play is disabled. |
| `Recording` reaches 480,000 frames | Stop capture, atomically commit, enter `Ready`, and never accept another frame. |
| `Recording`/`RecordingPaused` + Escape or workspace exit | Stop capture, scrub the draft, restore `prior_clip`, and enter `Ready` or `Empty`. |
| `Ready` + Play | After destination safety is known/confirmed, start at frame zero and enter `Playing`. A start failure leaves `Ready`. |
| `Playing` + Play or Escape | Stop playback, rewind, enter `Ready`. |
| `Playing` + Record | Stop playback completely, preserve the clip, clear draft, start capture, enter `Recording`. No overlap is permitted. |
| `Playing` + workspace exit | Stop playback and enter `Ready`; retain the clip for this process session. |
| Any active state + endpoint loss/stream error | Stop and release every stream. Scrub an uncommitted draft, retain the last committed clip when memory integrity is known, and enter `Unavailable`. Never reconnect to a substitute endpoint. |
| Any state + shutdown | Stop streams, drain/ignore stale callbacks by generation, scrub both slots, emit one terminal event, and terminate the worker. |

Timer events are derived from accepted frame counts, not wall-clock guesses.
Recording displays remaining frames rounded to one decimal second. Playback
displays elapsed frames within the current loop against `10.0s`, matching the
observed UI even for a partial committed clip. Timer events are coalesced to 10
Hz; transitions, errors, and availability changes bypass coalescing.

## Worker protocol

Use bounded channels and sequence every command:

```text
Command (capacity 32)
  ProbeAvailability
  ToggleRecord { sequence }
  TogglePlay { sequence, destination_ack_generation? }
  Cancel { sequence }
  WorkspaceExited { sequence }
  Shutdown { sequence }

Event (bounded latest-state mailbox + terminal queue)
  Snapshot { revision, public_state, timer_tenths, controls, reason? }
  DestinationConfirmationRequired { revision, destinations, generation }
  Diagnostic { revision, kind }       # never contains samples/device serials
  ShutdownComplete { revision }
```

Commands are processed serially. A `revision` increments on every accepted
transition. Duplicate or stale sequence numbers are ignored, which makes rapid
double-click behavior deterministic. Timer snapshots may overwrite an older
unconsumed timer snapshot; terminal transitions and errors may not be dropped.
If the command queue is full, the UI reports busy and does not optimistically
change controls. On shutdown, the desktop waits for `ShutdownComplete` for a
short bounded interval and then joins the worker; the worker's drop guard still
deactivates streams and scrubs buffers.

The UI maps worker state directly to semantic button labels: `Record
comparison`, `Pause recording`, `Resume recording`, `Play comparison loop`, and
`Stop comparison playback`. Record and Play are real focusable buttons with
Enter/Space activation. Status announcements occur only on whole transitions,
not every timer tick.

## Linux driver and dependencies

The production driver should use PipeWire's native stream API (`pw_stream` via
the maintained Rust bindings) on its own PipeWire main loop. A recording stream
is `PW_DIRECTION_INPUT`; playback is `PW_DIRECTION_OUTPUT`. Both use explicit
targets, `AUTOCONNECT`, `MAP_BUFFERS`, and `RT_PROCESS`, track
`state_changed`, and dequeue/queue buffers only in `process`. Capture and
playback streams are never active simultaneously. Pausing deactivates the
capture stream rather than continuing to read and discard microphone data.

Build/runtime requirements to add only when the Linux implementation begins:

- Rust `pipewire`/`libspa` bindings compatible with the repository's supported
  PipeWire baseline;
- `libpipewire-0.3-dev` and `pkg-config` on Debian/Ubuntu,
  `pipewire-devel` on Fedora, and `pipewire` on Arch;
- PipeWire and WirePlumber in the user session; and
- a released or explicitly installed BEACN UCM profile containing only the
  physically validated comparison endpoints.

Do not reuse PipeWeaver's private Git fork of `pipewire-rs` or import
PipeWeaver's internal filter types into StudioBridge. PipeWeaver remains an
independently updated daemon, and its public v0.1.9 API does not promise a
sample-processing ABI. Select and pin a maintained binding in a separate
dependency review, then test it against the project's Ubuntu/Fedora/Arch
matrix.

## Physical validation gate

Run this only on Linux with the user present, low headphone level, streaming
and voice-chat applications stopped, and all Audience/Voice Chat/VOD/Link
routes independently confirmed inactive. The characterization utility must be
separate from normal startup and require an explicit acknowledgement before
any audio playback.

1. Record the USB descriptors, ALSA card identity, negotiated format, and UCM
   version without changing the graph or device.
2. Capture normal Mic plus raw capture channels 2 and 3 concurrently from the
   same spoken phrase. Confirm channel presence, rate, polarity, and clipping.
3. Using the existing attended DSP lease only, compare captures across at least
   two conspicuously different, reversible EQ/dynamics settings. A valid dry
   tap must remain materially invariant while normal Mic changes. Restore and
   verify the captured DSP state.
4. With every external Mic destination absent, play a quiet, bounded licensed
   speech sample into playback channel 2. Confirm which hardware outputs see
   it. Stop on any unexpected destination.
5. While the sample loops, use the existing DSP lease to make one reversible
   change. The observed output must change in real time while the stored sample
   remains byte-identical. Restore and verify DSP state.
6. Disconnect/reconnect USB1 during inactive and active streams. Confirm no
   fallback target, no stream migration, no route mutation, no stale callback,
   and no automatic replay.
7. Record the validated card/profile/version/channel capability tuple in code
   and tests. Unknown firmware or layouts remain `Unavailable`; do not broaden
   matching from one successful device.

If no capture channel is demonstrably pre-DSP or playback channel 2 does not
enter the current onboard chain, the feature is not implementable with the
known interface. Keep it unavailable and investigate an independently proven,
narrow BEACN audio endpoint. Do not fall back to baked post-DSP audio, a generic
monitor sink, an automatically created PipeWeaver route, or a software copy of
the hardware DSP.

## Staged implementation and test seams

### Stage 1 — Windows-verifiable policy core

1. Add the cross-platform state/buffer crate and deterministic fake driver.
2. Unit-test every table transition, exact 480,000-frame cap, 960-frame partial
   minimum, slot swapping, scrubbing, loop wrap, generation invalidation,
   duplicate commands, full queues, device loss, stream errors, navigation,
   and shutdown.
3. Add property tests with arbitrary command/error sequences asserting: at
   most one active stream, no action from `Unavailable`, no buffer growth, no
   draft replacing a committed clip before commit, and eventual cleanup.
4. Bind Slint to worker snapshots behind a build-time fake driver. Test button
   names, focus, Enter/Space, Escape, disabled reasons, timer rounding, and no
   optimistic transition.

Stage 1 may render and exercise the complete interaction on Windows, but its
availability label must say `Simulated comparison` in development builds and
must never ship as evidence of Linux audio support.

### Stage 2 — read-only Linux capability probe

1. Add registry discovery for the exact validated endpoint capability tuple.
2. Report availability and observable Mic destinations without opening an
   active stream.
3. Test missing, duplicate, renamed, replaced, and generation-changed nodes
   with registry fixtures. No API in this stage may create a stream or mutate a
   graph.

### Stage 3 — capture only

1. Implement explicit-target PipeWire capture into the preallocated draft.
2. Verify pause/resume, ten-second auto-commit, cancellation, disconnect, CPU,
   and real-time allocation/lock instrumentation.
3. Keep Play disabled; compare the captured tap against the physical-validation
   evidence before proceeding.

### Stage 4 — attended playback and UI completion

1. Implement explicit-target playback only for the proven chain ingress.
2. Add destination confirmation, seamless loop, stream-error handling, and
   immediate stop on navigation/device loss.
3. Hardware-test that live DSP changes affect playback and that no route,
   default, USB control, or saved profile changes during the entire workflow.
4. Extend the native safety validator to reject PipeWeaver mutation calls,
   BEACN control endpoints, file writes, generic target selection, and fallback
   autoconnect in the comparison module.

## Open blockers

- USB1 capture channels 2 and 3 have suggestive upstream comments, not verified
  semantics.
- USB1 playback channel 2 is marked unused; its destination and safe level are
  unknown.
- No released UCM endpoint currently exposes these candidate mono channels to
  PipeWire.
- It is not yet proven that all destinations receiving the injected chain can
  be observed read-only, so safe playback confirmation cannot yet be complete.
- The maintained Rust PipeWire binding/version and distro package changes need
  a focused dependency review after the hardware topology is proven.
- The official app's minimum commit duration and exact behavior when no local
  monitor exists remain unobserved; StudioBridge's 20 ms minimum and fail-closed
  monitor behavior are explicit safe choices pending evidence.

These blockers prevent Linux audio activation, not the Windows-testable state
machine, buffer ownership, worker protocol, accessibility bindings, or safety
regression tests.
