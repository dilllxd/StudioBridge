# Voice Chat Mic and VOD Track

StudioBridge treats output buses as PipeWeaver targets. The UI and daemon do
not hard-code a two-target limit: Headphones, Audience Mix, Voice Chat Mic, VOD
Track, and future recording targets all appear as master strips and as rows in
the routing matrix. Every master strip exposes its independent PipeWeaver
volume and mute state. Muting a bus uses PipeWeaver's target-mute command rather
than replacing or forgetting its saved master volume, and mixer profiles
restore both values.

The Assignments panel keeps three device concepts separate. Recording and
playback each expose distinct Default and Default Communication rows. The rows
must remain separate in UI and state even though PipeWeaver currently has only
one Linux default per direction. Until a distinct communications-default
command exists, that row is explicitly disabled or described as mirrored; it
must not look like an independent selector that secretly invokes the ordinary
default callback. Ordinary defaults read PipeWeaver's current `defaults_id`
state and send the typed `SetDefaultInput` or `SetDefaultOutput` command.
Target device selectors list usable physical PipeWire nodes and attach them to
an individual mixer target.

The current desktop model still exposes one legacy physical-output selector per
target. The next target-slot milestone replaces that ambiguity with two
independently tracked Personal Mix slots and one Audience Mix slot. Each
Personal selector must omit the output selected in the other slot, while
Audience filters against both active Personal attachments. These are target
attachments, not Linux playback-default selection, and this paragraph describes
the required next design rather than landed UI behavior.
The four fixed outgoing Studio Link selectors instead choose which mixer target
feeds each discovered BEACN `Line4` through `Line1` playback endpoint. The
Personal Mix device hotkey cycles the default-playback candidate list; none of
these paths touch BEACN USB controls.

The target-slot implementation must track attachment roles rather than treating
all physical outputs as replaceable. Selecting a replacement validates and
attaches first, then removes only the descriptor tracked for that slot;
`Unassigned` removes only that slot. Profiles must restore stable descriptors,
never guess a missing device, and preserve Link, Voice Chat copy, and unmanaged
attachments. The current generic target-device path does not yet provide those
slot guarantees and must not be described as the final implementation.

Outgoing Link replacement also validates a usable BEACN-labelled endpoint,
attaches it to the new target before detaching it from the previous target, and
preserves unrelated outputs already attached to either target. Mixer profiles
restore Link assignments by stable Link slot and descriptor rather than stale
PipeWire node IDs.

## VOD Track

Create a PipeWeaver target named `VOD Track` and expose it to OBS as a separate
capture source. Route the sources that should be present in saved VODs to this
target. A typical streaming policy includes Mic, Game, Chat, Browser, System,
and Alerts but excludes Music. The matrix toggle is authoritative; StudioBridge
does not silently enforce or rewrite that policy.

VOD Track normally uses the Audience submix level for each source, matching the
two-fader model while retaining independent inclusion/exclusion and an
independent target master volume.

## Voice Chat Mic

Create a PipeWeaver target named `Voice Chat Mic`, connect it to the BEACN
Studio Link playback endpoint that returns audio to the gaming PC, and select
the resulting BEACN/Link microphone in the game or Windows voice-chat settings.
Route only the processed Microphone source to this target unless a deliberate
talkback mix is required.

This path is PipeWire routing plus the already validated Studio Link transport.
It does not enable BEACN gain, phantom power, firmware, factory reset, lighting,
or storage writes. The target master volume and source inclusion remain visible
in StudioBridge so a proximity-chat return can be checked at a glance.

The routing-row disclosure named Copy Chat Mic Output To is a separate optional
physical-output attachment for the Voice Chat Mic target. It is not shorthand
for changing the source-to-Voice-Chat routing matrix. `Nothing` detaches only
this copy attachment.

The core, PipeWeaver, and daemon transaction for that role is implemented.
Requests must include `device_node_id`; explicit JSON `null` means Nothing. The
service accepts only Voice Chat Mic and a uniquely resolved usable non-Link
physical node. It records the exact prior attachment multiset, attaches a new
choice first, verifies the expected full multiset, removes only the previously
tracked copy descriptor, and verifies the final multiset before committing
durable copy-role metadata and evidence. Any failure rolls physical and live
role state back to the exact prior multiset. Legacy profiles with no copy
metadata preserve the live copy; explicit Nothing removes only the tracked copy.

Copy changes and profile activation share the daemon's mixer-commit coordinator,
so persistence failure is compensated and restart state can be seeded from the
last verified durable metadata. This protects Link returns and unmanaged
attachments without silently changing routing-matrix inclusion.

The native desktop client and Slint UI do not yet expose this backend. The
measured two-level Copy Chat Mic Output To cascade, selector callbacks, request
method, pending/error state, and authoritative refresh remain the next UI work.
The drawn chevron alone is not a functional copy selector.

PipeWeaver removal is index-based: it snapshots attachments, resolves the
tracked descriptor to exactly one index, then issues the remove command. An
external actor could still reorder or change attachments between that snapshot
and command. The daemon's shared coordinator, strict preconditions, multiset
readback, and rollback fail closed for in-process changes but cannot eliminate
that external race; hardware-in-loop Linux validation remains required.

The active UI-parity priority is Mixer. Mixer and Settings are available by
default. Studio, Mic, Lighting, and Device remain default-disabled behind the
extended-workspace gate. This documentation does not claim final workspace-wide
or safety validation.

## Validation

Before going live:

1. Confirm both targets appear in the StudioBridge master-output row and routing
   matrix.
2. Play licensed test audio through Music and verify it reaches Audience Mix
   but not VOD Track when Music is excluded.
3. Speak into the stream-PC microphone and verify Voice Chat Mic meters while
   all non-mic routes to that target are excluded.
4. On the gaming PC, select the returned BEACN/Link microphone and make a local
   recording or use a game microphone test.
5. Re-run the read-only validator and confirm general hardware writes remain
   disabled.
