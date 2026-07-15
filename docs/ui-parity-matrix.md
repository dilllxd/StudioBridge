# Windows UI parity matrix

This inventory records a non-mutating comparison made on 2026-07-15 between
the installed BEACN app V1.2.62 and the StudioBridge native Windows mock
runtime. Both were inspected at a 1404-by-810 outer window size. “Observed”
means the control was seen in the running app; “code-inspected” means its
StudioBridge behavior was also traced to the Slint/Rust callback. No official
screenshots, logos, device identifiers, or long-form proprietary text are
stored here.

StudioBridge's safety boundary takes precedence over visual parity. General
hardware writes, phantom power, microphone/output gain, headphone level and amp
writes, lighting writes, firmware updates, factory reset, limiter guesses, and
unrestricted device storage remain excluded. Those differences are intentional,
not unfinished UI work.

## Status and priority

- **Matched**: observed structure and code-inspected behavior are materially
  equivalent.
- **Partial**: the surface exists but an interaction, semantic mapping, or
  responsive detail differs.
- **Missing**: the observed official interaction has no StudioBridge behavior.
- **Intentional deviation**: StudioBridge deliberately remains read-only or
  substitutes a Linux-specific behavior for safety or platform correctness.
- **Unverified**: the destructive or state-changing path was not exercised.
- **P0**: misleading or incorrect behavior that can affect a guarded audio edit.
- **P1**: major functional or layout parity gap.
- **P2**: secondary interaction or visual-polish gap.
- **P3**: intentionally excluded, platform-specific, or low-value parity work.

## Highest-impact implementable gaps

| Status | Gap | Evidence and exact next action | Priority |
| --- | --- | --- | --- |
| Partial | Compressor Simple amount mapping | Advanced now exposes independent Ratio, Attack, and Release controls with honest `:1`/`ms` units and validated protocol ranges. Official Simple exposes Compress Amount, but its mapping to the current model is unknown; StudioBridge keeps that amount unavailable instead of guessing while Threshold and Makeup Gain remain editable. Capture and test the exact mapping before enabling it. | P1 |
| Intentional deviation | Noise Suppression sensitivity display | Official displays a normalized percentage (90% in the observed state), but the percentage-to-dB conversion is unknown. StudioBridge now displays the actual `sensitivity_db` read-back in dB and edits only the validated -120…-60 dB protocol range. Add a percentage only after a reversible mapping is captured and tested. | P2 |
| Partial | Equalizer presets | Official menu contains No EQ, Low Broadcast Voice, High Broadcast Voice, and Sub Bass Rolloff. StudioBridge displays only “Custom / read-back.” Implement selection only after the exact staged EQ payload for every preset is captured and tested; keep Custom for non-preset read-back. | P1 |
| Missing | Microphone comparison recorder/player | Official Mic Output has a 10-second record/play comparison control. StudioBridge draws the time, record dot, and play glyph as inert text. Implement a local-only capture/playback state machine with elapsed time, enabled states, cancellation, and no device writes. | P1 |
| Partial | Voice Chat Mic copy target affordance | Official Voice Chat Mic row exposes a small disclosure affordance and a “Copy Chat Mic Output To” prompt. StudioBridge draws a chevron but has no interaction. Add a target selector backed by explicit PipeWeaver routing, without guessing or replacing unrelated routes. | P1 |
| Partial | Global settings with no runtime effect | `beta_opt_in`, `mixing_suite_enabled`, and `meter_crossfade` are persisted and reflected in the UI but have no code-inspected consumer. Implement the behavior or mark/disable the controls until it exists; profile crossfade is the most user-visible. | P1 |
| Partial | Profile-rail collapse reflow | Both apps hide the complete 285-pixel rail and reclaim the main area. Official settings reflow across the newly available width; StudioBridge keeps a fixed 684-pixel settings column and leaves a large blank region. Make workspace widths responsive while retaining the measured default geometry. | P1 |
| Partial | Native window chrome | Default outer geometry matches, but StudioBridge shows a native Windows title bar plus its own compact header while official chrome reads as one integrated header. Decide on a Slint custom-titlebar implementation and preserve native move, resize, minimize, maximize, close, focus, and accessibility behavior. | P2 |

