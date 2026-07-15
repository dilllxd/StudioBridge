# Windows UI parity matrix

This inventory records a non-mutating comparison made on 2026-07-15 between
the installed BEACN app V1.2.62 and the current StudioBridge source state. The
official app was observed in its persisted restored state at a
946-by-810 outer size with an approximately 946-by-778 usable client below the
32-pixel native title bar. Repeated right- and bottom-edge shrink attempts held
that size, establishing 946-by-810 as the verified minimum, not the canonical
first-launch default. StudioBridge accepts a 944-by-778 Slint client as that
minimum, but deliberately starts at a 1402-by-778 client (about 1404-by-810
outer on Windows) as its own working default. That choice is not evidence of a
canonical BEACN default. The official app was also measured maximized at the
1920-by-1032 desktop work area. Its intermediate and first-launch sizes remain
unresolved. “Observed” means the control was seen in the running app;
“code-inspected” means its StudioBridge behavior was also traced to the
Slint/Rust callback. No official screenshots, logos, device identifiers, or
long-form proprietary text are stored here.

StudioBridge's safety boundary takes precedence over visual parity. General
hardware writes, phantom power, microphone/output gain, headphone level and amp
writes, lighting writes, firmware updates, factory reset, limiter guesses, and
unrestricted device storage remain excluded. Those differences are intentional,
not unfinished UI work.

The active parity priority is the Mixer workspace. Mixer and Settings are
available by default. Studio, Mic, Lighting, and Device remain documented for
context but are default-disabled behind `extended-workspaces-enabled` while
their behavior is incomplete or safety-gated. This is a source-state inventory,
not a claim that full workspace or safety validation is complete.

## Status and priority

- **Matched**: observed structure and code-inspected behavior are materially
  equivalent.
- **Partial**: the surface exists but an interaction, semantic mapping, or
  responsive detail differs.
- **Missing**: the observed official interaction has no StudioBridge behavior.
- **Intentional deviation**: StudioBridge deliberately remains read-only or
  substitutes a Linux-specific behavior for safety or platform correctness.
- **Blocked by protocol evidence**: implementation would require a mapping or
  payload that has not been reversibly captured and hardware-validated.
- **Unverified**: the destructive or state-changing path was not exercised.
- **P0**: misleading or incorrect behavior that can affect a guarded audio edit.
- **P1**: major functional or layout parity gap.
- **P2**: secondary interaction or visual-polish gap.
- **P3**: intentionally excluded, platform-specific, or low-value parity work.

## Highest-impact implementable gaps

| Status | Gap | Evidence and exact next action | Priority |
| --- | --- | --- | --- |
| Partial | Structural safety regression coverage | Central daemon-write readiness guards, module-scoped timed DSP leases, independent Link gating, and excluded controls are present. Extend UI regression tests so phantom power, output/headphone gain and amp, lighting, firmware, reset, limiter guesses, unrestricted storage, and every other excluded surface can never gain a callback, even in mock mode. | P0 |
| Partial | Responsive Mixer geometry validation | Measured, capped rules now drive navigation, drawer, source strips, output cards, Assignments panels, and routing cells between the verified endpoints. Run settled drawer-open/closed visual and hit-target checks at minimum, chosen working default, and maximized sizes. | P1 |
| Partial | Personal and communications assignment semantics | Official Personal exposes two independently filtered physical-output selectors, and Recording/Playback each expose separate Default and Default Communication rows. StudioBridge currently has one Personal attachment and shares each direction's default index/callback. Model the fields independently while preserving Linux's explicit platform limitations. | P1 |
| Partial | Microphone comparison UI binding | A dependency-light policy engine and simulated worker now implement the evidence-backed state machine, bounded sample ownership, destination acknowledgement policy, generation handling, timer coalescing, and teardown. The Mic Output timer and glyphs remain inert Slint text and are not connected to that worker. Bind the simulated worker in an explicitly non-production Windows/dev path with semantic buttons, keyboard/Escape behavior, worker-authoritative status, and no optimistic UI transitions. | P1 |
| Partial | Voice Chat Mic copy target affordance | Core, PipeWeaver, and daemon now implement the guarded physical-copy transaction, durable role metadata, and committed multiset evidence. Slint still only draws the affordance: add the client method, callbacks, and two-level selector without changing source routes or unrelated outputs. | P1 |
| Partial | Accessibility and failure-flow audit | Core controls have labels and keyboard activation, but a complete screen-by-screen Tab/Shift-Tab, visible-focus, popup Escape, modal trap/restore, destructive-profile error rollback, and rapid-overlap audit remains Windows-verifiable. | P1 |
| Partial | Profile crossfade runtime consumer | The compatibility preference is visibly disabled because profile switching does not consume it. A bounded, cancellable interpolation policy and mock tests can be completed on Windows before any Linux audio integration. | P1 |
| Blocked by protocol evidence | EQ presets and Compressor Simple mapping | Exact staged EQ payloads and the official Simple-amount mapping are unknown. Keep Custom/read-back and the Simple amount unavailable until reversible captures and hardware-backed tests exist; do not infer values from labels or screenshots. | P1 |
| Intentional deviation | Noise Suppression sensitivity display | Official displays a normalized percentage, but its percentage-to-dB conversion is unknown. StudioBridge displays and edits the validated -120…-60 dB read-back honestly. Add a percentage only after a reversible mapping is captured and tested. | P2 |
| Partial | Global settings with no runtime effect | `beta_opt_in`, `mixing_suite_enabled`, and `meter_crossfade` remain persisted for compatibility, but render as non-interactive controls with a visible unavailable reason and accessible disabled description. Implement each behavior before re-enabling it. | P2 |
| Partial | Native window chrome | The verified restored/minimum outer geometry matches, but StudioBridge shows a native Windows title bar plus its own compact header while official chrome reads as one integrated header. Decide on a Slint custom-titlebar implementation and preserve native move, resize, minimize, maximize, close, focus, and accessibility behavior. | P2 |

