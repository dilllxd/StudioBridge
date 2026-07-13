# BEACN workflow reference for StudioBridge

This document records interaction patterns observed in the current BEACN app
through an attended remote-desktop reference session on 2026-07-11. It does not
contain BEACN source code, extracted assets, or a pixel-for-pixel design.

## Mixer

- The mixer is the primary workspace, reached from a narrow left navigation
  rail rather than a row of equally weighted top-level pages.
- Mixer, Studio, microphone, lighting, and device buttons remain visible in the
  rail on every page; Studio and its selected child workspace can be highlighted
  together.
- Sources are horizontal channel strips. Each strip keeps its color, name,
  source/application list, Personal fader, Audience fader, separate mute
  actions, and a link toggle together.
- The two faders are distinguished by their Personal/Audience icons and teal or
  neutral treatment rather than persistent text captions above the tracks. The
  compact header leaves enough blank separation below its colour rule for the
  fader tracks to begin at the same height in every strip.
- The grip at the leading edge of each source header reorders the strip by
  dragging; order is mixer state and follows saved profiles.
- The arrow at the trailing edge of a source header opens a compact 93-by-20
  Delete Knob popup anchored over the next strip. Clicking outside or pressing
  Escape dismisses it without changing the mixer.
- The compact mute action at the bottom of each strip has a separate arrow menu:
  a 112-by-60 popup anchored over the next strip, with three 20-pixel rows for
  Mute to All, Mute to Audience, and Mute to Self. The active action is shown
  with a checkmark. Selecting a row changes the main action and closes the menu;
  clicking outside or pressing Escape dismisses it without changing the action.
  Pressing the main area applies the selected action.
- Personal and Audience master devices remain visible directly below the
  source strips. The Personal card combines its playback-device selector and
  master level, while the Audience card keeps the virtual mix and master level
  together.
- The Personal playback-device control is a compact 170-by-27 field. Its menu
  opens below the field, expands to roughly 248 pixels when device names need
  more room, reserves a 20-pixel icon gutter, and uses 24-pixel rows. The active
  device remains in the field while the menu lists the alternative devices;
  clicking outside or pressing Escape dismisses the menu without changing it.
- The trailing add-source card opens an 82-by-238 popup at the plus button with
  thirteen 18-pixel rows and a reserved icon/check gutter: Mic, Game, Music,
  Chat, Browser, System, Aux 1, Aux 2, Hardware, and four Link inputs. Types
  already present remain visible but are dimmed and inert; available types are
  bold and create a strip directly. Clicking outside or pressing Escape closes
  the popup without changing the mixer.
- Assignments and Routing Table are contextual tabs below the mixer. They do
  not replace the mixer with unrelated full-screen pages.
- The contextual tab strip uses a 126-pixel Assignments tab and a 135-pixel
  Routing Table tab with a raised active fill, teal label, and two-pixel teal
  underline.
- Profiles occupy a secondary right rail and do not compete with live mixer
  controls. A profile icon in the 28-pixel application header toggles the
  entire rail; the installed settings persist that state as
  `profilesDrawerOpen`.
- The BEACN Studio and Mixer Profiles headings use independent disclosure
  chevrons. The installed settings persist profile-list expansion under
  `profileListExpanded`.
- Mixer profiles use compact 22-pixel rows. Hovering a row reveals Save,
  Duplicate, and Delete actions in that order in three trailing 16-pixel areas;
  the selected profile keeps the raised row background while its actions are
  visible. The section-level plus action creates the next numbered profile.

StudioBridge adaptation: the observed BEACN settings surface exposes a Save
Confirmation option, but its resulting dialog was not observable during the
reference session. StudioBridge therefore implements the least-surprising
meaning of that control: when enabled, Save asks before overwriting the selected
profile; when disabled, Save is immediate. The dialog supports Save/Enter,
Cancel/Escape, and backdrop dismissal.
StudioBridge persists both disclosure states locally and exposes them to mouse
and keyboard users. The BEACN Studio section remains a read-only representation:
its device-profile plus action stays unavailable because on-device profile
creation would require an excluded storage-write path. A native top-right
profile icon likewise toggles and persists the complete 285-pixel rail.

## Assignments