## Window, navigation, and profiles

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Default window | Outer window measured 1404x810; content starts at y=58. | Preferred client size accounts for Windows decoration and produces the same outer size; content starts at y=58. | Re-check at 100%, 125%, and 150% scaling; default 100% geometry is matched. | Matched / P2 |
| Minimum/resized window | Not exhaustively measured. | Minimum is 1120x700 and major workspaces can scroll. | Measure official minimum size and verify no clipped menus at StudioBridge minimum. | Unverified / P2 |
| Header | Help link, two external-link icons, and profile toggle at right. | Equivalent project/support affordances and profile toggle are wired. | Keep Linux-appropriate destinations; do not copy official artwork. | Matched / P3 |
| Left navigation | Mixer; BEACN Studio parent; Mic Chain, Lighting, Device Settings; Global Settings at bottom. | Same information architecture with StudioBridge naming. | Tighten icon shapes, indentation, selected-state dimensions, and vertical spacing using original assets only. | Partial / P2 |
| Profile toggle | Hides the full 285px rail and expands the main workspace. | Toggle is persisted and hides/reclaims the full rail. | Fix per-workspace reflow, especially Global Settings. | Partial / P1 |
| Profile groups | Studio-device and Mixer Profiles groups have independent disclosure and section-level add affordances. | Independent disclosure is persisted; mixer add is wired; on-device creation remains unavailable. | On-device creation is intentionally unavailable; its disabled state needs a concise explanation. | Intentional deviation / P3 |
| Mixer profile rows | Selected row is raised; hover reveals Load/Save/Duplicate/Delete actions; create generates the next numbered profile. | Rust/Slint callbacks and daemon endpoints exist for create, load, save, duplicate, and delete; hover actions and keyboard focus are present. | Destructive paths were not exercised in this audit; retain save confirmation preference and add undo/error feedback tests. | Unverified / P1 |
| Profile save confirmation | Optional blocking overwrite confirmation. | Preference controls a functional save-confirm modal. | Verify Enter, Escape/Cancel, focus trap, and restored focus. | Matched / P2 |

## Mixer strips and source interactions

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Default source set | Mic, Chat, Music, Browser, Game, System, and Link In. | Mock uses the same seven-source set and order. | Live order must continue to come from mixer state/profile, not hard-coded mock order. | Matched / P2 |
| Strip geometry | Compact strip with two faders, link control, app/device detail, and mute action. Horizontal scrolling leads to add tile. | Same structure, 137px strip cadence, horizontal scrolling, and add tile. | Pixel-tune typography and separators after P0/P1 behavior work. | Matched / P2 |
| Personal/Audience faders | Independent values with optional link; meters move smoothly. | Separate volume callbacks, linked state, and smoothed meter display are wired. | Hardware-in-loop latency and link behavior remain Linux validation work. | Matched / P1 |
| Source reorder | Dragging a strip changes mixer order and saved profiles preserve it. | Pointer drag and Left/Right keyboard reorder call the daemon reorder endpoint. | Validate drag across a scrolled viewport and error rollback. | Matched / P1 |
| Header menu | Small popup contains one action: Delete Knob. | Same compact one-row popup and removal callback. | Verify focus return and disabled/error state when backend refuses removal. | Matched / P2 |
| Add source menu | 82x238 popup with 13 rows: Mic, Game, Music, Chat, Browser, System, Aux 1, Aux 2, Hardware, Link In, Link 2 In, Link 3 In, Link 4 In. Occupied names are disabled. | Same dimensions, order, names, availability treatment, keyboard activation, and create callback. | Confirm Escape and click-outside dismissal on Linux compositors. | Matched / P2 |
| Mute menu | Three 20px rows; Mic observed as Mute to All, Mute to Audience, Mute to Chat, with selected checkmark. Other source behavior was not exhaustively measured. | Three-row popup, selected action, and daemon persistence are wired; Mic uses Chat while non-Mic uses Self. | Measure at least one non-Mic official menu before changing the conditional label. | Unverified / P2 |
| Physical input selector | Compact ~170x27 field; content-width popup below; selected row has checkmark. | Popup selector is backed by discovered physical input names and set-source-device callback. | Validate long names, disconnected devices, checkmark update, and reopen position. | Matched / P1 |
| Application assignment | Compact app label plus colored destination; drag changes assignment. | Linux and Link application chips support pointer drag and keyboard stepping; callbacks route to source/Link assignment APIs. | Add drag ghost/cancel feedback closer to official and test scrolling during drag. | Matched / P1 |

