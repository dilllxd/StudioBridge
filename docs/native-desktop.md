# Native Linux desktop client

StudioBridge is migrating from a browser-first interface to a native Rust
desktop client. The local HTTP interface remains available during migration for
diagnostics and as a fallback, but it is no longer the intended daily control
surface.

## Architecture

The native client uses [Slint](https://docs.slint.dev/latest/docs/slint/) with
its Winit backend. GPU-accelerated FemtoVG is preferred for 60 Hz meters and
the Slint software renderer remains compiled in as a compatibility fallback.
It creates a normal Wayland/X11 window and a
StatusNotifier system tray item; it does not embed Chromium, WebKit, or the old
web application. Slint supports current glibc Linux distributions with Wayland
or X11 and D-Bus, native drag-and-drop, and explicit accessibility roles and
values.

The existing daemon remains a separate process and the only component that can
reach BEACN USB or PipeWeaver. This keeps crashes, rendering, and desktop-session
behavior out of the hardware boundary:

```text
studiobridge-desktop (Slint window + tray)
        |
        | localhost typed API + meter WebSocket
        v
studiobridge-daemon (validation + write gates)
        |                         |
        v                         v
BEACN Studio USB1             PipeWeaver
```

Closing the main window hides it while the tray remains active. Launching with
`--background` creates the tray without initially showing the window. Use
`scripts/enable-desktop-autostart.sh` to install the XDG autostart entry and
`scripts/disable-desktop-autostart.sh` to remove it. These scripts do not change
the daemon or any hardware gate.

Only one desktop process runs per login session. A second normal launch asks
the existing tray process to show its window over a mode-0600 Unix socket in
`XDG_RUNTIME_DIR`; a duplicate background launch exits silently. This avoids
duplicate meter streams and conflicting edit sessions.

## Interaction model

The design follows concepts documented in the current BEACN application rather
than copying its source or visual assets:

- a visible device and module column;
- a stable measured 1404-by-810 Windows default frame that does not collapse
  when switching between Mixer, Studio, device, and application settings,
  remains user-resizable, and returns to that default on a clean restart;
- a persistent five-button device navigation hierarchy plus application
  settings at the foot of the rail, with visible focus and Return/Space
  activation;
- a measured 28-pixel application header with fixed-destination Project and
  Support actions plus the persistent Profiles control, all keyboard and
  accessibility enabled;
- a source-first mixer whose Personal and Audience faders expose named slider
  ranges, values, one-percent keyboard steps, assistive-technology value
  actions, visible focus, and keyboard-accessible mute and volume-link controls;
- draggable and keyboard-accessible source-strip grips backed by the native
  PipeWeaver order command, with saved profiles restoring the captured order;
- measured source-header Delete Knob popups with native outside-click and
  Escape dismissal plus keyboard opening and activation;
- linked or independent submix faders;
- a BEACN-matched add-source menu using the measured 82-by-238 popup and
  thirteen 18-pixel rows, keeping every source type visible, disabling occupied
  names before they reach the daemon, creating available strips directly, and
  supporting native outside/Escape dismissal plus keyboard activation;
- a persistent per-source mute action menu matching BEACN's measured
  112-by-60 popup, three 20-pixel rows, checked selection, outside/Escape
  dismissal, and keyboard activation for Mute to All, Mute to Audience, and
  Mute to Self while retaining the internal Personal target;
- a prominent Main Out section;
- BEACN-aligned Personal and Audience master-device cards backed by independent
  PipeWeaver target masters and per-target physical-output attachment, separate
  from the Linux default-playback selector;
- a shared BEACN-style device and assignment selector with the measured
  170-by-27 master field, 24-pixel alternative rows, content-width popup,
  native outside/Escape dismissal, and mouse plus arrow-key activation;
- additional target strips and routing rows for Voice Chat Mic and VOD Track;
- an Output Bus Device Assignments workspace that attaches Personal, Audience,
  Voice Chat Mic, VOD Track, and future mix buses to explicit usable PipeWire
  outputs, including BEACN Studio Link playback endpoints, with Unassigned as a
  deliberate detach state;
- functional keyboard-accessible target-master volume sliders and mute controls
  for Personal, Audience, Voice Chat Mic, VOD Track, and future PipeWeaver
  outputs, backed by PipeWeaver's exact target commands and restored by mixer
  profiles;
- measured 126/135-pixel Assignments and Routing Table tabs with native tab
  accessibility and keyboard activation;
- a measured routing matrix with 112-pixel destinations, 90-pixel source
  columns, 34-pixel row cadence, drawn teal/red state indicators, and accessible
  keyboard toggles backed by exact PipeWeaver route commands;
- PipeWeaver-backed Linux default recording and playback device selectors;
- a device-information card with the measured Legal and Regulatory modal,
  keyboard dismissal, and permanently read-only USB2 driverless-mode state;
- a BEACN-style profile rail with instant numbered creation plus hover Save,
  Duplicate, and Delete actions in measured 22-pixel rows and three 16-pixel
  action areas, with drawn native glyphs, keyboard activation, and accessible
  labels; the top-right profile icon toggles and persists the complete rail,
  while independently collapsible BEACN Studio and Mixer Profile groups retain
  their local disclosure state and on-device profile creation remains
  unavailable; the persisted Save Confirmation setting controls an attended
  overwrite dialog that supports mouse and keyboard confirmation or
  cancellation;
- BEACN-style key-mapping capture that listens for the next native key chord,
  previews its primary key, validates it, and persists the complete binding in
  a measured 350-by-175 blocking dialog with disabled-until-valid confirmation
  and BEACN-matched Escape-key capture; every assignment row has visible focus,
  keyboard activation, and a binding-aware accessible label;
- BEACN-measured application settings rows with 18-by-18 square checkboxes,
  keyboard toggling, checkbox accessibility state, Linux-specific labels, and
  an Open to System Tray preference that controls whether an autostart launch
  stays hidden while accepting the legacy `close_to_tray` settings key;
- native application chips that drag onto source strips, with accepted-drop
  highlighting, an unassign target, and accessible selector fallback;
- always-accessible voice EQ and enhancement controls;
- a functional eight-band Voice EQ editor with graph-point selection, all six
  validated filter types, guarded Simple/Advanced profiles, add/remove,
  FREQ/GAIN/Q stepping, continuous selected-band sliders, and keyboard-labelled
  controls;
- functional read-back Enhancement Suite cards with all four Bass Enhance
  styles, guarded De-Esser/Bass/Exciter dials, Exciter frequency, continuous
  editor sliders, keyboard stepping, and protocol-valid staged enabled states;
  processor selectors, modes, dials, sliders, and threshold steppers expose
  descriptive names plus native range/value/action semantics;
- secondary microphone processors grouped by module;
- a measured 897/150 microphone editor/output-column split, 864-by-256 graph,
  exact 30-pixel processor tab strip, live Mic source meter, read-back gain and
  phantom state, module-backed On switches for Noise Suppression, Expander,
  Compressor, and Headphones, plus exact read-only Mic Output Gain and USB2
  driverless-mode state;
- live profiles plus a known captured state to revert to.
- fail-closed daemon monitoring that marks the client offline within one health
  interval, clears transient Link and DSP lease indicators, blocks every stale
  mixer/profile control behind a native retry panel, and automatically refreshes
  the complete snapshot after the service recovers.
- a Headphones workspace whose level faders, channel-link indicator, and amp
  selection are device readback; amp and level controls stay intentionally
  read-only, while the three playback-EQ dials and bounded subwoofer amount are
  staged and verified only under the exclusive Headphone Equalizer lease.

StudioBridge uses its own name, icon, colors, layout code, terminology where
needed for Linux, and all-original assets.

## Safety model

The desktop client cannot bypass daemon gates. PipeWeaver volume/routing writes
remain software-only. A microphone module must be armed independently with an
explicit attended confirmation. The daemon first verifies both real backends
and reads the entire DSP chain. The in-memory lease is exclusive to one module,
lasts at most five minutes, expires inside the backend, and is never persisted.
Every apply is validated and read back before success is reported; the client
also retains the pre-edit snapshot for a one-click verified revert. Quit and
explicit completion disarm immediately, while crashes are bounded by lease
expiry. General hardware writes stay off. Phantom power is shown but never
automatically armed or changed.

For Windows UI development, the daemon also permits the identical attended
lease flow when—and only when—both backends are `mock`. That path mutates only
the in-memory mock snapshot. A mixed mock/real pair is rejected, and real USB
editing still requires the complete BEACN/PipeWeaver pair and all normal gates.

Meter websocket events are coalesced at 60 Hz and each bar interpolates between
samples on the renderer, avoiding the stepping and full-page flashes of the
browser UI. On the Ubuntu Live validation machine, the optimized FemtoVG build
used roughly 17% CPU while visible and 0.5% while hidden in the tray.

## Distribution support

The source build currently targets Rust 1.92 or newer because that is Slint's
desktop toolchain floor. `scripts/preflight-linux.sh` provides package guidance
for:

| Family | Primary package format | Native build dependency |
|---|---|---|
| Ubuntu / Debian | `.deb` | `libfontconfig1-dev` |
| Fedora / RHEL family | `.rpm` | `fontconfig-devel` |
| Arch / Manjaro | `PKGBUILD` | `fontconfig` |
| Other glibc Linux | portable bundle | Fontconfig runtime plus Wayland or X11 |

`scripts/package-linux.sh` produces a conventional `.deb` and a portable
root-filesystem `.tar.gz`; the latter is intentionally transparent and works on
other glibc distributions without relying on AppImage mount support. An RPM
spec and Arch `PKGBUILD` live under `packaging/`. Every format installs the
daemon, desktop client, web fallback, user systemd unit, desktop metadata, icon,
and udev rule from the same staging script.

Flatpak is not currently shipped. Direct USB access, a persistent host user
service, PipeWeaver IPC, and udev installation conflict with Flatpak's sandbox
and immutable packaging model. A future Flatpak should be client-only and talk
to a separately installed host daemon through a narrowly scoped D-Bus API;
granting the GUI broad device or host filesystem access would weaken the safety
boundary. AppImage has similar lifecycle and udev limitations, so the portable
tar bundle is the supported portable format for now.

Package commands:

```bash
# Build portable tar and .deb (when dpkg-deb is installed)
bash scripts/package-linux.sh

# Reuse existing release/web builds
bash scripts/package-linux.sh --format tar --skip-build
```

## Migration status

The native milestone now provides the original StudioBridge icon, native window
and tray, launch-in-background and single-instance behavior, real daemon health,
source discovery, independent Personal/Audience faders, 60 Hz interpolated live
meters, arbitrary PipeWeaver output buses (including Voice Chat Mic and VOD
Track), functional Linux default-device selectors and a cycling global hotkey,
a BEACN-style native key-mapping workflow, a BEACN-familiar assignment/routing
workspace, native drag/drop application assignment, and an attended guarded
editor for all six validated microphone DSP modules. Final distribution
validation remains in progress. The web assets continue to build and are served
by the daemon until native parity and packaging complete physical validation.