## Window, navigation, and profiles

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Persisted restored window | Outer window measured 946x810; usable client is approximately 946x778 below the 32px native title bar. | A 944x778 Slint client produces the same 946x810 Windows outer size. | Preserve this restored endpoint without calling it the canonical default. Windows scaling variants were not independently verified. | Matched / P1 |
| Canonical default/intermediate window | Not established. A supported resize/bounds primitive was unavailable, so no 1404px or other intermediate size is authoritative for BEACN. | StudioBridge deliberately chooses a 1402x778 client (about 1404x810 outer) as its own working startup default. | Keep the official default explicitly unresolved; do not present StudioBridge's practical choice as measured BEACN evidence. | Intentional deviation / P1 |
| Minimum/resized window | Repeated right- and bottom-edge shrink attempts held the outer window at 946x810. | `min-width: 944px` and `min-height: 778px` preserve the measured client baseline. Mixer is enabled at this size; extended workspaces are not part of the default surface. | Verify settled Mixer reachability and drawer transitions without claiming all-workspace validation. | Partial / P1 |
| Maximized window | Official settled at 1920x1032 at desktop-work-area origin. Navigation widened to about 91px and the open profile drawer to about 369px. | Capped formulas now grow navigation from 55px to 91px and the open drawer from 220px to 369px across the measured width span. | Verify maximize/restore, drawer animation, and hit targets at runtime. | Partial / P1 |
| Header | Help link, two external-link icons, and profile toggle at right. | Equivalent project/support affordances and profile toggle are wired. | Keep Linux-appropriate destinations; do not copy official artwork. | Matched / P3 |
| Left navigation | Mixer; BEACN Studio parent; Mic Chain, Lighting, Device Settings; Global Settings at bottom. | Same information architecture with StudioBridge naming. | Tighten icon shapes, indentation, selected-state dimensions, and vertical spacing using original assets only. | Partial / P2 |
| Profile toggle | At 946x810, the 220px drawer opens and closes without changing the outer window. | The rebuilt safe mock preserves the same outer/client size and reallocates the drawer width to the working surface in both directions. | Keep visual state, accessible expanded state, persisted preference, and allocated width synchronized. | Matched / P1 |
| Profile groups | Studio-device and Mixer Profiles groups have independent disclosure and section-level add affordances. | Independent disclosure is persisted; mixer add is wired; on-device creation remains unavailable. | On-device creation is intentionally unavailable; its disabled state needs a concise explanation. | Intentional deviation / P3 |
| Mixer profile rows | Selected row is raised; hover reveals Load/Save/Duplicate/Delete actions; create generates the next numbered profile. | Rust/Slint callbacks and daemon endpoints exist for create, load, save, duplicate, and delete; hover actions and keyboard focus are present. | Destructive paths were not exercised in this audit; retain save confirmation preference and add undo/error feedback tests. | Unverified / P1 |
| Profile save confirmation | Optional blocking overwrite confirmation. | Preference controls a functional save-confirm modal. | Verify Enter, Escape/Cancel, focus trap, and restored focus. | Matched / P2 |

## Responsive-layout evidence (2026-07-15)

Measurements below are logical pixels from settled official-app captures at the
verified 946-by-810 restored/minimum endpoint and the 1920-by-1032 maximized
endpoint. Profile transitions were allowed to finish before boundaries were
recorded. Intermediate/default behavior remains unresolved.

