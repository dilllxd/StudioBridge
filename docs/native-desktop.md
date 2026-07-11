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
- a source-first mixer with Personal and Audience controls;
- linked or independent submix faders;
- a prominent Main Out section;
- drag-and-drop application assignment;
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

The planned portable build is AppImage because direct USB, user systemd, and a
separately installed PipeWeaver service do not map cleanly to Flatpak's sandbox.
Flatpak remains under evaluation as a client-only package that talks to a host
daemon through a narrowly exposed socket or localhost permission.

## Migration status

The native milestone now provides the original StudioBridge icon, native window
and tray, launch-in-background and single-instance behavior, real daemon health,
source discovery, independent Personal/Audience faders, 60 Hz interpolated live
meters, and an attended guarded editor for all six validated microphone DSP
modules. Routing and application drag/drop remain the largest native parity
items. The web assets continue to build and are served by the daemon until
those views and packaging complete physical validation.