- Device defaults, outgoing Link assignments, and running applications appear
  in compact adjacent groups.
- Applications are shown as recognizable chips near their current destination.
- A destination can contain multiple applications. Reassignment is direct and
  does not imply a one-app-per-channel limit.
- Empty destinations remain visible as drop/assignment targets.

StudioBridge adaptation: Windows Link and Linux PipeWire application pools live
in the mixer Assignments tab. Linux application chips can be dragged directly
onto a source or the unassign target, while keyboard/touch selectors remain
available and unassigned applications stay visible. The Personal device,
recording/playback defaults, outgoing Link assignments, and application
destinations share the measured BEACN selector treatment rather than native
toolkit combo-box styling.

## Routing table

- Sources are columns and output/recording destinations are rows.
- Each intersection is one large binary included/excluded control.
- The table uses positive teal checks and negative red exclusions, making the
  entire signal graph readable without opening individual source cards.
- The table begins 179 pixels into the contextual workspace. Destination labels
  are 112 pixels wide, source columns are 90 pixels wide, and rows advance on a
  34-pixel cadence with neutral 28-pixel cells. Each state is drawn as an
  18-pixel circle: filled teal with a dark check when included, or a red ring
  with a diagonal exclusion stroke when excluded.

StudioBridge adaptation: render PipeWeaver targets as rows and mixer sources as
columns, with accessible toggle buttons and source colors. Do not use a long
list of source-to-target sentences. The physical microphone is labelled Mic
Relay in this matrix to match the familiar BEACN routing vocabulary.

## Microphone workspace

- A frequency graph and live output meter remain persistent while processors
  change.
- The 1047-pixel workspace is divided into an approximately 897-pixel editor
  and a persistent 150-pixel Mic Output column. The editor begins with an
  864-by-256 frequency graph inset 28 pixels from its leading edge, followed by
  a 150-pixel EQ/enhancement control row.
- Voice EQ and enhancement controls are visually attached to that graph.
- Voice EQ uses graph points as the band selector. Its compact card places Add
  Band, remove, preset, Advanced EQ, and Guide controls on the left; six filter
  shapes occupy a two-by-three grid beside FREQ, GAIN, and Q step controls.
- Mic Setup, Noise Suppression, Expander, Compressor, and Headphones are a
  horizontal 30-pixel tab strip; selecting one swaps only the lower editor
  panel. Their measured widths are 81, 161, 80, 101, and 108 pixels.
- Live speaking/peak regions provide context for gain and dynamics controls.
- Mic Setup reads the live microphone gain and phantom state, keeps phantom
  read-only, and combines Peak/Speaking regions with a scrolling waveform.
- Noise Suppression uses a fixed 200-pixel control column beside the response
  graph. Its control order is On, Style (Adaptive/Snapshot), Amount, then
  Sensitivity.
- Expander also uses a 200-pixel control column. It presents a large threshold
  readout, Simple/Advanced buttons, then the mode-specific values beside a
  0-to--100 dB graph with threshold and input traces.
- Compressor uses an approximately 293-pixel control column followed by
  separate Input, Attenuation, and Output meters and a Make-up Gain fader; it
  does not reuse the Expander graph.
- Headphones divides the editor at 268 and 673 pixels into Level Controls,
  Equalizer, and Amp Power panels. The EQ has three large Bass/Mids/Treble
  controls and a Subwoofer row; the observed Amp Power selection is Line Level.

StudioBridge adaptation: keep a persistent read-only spectrum/EQ overview and
live meter, move the six validated DSP modules to a horizontal tab strip, and
show the guarded editor below. Preserve explicit module arming, captured-state
revert, read-back verification, and the permanent phantom-power exclusion.
All eight validated EQ slots are drawn at their read-back frequency/gain and
act as accessible selectors. Filter type, frequency, gain, Q, Simple/Advanced,
and add/remove changes update the staged active profile only while Equalizer is
armed; the lower guarded editor mirrors the selected band with continuous
sliders. The preset row remains an honest Custom/read-back display until exact
preset payloads are captured, rather than applying guessed audio settings.
The adjacent enhancement cards mirror BEACN's four Bass Enhance styles and its
De-Esser, Bass Amount, Exciter Amount, and Exciter Frequency dials. Their
read-back values remain available in the guarded editor and accessibility tree;
mouse drag, arrow-key stepping, and the lower continuous sliders stage the exact
exposed protocol fields only while Enhancement Suite is armed. A non-zero
amount stages that processor's existing enabled field on, and zero stages it
off, matching the dial-only UI
without inventing a separate switch or hidden preset payload.
Simple/Advanced and Adaptive/Snapshot selections reflect the current read-back
state. The Noise Suppression, Expander, Compressor, and Headphones On switches
likewise reflect their module's stored enabled state. Mode and enable changes
are staged only while that exact module is armed; Apply + verify is still the
only operation that can send the staged state to the daemon.
Mic Output gain uses the exact read message and remains visibly read-only; no
setter or general hardware-write path is exposed.