| Surface and state | Observed geometry and overflow | Acceptance criterion |
| --- | --- | --- |
| Minimum, profiles open | Navigation about 55px; Mixer working surface about 670px; profile drawer about 221px. | Preserve the endpoint within measurement tolerance without labeling it default. |
| Minimum, profiles closed | Navigation remains about 55px; Mixer working surface expands to about 891px; no reserved drawer gap remains. | Toggle preserves the 946x810 outer window and settles without clipping or stale hit targets. |
| Maximized, profiles open | Navigation about 91px; Mixer working surface about 1460px; profile drawer about 369px. All seven sources plus Add fit without horizontal scrolling. | Reflow shell and Mixer content instead of retaining minimum fixed widths. |
| Maximized, profiles closed | Navigation remains about 91px and the available work surface grows to about 1829px. Source/target content remains capped near the former drawer boundary while the routing matrix recenters in the wider region. | Preserve capped content behavior and centered routing; do not stretch every component indefinitely. |
| Mixer vertical bands at minimum | Sources occupy approximately y=58..475, target cards y=475..555, contextual tabs y=555..589, and Assignments/Routing the remainder. | Keep all four bands reachable with either drawer state. |
| Mic | Profiles-open structure is 522px editor, 150px Mic Output, and 220px drawer. Compact top cards are 258/130/128px; EQ buttons are 24px and dials 56px. Every module tab and the lower workspace remain visible. | Keep all module controls reachable without reproducing the official minimum-size lower-panel clipping. Mic Output remains 150px. |
| Lighting | First panel is 200px with the remaining working width assigned to preview/configuration; profiles-closed reflow is 351px plus the remainder. | Keep preview-only behavior and preserve the two-column reflow with no hardware callbacks. |
| Device | Compact page header is 31px; the device card is 488x185; Driverless Mode uses a 14px read-only square; Legal remains an in-window modal. | Preserve the measured density and keep storage/driver mode non-interactive. |
| Settings | At the restored/minimum endpoint, content width was 440px with 29px/25px horizontal padding and visible content approximately x=85..470. | Settings remains available by default beside Mixer. Keep incomplete individual settings visibly disabled; this does not enable the Studio, Mic, Lighting, or Device workspaces. |
| Display scaling | No OS scaling setting was changed. Captures establish current logical-pixel geometry only; independent 100%, 125%, and 150% runs were not performed. | At each supported scale, verify restored minimum and maximized Mixer states plus drawer transitions. Defer other workspace scale audits until Mixer parity is complete. |

## Mixer strips and source interactions

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Default source set | Mic, Chat, Music, Browser, Game, System, and Link In. | Mock uses the same seven-source set and order. | Live order must continue to come from mixer state/profile, not hard-coded mock order. | Matched / P2 |
| Strip geometry | At minimum, strips are about 126px with 137px cadence and horizontal scrolling. At maximized/open-drawer, strips grow to about 163px with 178px cadence so all seven plus Add fit. | Width is `127 + 36t` and gap is `10 + 5t`, producing 137-to-178px cadence for clamped width factor `t`. | Verify endpoint overflow, scrolling, and drag/drop hit testing at runtime. | Partial / P1 |
| Personal/Audience faders | Independent values with optional link; meters move smoothly. | Separate volume callbacks, linked state, and smoothed meter display are wired. | Hardware-in-loop latency and link behavior remain Linux validation work. | Matched / P1 |
| Source reorder | Dragging a strip changes mixer order and saved profiles preserve it. | Pointer drag and Left/Right keyboard reorder call the daemon reorder endpoint. | Validate drag across a scrolled viewport and error rollback. | Matched / P1 |
| Header menu | Content is about 93x22 (about 117x46 including shadow) and contains only Delete Knob. Escape closes it. | Same compact one-row popup and removal callback. | Match settled shadow/density; verify Escape, outside dismissal, focus return, and backend-refusal state. | Partial / P2 |
| Add source menu | Content is about 83x238 (107x262 with shadow), with 13 18px rows: Mic, Game, Music, Chat, Browser, System, Aux 1, Aux 2, Hardware, Link In, Link 2 In, Link 3 In, Link 4 In. Occupied names are gray/inert and available names bold. Escape and click-outside close it. | Same row order and availability treatment; popup is fixed at 82x238. | Match shadow/density and verify dismissal/focus return on supported compositors. | Partial / P2 |
| Mute menu | Content is about 116x58 (140x82 with shadow), three rows: Mic uses All/Audience/Chat; observed non-Mic Chat uses All/Audience/Self. Selected row has a check and dim label. Escape closes it. | Three-row popup, selected action, and daemon persistence are wired; Mic uses Chat and non-Mic uses Self. | Match measured dimensions, checked/disabled visuals, shadow, Escape, and focus return. | Partial / P2 |
| Physical input selector | Compact ~170x27 field; content-width popup below; selected row has checkmark. | Popup selector is backed by discovered physical input names and set-source-device callback. | Validate long names, disconnected devices, checkmark update, and reopen position. | Matched / P1 |
| Application assignment | Compact app label plus colored destination; drag changes assignment. | Linux and Link application chips support pointer drag and keyboard stepping; callbacks route to source/Link assignment APIs. | Add drag ghost/cancel feedback closer to official and test scrolling during drag. | Matched / P1 |