## Outputs, assignments, and routing

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Main Out | Personal Mix Device and Audience Mix Device cards combine selector, master level, meter, and mute. | Target strips are generated from discovered targets and include selectors/master controls; additional targets can scroll. | Keep dynamic Voice Chat Mic/VOD/future targets even where this exceeds the current official two-card view. | Matched / P1 |
| Recording/Playback assignments | Compact default-device selectors show the real current default. | PipeWeaver-backed default input/output selectors are functional and preferred defaults can be restored. | Hardware-in-loop verification is Linux-only; Windows mock interaction is present. | Matched / P1 |
| Automatic default reset | Official toggle automatically restores configured Windows defaults. | Linux-specific preference is consumed by the default-device monitor and restores saved preferred endpoints. | This is correct platform adaptation; verify reconnect races on Linux. | Intentional deviation / P1 |
| Outgoing Studio Link | Link Out through Link 4 Out assignment rows. | Dynamic Link channel assignments are exposed and application chips can target them. | Verify exact endpoint-to-Link numbering with hardware. | Matched / P1 |
| Routing table geometry | Sources are columns; Personal Mix, Audience Mix, Voice Chat Mic, and VOD Track are rows. | Dynamic table uses the same row model and supports arbitrary future targets. | Preserve dynamic extensibility; pixel-tune fixed cell widths only after Linux target counts are known. | Matched / P2 |
| Microphone column label | Current official label is Mic Relay. | Microphone source is rendered as Mic Relay. | None. | Matched / P3 |
| Route toggle | Teal included state and red excluded state; click changes route. | Accessible checkbox exposes state; mouse, Enter, and Space call set-route. | Validate focus order and rollback on failed route update. | Matched / P1 |
| Voice Chat copy target | Small row disclosure produces a copy-output prompt. | A visual chevron is present but it is not interactive. | Implement explicit target selection; see high-impact gap. | Missing / P1 |

