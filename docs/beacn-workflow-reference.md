# BEACN workflow reference for StudioBridge

This document records interaction patterns observed in the current BEACN app
through an attended remote-desktop reference session on 2026-07-11. It does not
contain BEACN source code, extracted assets, or a pixel-for-pixel design.

## Mixer

- The mixer is the primary workspace, reached from a narrow left navigation
  rail rather than a row of equally weighted top-level pages.
- Sources are horizontal channel strips. Each strip keeps its color, name,
  source/application list, Personal fader, Audience fader, separate mute
  actions, and a link toggle together.
- Personal and Audience master devices remain visible directly below the
  source strips.
- Assignments and Routing Table are contextual tabs below the mixer. They do
  not replace the mixer with unrelated full-screen pages.
- Profiles occupy a secondary right rail and do not compete with live mixer
  controls.

## Assignments

- Device defaults, outgoing Link assignments, and running applications appear
  in compact adjacent groups.
- Applications are shown as recognizable chips near their current destination.
- A destination can contain multiple applications. Reassignment is direct and
  does not imply a one-app-per-channel limit.
- Empty destinations remain visible as drop/assignment targets.

StudioBridge adaptation: show Windows Link and Linux PipeWire application pools
in the mixer Assignments tab, group them by destination, support keyboard/touch
selectors and drag/drop, and keep unassigned applications visible.

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

## Originality boundary

StudioBridge retains its own bridge icon, typography, teal/amber palette,
component code, spacing, labels, and Linux-specific status language. The
reference is used only to preserve familiar information hierarchy and control
relationships. No BEACN artwork, screenshots, icons, or implementation code are
distributed with StudioBridge.