## Outputs, assignments, and routing

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Main Out responsive geometry | At minimum/open drawer Personal/Audience are about 316/318px; minimum/closed about 396/398px; maximized/open about 515/517px. | Personal is `316 + 199t + 80r`; Audience is `318 + 199t + 80r`, where `r` supplies the minimum-width closed-drawer reflow. | Verify settled endpoint widths and dynamic additional-bus overflow. | Partial / P1 |
| Personal output selectors | Personal Mix Device exposes two stacked physical-output selectors. Each popup is about 247x104 content with five rows and omits the device currently selected in the other field. | Personal exposes one output-device index and one selector. | Add two independent attachments and mutually filtered choices; attach-first/detach-after semantics must preserve unrelated routes. | Missing / P1 |
| Audience output selector | One selector; observed content about 251x136 with six rows, checked current value, and choices filtered against Personal attachments. | One shared physical-output selector model is present. | Preserve independent Audience attachment and reproduce filtering/check state without guessing missing devices. | Partial / P1 |
| Recording/Playback assignments | Recording and Playback each expose separate Default and Default Communication rows. The observed Recording popup was about 215x264 content with 13 rows. | Both rows in each direction share one index and callback because PipeWeaver currently exposes one Linux default per direction. | Represent the rows independently in UI/state. If Linux cannot set a distinct communications default, leave that row explicitly disabled or mirrored with an explanation; do not silently present two independent writes backed by one callback. | Partial / P1 |
| Automatic default reset | Official toggle automatically restores configured Windows defaults. | Linux-specific preference is consumed by the default-device monitor and restores saved preferred endpoints. | This is correct platform adaptation; verify reconnect races on Linux. | Intentional deviation / P1 |
| Outgoing Studio Link | Link Out through Link 4 Out assignment rows. The observed Link Out popup was about 219x188 content with eight rows; the already attached physical endpoint was omitted instead of checked. | Dynamic Link channel assignments are exposed and application chips can target them. | Verify endpoint numbering with hardware and match attach/filter semantics without detaching unrelated outputs. | Partial / P1 |
| Assignments layout | Minimum/open panels are about 225/226/219px; maximized/open panels grow to about 252/251/249px and then leave blank remainder. | A workspace-derived factor drives Recording `225 + 27a`, Link `225 + 26a`, and Applications `219 + 30a`. | Verify settled widths, blank remainder, and Applications scrolling. | Partial / P1 |
| Context tabs | Assignments/Routing measured about 124/137px at minimum and 161/178px maximized/open, with raised fill, teal label, and 2px underline for the active tab. | Tabs consume the responsive Mixer layout rather than a fixed minimum-only workspace. | Verify measured endpoint widths and native tab semantics. | Partial / P2 |
| Routing table geometry | Minimum/open table is about 562px with ~85px target and ~68px source columns; minimum/closed about 734px with ~110/~89px; maximized/open about 966px with ~146/~117px. It remains centered. | Target labels use `85 + 61t + 25r`, source columns use `68 + 49t + 21r`, and the computed table is centered. | Verify all endpoint widths, dynamic rows, clipping, and hit targets. | Partial / P1 |
| Microphone column label | Current official label is Mic Relay. | Microphone source is rendered as Mic Relay. | None. | Matched / P3 |
| Route toggle | Teal included state and red excluded state; click changes route. | Accessible checkbox exposes state; mouse, Enter, and Space call set-route. | Validate focus order and rollback on failed route update. | Matched / P1 |
| Voice Chat copy target | Row disclosure opens a 150x22 first-level “Copy Chat Mic Output To” item and a 243x112 six-row physical-output submenu. Current Nothing is checked; choices are physical outputs. | Backend support is landed: Voice Chat-only validation, attach-first replacement, exact attachment-multiset readback, rollback, durable copy-role metadata, and serialized profile/copy commits. The Slint cascade and desktop client wiring are still absent. | Add the two-level selector and client callback. `Nothing` must send explicit JSON `null`; never infer absence, rewrite source routes, or detach Link/unmanaged outputs. | Partial / P1 |

### Read-only Mixer evidence ledger

- Source order was Mic, Chat, Music, Browser, Game, System, Link In, then Add.
  The Add menu order was Mic, Game, Music, Chat, Browser, System, Aux 1,
  Aux 2, Hardware, Link In, Link 2 In, Link 3 In, Link 4 In.
- The Personal top and bottom selector popups each contained five approximately
  21-pixel rows. Each omitted the output already selected in the other Personal
  slot. Audience contained six rows and omitted the Personal-attached output.
  Current selection used a leading check.
- The Recording Default popup contained 13 rows in about 215x264 content. The
  Link Out popup contained eight rows in about 219x188 content. Link Out omitted
  its already attached physical endpoint instead of marking it checked.
- The observed routing matrix was: Personal excluded Mic and included every
  other source; Audience included every source; Voice Chat Mic included only
  Mic; VOD Track included Mic, Chat, Browser, Game, System, and Link In while
  excluding Music. These are observations, not defaults StudioBridge may impose.
- The Voice Chat copy submenu contained six rows: checked Nothing plus five
  physical outputs. No source route changed during inspection.
- Escape dismissed every inspected menu without selection. Add also dismissed
  on click outside. Hover-only and arrow-key selection behavior remain
  unverified because the available inspection API could not move the pointer
  without clicking and selection could have mutated live state.
- At 946x810, closing Profiles reclaimed about 221px without changing outer
  bounds; targets and routing reflowed while source strips retained their
  minimum cadence. At 1920x1032, closing the 369px drawer left source/target
  content capped while routing recentered across the wider workspace.