## Microphone workspace

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Workspace geometry | EQ graph and controls above; persistent module tabs and lower editor; 150px Mic Output column; profiles at right. | Major boundaries and tab arrangement closely match at default size. | Make graph/editor scale coherently when profile rail is closed or window is resized. | Partial / P2 |
| Persistent state | Switching modules retains the microphone workspace and current state. | Selected module, staged snapshot, lease status, and read-back remain in one persistent workspace. | Add automated tab-switch tests that prove no staged values leak between modules. | Matched / P1 |
| Write safety | Official controls write directly. | Only the selected DSP module can be armed for five minutes; Apply verifies read-back; Revert restores captured state. | Intentional extra controls must remain visible and must never be removed for visual parity. | Intentional deviation / P3 |
| EQ graph/bands | Active bands appear as colored graph points; add/remove, filter type, frequency, gain, and Q controls edit the selected band. | Eight-band model, enabled-band selection, add/remove, six filter types, frequency/gain/Q, staged apply, and verify are wired. | Improve filter glyph recognizability and ensure disabled bands do not appear as active mock points. | Partial / P1 |
| EQ presets | Four observed preset choices plus custom state. | Static Custom/read-back field only. | Capture exact payloads and implement safely; see high-impact gap. | Partial / P1 |
| Advanced EQ/Guide | Independent Advanced EQ and Guide toggles. | Both toggles exist; Advanced changes active profile and Guide is local UI state. | Verify Guide overlay/content against official; current guide effect was not audited. | Unverified / P2 |
| Enhancement controls | Bass styles 1-4 plus amount, De-Esser amount, and Exciter amount/frequency. | Equivalent controls and model values are present under guarded Enhancement Suite editing. | Verify exact step sizes, ranges, help affordances, and enabled-state visuals. | Partial / P1 |
| Mic Output | Live meter with Peak/Talk zones, output gain, and 10-second comparison record/play. | Live meter and read-only output gain are present; recorder/player is decorative. | Keep output gain read-only; implement local comparison recorder/player. | Partial / P1 |
| Mic Setup | Mic Gain, history graph, peak/speaking areas, and Phantom Power. | Read-back values and graph are displayed; gain and phantom are non-interactive. | Required safety deviation. Continue to make read-only state explicit without dominating the layout. | Intentional deviation / P3 |
| Noise style | On/Off and Adaptive/Snapshot controls. | Equivalent enabled/style controls are staged behind the module lease. | Validate curve changes and snapshot acquisition behavior with hardware. | Partial / P1 |
| Noise amount | Percentage slider. | Percentage is read and written as `amount_percent`. | Verify observed range and rounding. | Matched / P2 |
| Noise sensitivity | Official percentage display. | Actual model value is rendered and edited honestly in dB across the validated -120…-60 dB range. | Capture a reversible official percentage mapping before attempting closer display parity. | Intentional deviation / P2 |
| Expander | On, threshold, Simple/Advanced, and Advanced Ratio/Attack/Release; transfer graph. | Separate values and correct units are wired for Advanced; Simple uses amount label; graph is simplified. | Validate Simple amount mapping and replace static-looking curve with a state-driven transfer display. | Partial / P2 |
| Compressor | On, threshold, Simple Compress Amount, Advanced Ratio/Attack/Release, three meters, and Makeup Gain. | Advanced independently stages Ratio, Attack, Release, and Makeup Gain with honest units and validated ranges. Simple Threshold/Makeup Gain remain editable, while Compress Amount is explicitly unavailable because its protocol mapping is unverified. | Capture and test the exact Simple Amount mapping; do not infer it from the stored ratio. | Partial / P1 |
| Headphones level/amp | Mic Monitor, Headphones, link, and four amp-power choices. | Values and choices are shown as read-only. | Required safety deviation. Do not add write callbacks. | Intentional deviation / P3 |
| Headphone EQ/subwoofer | Bass/Mids/Treble and Subwoofer controls. | Three playback-EQ controls and bounded subwoofer edit are guarded; StudioBridge also shows an overall EQ toggle. | Confirm whether the official app has an implicit/hidden enable state; avoid presenting an invented global toggle if it changes all bands. | Unverified / P1 |

## Lighting, device settings, and global settings