## Lighting workspace

- The 1047-pixel workspace uses a 32-pixel section header followed by a
  388-pixel Lighting Options panel. Its style column is 200 pixels wide; the
  remaining panel contains Speed and Direction and Ring Brightness sliders
  aligned near the leading edge, with no decorative device preview.
- Solid Colour, Peak Meter, and Solid Spectrum are 32-pixel style rows. The
  selected style uses the same raised grey row treatment as other BEACN lists.
- A second 32-pixel header separates Other Lighting Options. The lower panel is
  split 351 pixels from its leading edge into When Muted and When USB Is
  Suspended sections.
- When Muted uses three radio rows followed by a large selected-colour swatch
  and a compact two-row palette. When USB Is Suspended uses three radio rows
  followed by a brightness slider.

StudioBridge adaptation: reproduce this geometry and interaction model as a
strictly local visual preview. Style, radio, palette, and brightness changes do
not call the daemon, do not persist a device command, and do not create any
lighting write path. The workspace states that it is preview-only while the
global hardware-write gate remains off.

## Device settings

- Device Settings presents the device name, serial number, firmware version,
  USB2 driverless-mode state, and a Legal and Regulatory action in one compact
  information card.
- The page uses a 42-pixel header. Its 755-by-285 information card is inset 13
  pixels from the leading edge and 10 pixels below the header; the device icon,
  identity, information rows, legal action, and driverless-mode row all remain
  inside that single card.
- Legal and Regulatory opens a measured 784-by-289 Useful Information modal
  with a title-bar close control, centered FCC/ICES notice, and a large OK
  action. Escape also dismisses the modal.

StudioBridge adaptation: preserve this read-only information hierarchy and
dialog workflow while continuing to expose USB2 driverless mode as read-only;
opening or dismissing the dialog never enters a hardware write path. The state
comes from the exact Studio driverless-mode read message rather than a UI
placeholder.

## Application settings

- Device power, save confirmation, beta opt-in, mixing-suite, default-device
  reset, and profile-crossfade preferences are presented as a single narrow
  column of 40-pixel rows separated by one-pixel rules.
- Each preference uses an 18-by-18 rounded square checkbox aligned eight pixels
  from the row's trailing edge. Enabled boxes are solid teal; disabled boxes
  retain a dark fill and grey outline rather than becoming pill switches.
- Hotkey assignments continue below the preferences in the same column, grouped
  by profile and mute sections.
- Selecting an unassigned hotkey opens a blocking 350-by-175 New key-mapping
  dialog. It prompts for the next key combination, keeps OK disabled until a
  valid mapping is captured, and uses 74-by-40 OK and 115-by-40 Cancel actions.
  Escape is captured as a valid mapping rather than dismissing the dialog;
  Cancel closes it without letting clicks reach the settings page underneath.

StudioBridge adaptation: preserve the same settings hierarchy and checkbox
geometry while using System Startup, System Tray, and Linux Audio wording where
the Windows-specific labels do not apply. The installed BEACN binary identifies
the tray preference internally as `openAppToSystemTray`; StudioBridge mirrors
that startup behavior, so an autostart request stays hidden only while the
preference is enabled.

## Originality boundary

StudioBridge retains its own bridge icon, typography, teal/amber palette,
component code, spacing, labels, and Linux-specific status language. The
reference is used only to preserve familiar information hierarchy and control
relationships. No BEACN artwork, screenshots, icons, or implementation code are
distributed with StudioBridge.