## Microphone workspace

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Workspace geometry | EQ graph and controls above; persistent module tabs and lower editor; 150px Mic Output column; profiles at right. | At the verified 946x810 restored/minimum endpoint, the rebuilt safe mock holds the 522px editor, 150px Mic Output, and 220px drawer structure; closing the drawer preserves the host size and all module/lower-workspace controls remain reachable. | Deferred while Mixer parity is active; preserve the measured endpoint and safety gates. | Matched / P1 |
| Persistent state | Switching modules retains the microphone workspace and current state. | Selected module, staged snapshot, lease status, and read-back remain in one persistent workspace. | Add automated tab-switch tests that prove no staged values leak between modules. | Matched / P1 |
| Write safety | Official controls write directly. | Only the selected DSP module can be armed for five minutes; Apply verifies read-back; Revert restores captured state. | Intentional extra controls must remain visible and must never be removed for visual parity. | Intentional deviation / P3 |
| EQ graph/bands | Active bands appear as colored graph points; add/remove, filter type, frequency, gain, and Q controls edit the selected band. | Eight-band model, enabled-band selection, add/remove, six filter types, frequency/gain/Q, staged apply, and verify are wired. | Improve filter glyph recognizability and ensure disabled bands do not appear as active mock points. | Partial / P1 |
| EQ presets | Four observed preset choices plus custom state. | Static Custom/read-back field only. | Capture exact payloads and implement safely; see high-impact gap. | Partial / P1 |
| Advanced EQ/Guide | Independent Advanced EQ and Guide toggles. | Both toggles exist; Advanced changes active profile and Guide is local UI state. | Verify Guide overlay/content against official; current guide effect was not audited. | Unverified / P2 |
| Enhancement controls | Bass styles 1-4 plus amount, De-Esser amount, and Exciter amount/frequency. | Equivalent controls and model values are present under guarded Enhancement Suite editing. | Verify exact step sizes, ranges, help affordances, and enabled-state visuals. | Partial / P1 |
| Mic Output | Live meter with Peak/Talk zones, output gain, and a ten-second resumable recorder with continuously looping playback through the current microphone chain. | Live meter and read-only output gain are present. The comparison policy engine and simulated worker exist, but the Slint row remains static `"10.0s / 10.0s"`, red `●`, and green `▶` text with no binding. | Keep output gain read-only. Bind semantic controls to worker-authoritative snapshots in a dev-only simulated path; production playback remains unavailable until validated endpoints exist. | Partial / P1 |
| Mic Setup | Mic Gain, history graph, peak/speaking areas, and Phantom Power. | Read-back values and graph are displayed; gain and phantom are non-interactive. | Required safety deviation. Continue to make read-only state explicit without dominating the layout. | Intentional deviation / P3 |
| Noise style | On/Off and Adaptive/Snapshot controls. | Equivalent enabled/style controls are staged behind the module lease. | Validate curve changes and snapshot acquisition behavior with hardware. | Partial / P1 |
| Noise amount | Percentage slider. | Percentage is read and written as `amount_percent`. | Verify observed range and rounding. | Matched / P2 |
| Noise sensitivity | Official percentage display. | Actual model value is rendered and edited honestly in dB across the validated -120…-60 dB range. | Capture a reversible official percentage mapping before attempting closer display parity. | Intentional deviation / P2 |
| Expander | On, threshold, Simple/Advanced, and Advanced Ratio/Attack/Release; transfer graph. | Separate values and correct units are wired for Advanced; Simple uses amount label; graph is simplified. | Validate Simple amount mapping and replace static-looking curve with a state-driven transfer display. | Partial / P2 |
| Compressor | On, threshold, Simple Compress Amount, Advanced Ratio/Attack/Release, three meters, and Makeup Gain. | Advanced independently stages Ratio, Attack, Release, and Makeup Gain with honest units and validated ranges. Simple Threshold/Makeup Gain remain editable, while Compress Amount is explicitly unavailable because its protocol mapping is unverified. | Capture and test the exact Simple Amount mapping; do not infer it from the stored ratio. | Partial / P1 |
| Headphones level/amp | Mic Monitor, Headphones, link, and four amp-power choices. | Values and choices are shown as read-only. | Required safety deviation. Do not add write callbacks. | Intentional deviation / P3 |
| Headphone EQ/subwoofer | Bass/Mids/Treble and Subwoofer controls. | Three playback-EQ controls and bounded subwoofer edit are guarded; StudioBridge also shows an overall EQ toggle. | Confirm whether the official app has an implicit/hidden enable state; avoid presenting an invented global toggle if it changes all bands. | Unverified / P1 |

### Microphone comparison recorder evidence and state machine