| Area or control | Official observed behavior | StudioBridge current state | Exact gap or acceptance check | Status / priority |
| --- | --- | --- | --- | --- |
| Lighting layout | Solid Colour, Peak Meter, Solid Spectrum; speed/direction, brightness, muted behavior/color, and USB-suspend behavior. | Layout and local preview controls mirror the observed categories. A clear preview-only banner is present. | Keep preview state local and ephemeral; no hardware callback may be added under this parity goal. | Intentional deviation / P3 |
| Device card | Device name, device information, Legal and Regulatory button, USB2 Driverless Mode. | Identity read-back, legal modal, and read-only driverless state are present. Rename affordance is absent. | Device rename would be a storage write; leave absent unless a separately reviewed safe protocol is authorized. | Intentional deviation / P3 |
| Legal modal | Separate titled modal with close, content region, and wide OK button. | Blocking in-window overlay has equivalent structure and keyboard close. | Do not reproduce official regulatory prose; use project-owned/legal text reviewed for distribution. Consider a native child window only if modal focus differs on Linux. | Partial / P3 |
| Startup setting | Boot at system startup. | Preference writes/removes Linux autostart configuration; Windows mock only persists it. | Linux packaging verification is required before calling this fully matched. | Intentional deviation / P1 |
| Open to tray | Launch in the tray/background when startup launch requests it. | Preference is consumed by startup visibility logic. | Verify tray availability/fallback under GNOME and KDE. | Matched / P1 |
| Save confirmation | Toggle controls mixer-profile overwrite confirmation. | Fully consumed by the profile-save path. | Verify persistence and keyboard flow. | Matched / P2 |
| Beta opt-in | Official setting is persisted and influences official release channel. | StudioBridge persists the boolean but has no updater/release-channel consumer. | Disable or label unavailable until an update channel exists. | Partial / P1 |
| Mixing suite enabled | Official setting enables/disables its mixing suite behavior. | StudioBridge persists the boolean but no runtime code consumes it. | Either gate the relevant suite or remove/disable the misleading toggle. | Partial / P1 |
| Profile crossfade | Official setting enables crossfade while switching profiles. | StudioBridge persists the boolean but profile loading does not consume it. | Implement bounded volume interpolation with cancellation and tests, or disable the toggle. | Partial / P1 |
| Hotkey rows | Grouped mixer-profile and source-mute rows with add/edit affordance. | Grouped rows are generated from live profiles/sources; capture and global registration are wired. StudioBridge also exposes a Linux-specific mix-device action. | Keep the Linux-specific action in a clearly separate group; validate collisions and portal denial. | Matched / P1 |
| Hotkey capture modal | 350x175 blocking dialog; key chord capture; OK disabled until a chord; Cancel. | Same dimensions and states; normalization, save, global reload, and keyboard focus are wired. | Test modifiers-only input, Escape capture/cancel semantics, duplicate chords, and Wayland portal failure. | Matched / P1 |

## Cross-cutting acceptance checklist

| Check | Current evidence | Gap / acceptance criterion | Priority |
| --- | --- | --- | --- |
| Keyboard access | Core source, route, profile, application, add/delete, and hotkey controls expose focus roles and activation keys. | Complete a screen-by-screen Tab/Shift-Tab audit, visible-focus check, modal focus trap, and focus restoration pass. | P1 |
| Screen-reader names | Many interactive Slint components have explicit accessible labels and route checked state. | Remove ambiguous glyph-only names; announce popup expanded state, selected menu rows, disabled reason, meter labels, and read-only safety state without duplication. | P1 |
| Popup dismissal | Click-outside policies are present on source/add/device menus; modals have explicit Cancel/close. | Verify Escape behavior and focus restoration consistently on Windows, X11, and Wayland. | P2 |
| Error rollback | Rust callbacks refresh after most daemon operations and show a status message. | Ensure every optimistic slider/toggle/profile/routing change restores read-back after failure and keeps the error visible long enough to read. | P1 |
| Offline behavior | Native client has daemon health/status and safe-mode handling. | Audit every page with daemon absent: navigation must remain usable, writes disabled, read-back clearly stale/unavailable, retry non-destructive. | P1 |
| Meter animation | Mixer and processor meters animate smoothly in the mock. | Validate websocket reconnect, stale-meter decay, clipping, and CPU use with real multichannel input. | P1 |
| Safety invariants | General writes are off; DSP writes require an exclusive timed lease; headphone level/amp, phantom, lighting, firmware, reset, limiter, and storage writes are excluded. | Add UI regression tests asserting excluded controls never acquire callbacks and remain non-interactive even in mock mode. | P0 |

## Safe next implementation order

1. Make inert or persist-only surfaces honest: recorder/player, copy-output,
   beta, mixing-suite, and crossfade controls must either work or visibly state
   that they are unavailable.
2. Add EQ presets only after exact payload capture; do not infer DSP values from
   labels or screenshots.
3. Make all workspace widths respond to profile-rail collapse and resized
   windows, then do the typography/icon/pixel pass.
4. Complete keyboard, accessibility, offline, and failure-rollback audits before
   hardware-in-loop Linux validation.
