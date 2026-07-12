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
  when switching between Mixer, Studio, device, and application settings;
- a persistent five-button device navigation hierarchy plus application
  settings at the foot of the rail;
- a source-first mixer with Personal and Audience controls;
- draggable and keyboard-accessible source-strip grips backed by the native
  PipeWeaver order command, with saved profiles restoring the captured order;
- linked or independent submix faders;
- a persistent per-source mute action menu matching Mute to All, Mute to
  Audience, and Mute to Chat while retaining the internal Personal target;
- a prominent Main Out section;
- BEACN-aligned Personal and Audience master-device cards backed by the real
  default-output selector and independent PipeWeaver target masters;
- additional target strips and routing rows for Voice Chat Mic and VOD Track;
- PipeWeaver-backed Linux default recording and playback device selectors;
- a BEACN-style profile rail with instant numbered creation plus hover Save,
  Duplicate, and Delete actions, with the persisted Save Confirmation setting
  controlling an attended overwrite dialog that supports mouse and keyboard
  confirmation or cancellation;
- BEACN-style key-mapping capture that listens for the next native key chord,
  previews its primary key, validates it, and persists the complete binding;
- native application chips that drag onto source strips, with accepted-drop
  highlighting, an unassign target, and accessible selector fallback;
- always-accessible voice EQ and enhancement controls;
- secondary microphone processors grouped by module;
- live profiles plus a known captured state to revert to.

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
