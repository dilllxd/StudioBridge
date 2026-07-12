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
- The grip at the leading edge of each source header reorders the strip by
  dragging; order is mixer state and follows saved profiles.
- The compact mute action at the bottom of each strip has a separate arrow menu
  with Mute to All, Mute to Audience, and Mute to Chat. Selecting an item changes
  the action (shown with a checkmark); pressing the main area applies that action.
- Personal and Audience master devices remain visible directly below the
  source strips. The Personal card combines its playback-device selector and
  master level, while the Audience card keeps the virtual mix and master level
  together.
- Assignments and Routing Table are contextual tabs below the mixer. They do
  not replace the mixer with unrelated full-screen pages.
- Profiles occupy a secondary right rail and do not compete with live mixer
  controls.

StudioBridge adaptation: the observed BEACN settings surface exposes a Save
Confirmation option, but its resulting dialog was not observable during the
reference session. StudioBridge therefore implements the least-surprising
meaning of that control: when enabled, Save asks before overwriting the selected
profile; when disabled, Save is immediate. The dialog supports Save/Enter,
Cancel/Escape, and backdrop dismissal.

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
available and unassigned applications stay visible.

## Routing table

- Sources are columns and output/recording destinations are rows.
- Each intersection is one large binary included/excluded control.
- The table uses positive teal checks and negative red exclusions, making the
  entire signal graph readable without opening individual source cards.

StudioBridge adaptation: render PipeWeaver targets as rows and mixer sources as
columns, with accessible toggle buttons and source colors. Do not use a long
list of source-to-target sentences.

## Microphone workspace

- A frequency graph and live output meter remain persistent while processors
  change.
- Voice EQ and enhancement controls are visually attached to that graph.
- Mic Setup, Noise Suppression, Expander, Compressor, and Headphones are a
  horizontal tab strip; selecting one swaps only the lower editor panel.
- Live speaking/peak regions provide context for gain and dynamics controls.

StudioBridge adaptation: keep a persistent read-only spectrum/EQ overview and
live meter, move the six validated DSP modules to a horizontal tab strip, and
show the guarded editor below. Preserve explicit module arming, captured-state
revert, read-back verification, and the permanent phantom-power exclusion.

## Device settings

- Device Settings presents the device name, serial number, firmware version,
  USB2 driverless-mode state, and a Legal and Regulatory action in one compact
  information card.
- Legal and Regulatory opens a measured 784-by-289 Useful Information modal
  with a title-bar close control, centered FCC/ICES notice, and a large OK
  action. Escape also dismisses the modal.

StudioBridge adaptation: preserve this read-only information hierarchy and
dialog workflow while continuing to expose USB2 driverless mode as read-only;
opening or dismissing the dialog never enters a hardware write path.

## Originality boundary

StudioBridge retains its own bridge icon, typography, teal/amber palette,
component code, spacing, labels, and Linux-specific status language. The
reference is used only to preserve familiar information hierarchy and control
relationships. No BEACN artwork, screenshots, icons, or implementation code are
distributed with StudioBridge.
