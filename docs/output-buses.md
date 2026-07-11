# Voice Chat Mic and VOD Track

StudioBridge treats output buses as PipeWeaver targets. The UI and daemon do
not hard-code a two-target limit: Headphones, Audience Mix, Voice Chat Mic, VOD
Track, and future recording targets all appear as master strips and as rows in
the routing matrix.

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