This non-destructive audit used only local microphone recording and playback in
BEACN App V1.2.62. It did not change DSP, output gain, routing, device settings,
or any StudioBridge write gate. BEACN's official support documentation confirms
that the clip is replayed through the microphone chain, current chain changes
are reflected in real time, and playback loops until explicitly stopped:
[Getting Started with BEACN Mic](https://support.beacn.com/en-US/getting-started-with-beacn-mic-282017)
and [test recording feature](https://support.beacn.com/en-US/how-to-use-the-test-recording-feature-of-beacn-studio-420869).

| Evidence area | Official V1.2.62 observation | StudioBridge acceptance criterion |
| --- | --- | --- |
| Geometry and controls | The comparison row is about 31px high at the bottom of the 150px Mic Output column. It contains a one-decimal `elapsed-or-remaining / 10.0s` timer, a red filled-circle record control, and a green right-triangle play control. The glyph does not change to a square or pause symbol; the active control receives a cyan/blue rounded highlight. | Preserve the compact row and one-decimal timer, but implement semantic buttons rather than clickable text. Active state must be visible without relying on color alone. |
| Labels and tooltips | No adjacent Record/Play text or tooltip appeared after the pointer rested on either control. UI Automation exposed the timer as text and both controls only as unnamed buttons. | Expose `Record comparison`, `Pause recording`, `Resume recording`, `Play comparison loop`, and `Stop comparison playback` names as state changes. Announce timer/status changes without speaking every 100ms tick. Tooltips may improve on official behavior. |
| Initial/empty state | This running BEACN process already had a playable clip when Mic Chain opened, so the official fresh-process/no-clip appearance and Play-disabled styling were not observed. The idle completed-clip row showed `10.0s / 10.0s` with red Record and green Play. | `Empty` must be explicit: show the ten-second capacity, enable Record, disable Play semantically and visually, and explain `Record a clip to enable playback`. Do not infer clip availability from timer text or glyph color. |
| Start and pause | Record starts a fresh ten-second draft and counts remaining time down in tenths. Clicking Record again pauses at the current remaining time; the value then stays fixed. | `Ready/Empty -> Recording(10s)`. Record while recording enters `RecordingPaused(remaining)`. Audio capture must stop while paused without discarding either the draft or the previously committed clip. |
| Resume and automatic completion | Clicking Record while paused resumes from the remaining time rather than restarting. The audit measured 9.8s paused, then 9.3s after resume/stop. At zero it stops automatically and leaves a playable clip. | `RecordingPaused -> Recording` from the same cursor. At zero, atomically commit the draft and enter `Ready(clip)`; cap at ten seconds and never append beyond it. |
| Cancel and re-record | Escape during a new recording canceled the draft and restored the prior completed clip; its longer playback remained available. Starting Record after a completed clip begins a new ten-second draft. | Maintain `prior_clip` until a draft is committed. Escape/cancel restores it. Re-record must be transactional so capture or device errors never destroy the last good clip. |
| Play while recording | Clicking Play during recording stopped/committed the partial draft and immediately entered playback; the timer changed from remaining-time countdown to elapsed-time count-up. | `Recording` or `RecordingPaused` + Play commits a non-empty partial draft, stops capture, and enters `Playing`. Define and test the minimum non-empty duration; keep Play disabled for an empty draft. |
| Playback and looping | Play counts elapsed time upward in tenths against the fixed `10.0s` denominator. Official documentation states that the captured duration loops continuously until Play is clicked again. The same green triangle remains visible with an active highlight. Stopping playback returned the display to `10.0s / 10.0s`. | `Ready + Play -> Playing(elapsed=0)`. Loop only the committed duration, without gaps or cumulative level change. Play while playing stops immediately, rewinds, and returns to Ready. Never send the clip to Audience/Voice Chat/VOD unless an already-authorized route explicitly does so. |
| Record while playing | Clicking Record during playback stopped playback and began a fresh recording at roughly 10.0 seconds remaining. | `Playing + Record -> Recording(10s)` with the old clip retained transactionally until the new draft commits. No playback/capture overlap. |
| Live DSP comparison | Official support says the clip is replayed through the microphone chain and current microphone-chain changes are reflected in real time. Playback continued to be available beside every Mic module; no DSP value was changed during this audit. | Capture from the correct pre-processing tap and inject playback through the same processing path as live speech. If the Linux graph cannot provide that topology, label A/B unavailable instead of replaying a baked processed file and implying live comparison. Recorder playback itself must never arm or write DSP. |
| Monitoring/routing | Official documentation says BEACN Studio playback is audible through directly connected headphones or an already-enabled Mic Relay route. The audit did not alter routing. | Use an explicit, existing local monitor target. If none is available, keep the clip and explain why it is inaudible; never enable Mic Relay, create a route, or change a default device automatically. Prevent feedback by excluding the playback stream from its capture source. |
| Navigation | Navigating away from Mic Chain during recording canceled the draft and restored the prior clip. Navigating away during playback stopped playback. Returning showed the idle comparison controls. Switching among Mic module tabs leaves the comparison row mounted. | On workspace exit, synchronously stop playback/capture, release local stream handles, cancel only an uncommitted draft, and retain the prior clip for the session. Module-tab changes must not reset the recorder. |
| Device loss | Physical disconnect was not performed in this audit, so official behavior remains unverified. | Device loss must enter `Unavailable`: stop streams, release handles, preserve the last committed in-memory clip when safe, disable actions with a named reason, and recover to Ready/Empty only after the same source/monitor endpoints are valid. No reconnect-time route or hardware writes. |
| Lifetime and storage | The installed app already had a playable sample when this audit opened, but process-restart persistence was not tested. No export, save, or file control was visible. | Treat clips as private session memory by default, erase them on app exit, never write them to unrestricted storage, and expose no export until separately authorized. Bound memory for ten seconds at the selected format and scrub discarded buffers. |
| Keyboard/accessibility | Pointer actions worked. The controls appeared as unnamed UI Automation buttons; focus remained reported at the window, and a reliable Tab/keyboard activation path was not demonstrated. | Both controls must be in logical Tab order with visible focus, Enter/Space activation, accurate button/toggle state, and deterministic focus after state transitions. Escape cancels a draft or stops playback without leaving an audio stream running. |

The comparison crate now implements explicit `Unavailable`, `Empty`, `Ready`,
`Recording`, `RecordingPaused`, and `Playing` policy states rather than inferring
state from timer text. It owns two fixed sample slots, sanitizes and scrubs
bounded buffers (480,000 frames maximum; 960-frame minimum), and fails closed on
stale generations, stream errors, teardown, or invalid destination state. Its
simulated non-UI worker has a bounded 32-command queue, worker-authoritative
snapshots, coalesced 10Hz frame-derived timer updates, ordered terminal events,
and joined teardown. The destination policy requires complete snapshots and
exact one-time acknowledgement from every non-local destination while allowing
an explicitly local-only monitor; destination generation changes stop active
playback. The crate has only the workspace `zeroize` dependency. The currently
verified scoped backend run reports 67 passing tests; desktop validation
checkpoints report 54, 54, and 58 passing tests. These scoped counts do not
assert final workspace-wide or safety validation.

This is still a simulation/policy implementation, not production audio. It is
not bound to Slint, the desktop runtime, PipeWire, HTTP, or BEACN hardware. The
next Windows-verifiable step is to connect the simulated worker to semantic UI
buttons and test focus, Enter/Space, Escape, navigation, unavailable state, and
worker error rendering without enabling audio. Production comparison must stay
disabled until a production destination observer and physically validated
pre-DSP capture plus live-chain ingress exist.

## Lighting, device settings, and global settings

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Lighting layout | Solid Colour, Peak Meter, Solid Spectrum; speed/direction, brightness, muted behavior/color, and USB-suspend behavior. | The preview-only layout matches the 946x810 reflow: 200px plus remaining width with profiles open and 351px plus remaining width when closed. A clear preview-only banner is present. | Keep preview state local and ephemeral; no hardware callback may be added under this parity goal. | Intentional deviation / P3 |
| Device card | Device name, device information, Legal and Regulatory button, USB2 Driverless Mode. | The 31px header and 488x185 card match the compact official density. Identity read-back, legal modal, and a 14px read-only driverless square are present; rename is absent. | Device rename would be a storage write; leave absent unless a separately reviewed safe protocol is authorized. | Intentional deviation / P3 |
| Legal modal | Separate titled modal with close, content region, and wide OK button. | Blocking in-window overlay has equivalent structure and keyboard close. | Do not reproduce official regulatory prose; use project-owned/legal text reviewed for distribution. Consider a native child window only if modal focus differs on Linux. | Partial / P3 |
| Startup setting | Boot at system startup. | Preference writes/removes Linux autostart configuration; Windows mock only persists it. | Linux packaging verification is required before calling this fully matched. | Intentional deviation / P1 |
| Open to tray | Launch in the tray/background when startup launch requests it. | Preference is consumed by startup visibility logic. | Verify tray availability/fallback under GNOME and KDE. | Matched / P1 |
| Save confirmation | Toggle controls mixer-profile overwrite confirmation. | Fully consumed by the profile-save path. | Verify persistence and keyboard flow. | Matched / P2 |
| Beta opt-in | Official setting is persisted and influences official release channel. | StudioBridge preserves the stored boolean but visibly disables the control with `Update channels are not implemented in this build.` | Implement a verified updater/release-channel consumer before enabling the control. | Partial / P2 |
| Mixing suite enabled | Official setting enables/disables its mixing suite behavior. | StudioBridge preserves the stored boolean but visibly disables the control with an explicit no-runtime-consumer reason. | Define and test the exact feature boundary before enabling this compatibility preference. | Partial / P2 |
| Profile crossfade | Official setting enables crossfade while switching profiles. | StudioBridge preserves the stored boolean but visibly disables the control because profile switching does not consume it. | Implement bounded volume interpolation with cancellation and tests before enabling the toggle. | Partial / P1 |
| Hotkey rows | Grouped mixer-profile and source-mute rows with add/edit affordance. | Grouped rows are generated from live profiles/sources; capture and global registration are wired. The compact 440px settings column keeps current rows visible at 946x810. StudioBridge also exposes a Linux-specific mix-device action. | Keep the Linux-specific action in a clearly separate group; validate collisions and portal denial. | Matched / P1 |
| Hotkey capture modal | 350x175 blocking dialog; key chord capture; OK disabled until a chord; Cancel. | Same dimensions and states; normalization, save, global reload, and keyboard focus are wired. | Test modifiers-only input, Escape capture/cancel semantics, duplicate chords, and Wayland portal failure. | Matched / P1 |

## Cross-cutting acceptance checklist

| Check | Current evidence | Gap / acceptance criterion | Priority |
| --- | --- | --- | --- |
| Keyboard access | Core source, route, profile, application, add/delete, and hotkey controls expose focus roles and activation keys. | Complete a screen-by-screen Tab/Shift-Tab audit, visible-focus check, modal focus trap, and focus restoration pass. | P1 |
| Screen-reader names | Many interactive Slint components have explicit accessible labels and route checked state. | Remove ambiguous glyph-only names; announce popup expanded state, selected menu rows, disabled reason, meter labels, and read-only safety state without duplication. | P1 |
| Popup dismissal | Click-outside policies are present on source/add/device menus; modals have explicit Cancel/close. | Verify Escape behavior and focus restoration consistently on Windows, X11, and Wayland. | P2 |
| Error rollback | Every daemon mutation now performs a bounded read-back refresh; accepted requests are not labeled confirmed, profile mutations require a successful profile-list read-back, and preferred defaults persist only after the snapshot matches. | Validate rapid overlapping mutations and error-message dwell time with real PipeWire/Link state. | P1 |
| Offline behavior | Rebuilt Windows QA stopped the safe mock daemon with Add Source open: the category-specific banner appeared, navigation remained usable, popup actions disabled, centralized callback guards refused writes, and restart reconnected automatically with hardware writes and Link control still off. | Repeat the screen-by-screen audit under Linux, including assistive-technology disabled-state announcements and real daemon crash/restart behavior. | P1 |
| Meter animation | Mixer and processor meters animate smoothly in the mock. | Validate websocket reconnect, stale-meter decay, clipping, and CPU use with real multichannel input. | P1 |
| Safety invariants | General writes are off; every daemon-write callback has a central readiness guard; DSP writes require an exclusive module-scoped timed lease; Link authorization is independent; headphone level/amp, phantom, lighting, firmware, reset, limiter, and unrestricted storage writes are excluded. The comparison crate is isolated from Slint, HTTP, hardware, and daemon mutation dependencies, and static validation covers its ownership and teardown constraints. | Extend structural UI tests so every excluded visual control is proven callback-free and non-interactive even in mock mode. Preserve the packaged service's read-only posture. | P0 |

## Prioritized remaining gaps by validation boundary

| Boundary | Priority | Remaining work | Completion evidence |
| --- | --- | --- | --- |
| Windows now | P0 | Expand structural safety regression coverage for every excluded control and mutation callback. | Source-level/callback tests fail if an excluded control gains a handler or bypasses the central ready gate. |
| Windows now | P1 | Runtime-verify the landed responsive Mixer shell/strip/target/assignment/routing formulas, then implement explicit target slots and separate communications-default state. | Verified minimum, chosen working default, and maximized open/closed drawer screenshots meet the evidence ledger; two Personal slots and one Audience slot preserve unrelated attachments. |
| Windows now | P1 | Wire the landed Voice Chat Mic copy backend into the desktop client and Slint cascade. | Selection never creates, replaces, or guesses source routes or unrelated outputs; explicit `null` selects Nothing and rejection restores authoritative state. |
| Windows now | P1 | Complete Mixer keyboard, accessibility, popup, profile-drawer, and error-rollback QA. | Mixer Tab order, visible focus, names/states, Escape, focus restoration, rapid overlap, and rejection are repeatable in the safe mock. |
| Deferred Windows | P1 | Bind simulated comparison UI and implement profile-crossfade policy only after Mixer parity. | Incomplete non-Mixer controls remain default-disabled until their consumers and tests exist. |
| Protocol/hardware evidence | P1 | Capture exact EQ preset payloads, Compressor Simple mapping, Noise percentage mapping, endpoint-to-Link numbering, and hardware write/read-back behavior. | Reversible captures and hardware-backed tests exist; no value is inferred from the official UI. |
| Linux/audio topology | P1 | Supply production comparison destination observation and physically validate pre-DSP capture plus live-chain ingress. | Fail-closed production endpoints prove local monitoring, acknowledgements, feedback exclusion, generation changes, and teardown on real PipeWire topology. |
| Linux/platform | P1 | Validate PipeWire defaults and reconnect races, global hotkeys/portal denial, tray/autostart packaging, meters, Link, and daemon crash/restart. | GNOME/KDE and X11/Wayland packaging/runtime matrices pass with hardware writes still gated. |

## Safe next implementation order

1. Runtime-verify the landed responsive Mixer geometry, then implement the
   target-slot model (two Personal, one Audience), independently represented
   communications rows, and the Voice Chat copy client/Slint cascade.
2. Complete Mixer keyboard, focus, popup-dismissal, offline, rollback, and
   rapid-overlap tests while extending structural safety coverage.
3. Keep Studio, Mic, Lighting, and Device default-disabled. Settings remains
   available, with incomplete individual controls visibly disabled.
4. Resume DSP mappings, comparison audio, profile crossfade, and Linux physical
   validation only under their existing evidence and safety prerequisites.
