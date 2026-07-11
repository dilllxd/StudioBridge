# StudioBridge

StudioBridge is an unofficial Linux control and mixing application for the
BEACN Studio in a dual-PC streaming setup.

The initial target topology is:

```text
Windows gaming PC (BEACN Link) <-- USB2 --> BEACN Studio <-- USB1 --> Linux streaming PC
```

StudioBridge combines three existing Linux capabilities behind one interface:

- Studio hardware and onboard DSP control through `beacn-lib`.
- Streaming-oriented PipeWire routing and submixes through PipeWeaver's public API.
- Remote assignment of Windows gaming applications to the four Studio Link channels.

The primary interface is now being migrated to a native Rust/Slint Linux
application with a system tray and XDG autostart. The existing web interface is
kept as a migration fallback. See
[docs/native-desktop.md](docs/native-desktop.md) for architecture and current
status.

This repository is independent of BEACN and PipeWeaver. It does not contain
BEACN firmware or code copied from the official application.

## Current status

The alpha contains mock and real backends behind the same local daemon. The real
Studio adapter discovers USB1, reads serial/firmware, microphone gain, phantom
power, headphone level, mic-monitor level, and Windows Link assignments. General
hardware writes remain disabled unless `--allow-hardware-writes` is supplied.
After the read-only baseline passes, `--enable-link-host` independently enables
only the fixed USB1 host heartbeat and Link application assignments. The complete
onboard microphone chain is readable through a separate bounded snapshot: both
Simple/Advanced profiles for eight-band EQ, compressor, and expander, plus noise
suppression, bass enhancement, de-esser, exciter, and headphone EQ. DSP writes
use independent per-module gates and never require unrestricted hardware writes.
The real PipeWeaver adapter reads and controls channels, Personal/Audience
volumes, mute targets, applications, output targets, and routes.

USB and PipeWeaver operations have a five-second service timeout. A stalled
backend is shown as disconnected instead of indefinitely blocking the UI.

Mock mode is the default, so the complete UI can be exercised without Linux or
without changing a connected Studio:

```powershell
cargo run -p studiobridge-daemon
```

The API listens on `http://127.0.0.1:17840` by default:

```text
GET  /api/health
GET  /api/state
GET  /api/studio/microphone-dsp
POST /api/studio/microphone-dsp
POST /api/studio/microphone
POST /api/studio/link-assignment
POST /api/mixer/volume
POST /api/mixer/volume-link
POST /api/mixer/mute
POST /api/mixer/route
POST /api/mixer/application
```

Run the interface in a second terminal:

```powershell
cd web
npm install
npm run dev
```

Then open `http://127.0.0.1:5173`. Production builds are served directly by the
daemon at `http://127.0.0.1:17840`. The interface follows PipeWeaver's clear
Sources/Targets/Routing model, adds a unified Windows/Linux Applications view,
and consumes PipeWeaver's read-only meter WebSocket for real source and target
levels. Personal/Audience faders can be linked or independent and each mix has a
separate mute. Windows applications can be dragged onto Link 1–4; multiple
applications may share a Link channel. The Mic Chain view shows real captured
DSP values, a live microphone meter, editable Simple/Advanced profiles only when
that exact module is armed, verified Apply, and revert to the captured baseline.

On Linux, after completing the read-only checks, start with real read-only
Studio access and PipeWeaver:

```bash
cargo run -p studiobridge-daemon -- \
  --studio beacn \
  --mixer pipeweaver
```

After this fully read-only baseline passes, run `scripts/enable-link-host.sh` to
enable the constrained Link heartbeat and assignments. This does not enable gain,
phantom-power, firmware, factory-reset, or storage writes. The broader
`--allow-hardware-writes` mode is not required for the dual-PC Link workflow.

After `scripts/validate-microphone-dsp.sh` passes, a single DSP module can be
armed with `scripts/arm-dsp-module.sh MODULE`. The helper reruns both safe
baselines and installs an override containing exactly one `--enable-dsp-write`
argument. `scripts/disarm-dsp-writes.sh` removes it. Phantom power is never part
of these module writes.

See [docs/architecture.md](docs/architecture.md) for the implementation plan.

## Safety

Hardware writes will be opt-in during early development. Mock mode performs no
USB access. Firmware update and raw onboard-storage operations are deliberately
outside the first release.

The first Linux machine check is documented in
[docs/hardware-validation.md](docs/hardware-validation.md). A read-only collector
is provided at `scripts/collect-linux-audio-info.sh`; once the service is
running, `scripts/validate-readonly.sh` verifies the real backend state and
confirms that hardware writes are still disabled.

`scripts/install-linux.sh` builds and installs a fully read-only systemd user
service plus a desktop launcher. It expects PipeWeaver to be installed
separately. The guarded `scripts/enable-link-host.sh` step refuses to proceed
unless that read-only baseline validates first.
Run `scripts/preflight-linux.sh` first for Ubuntu/Debian or Arch-family dependency,
PipeWire, UCM-profile, USB1, and PipeWeaver API checks.

For the first hardware test, current Arch is recommended over Ubuntu 24.04:
Arch already carries the BEACN UCM profiles and a PipeWire version meeting
PipeWeaver's 1.4+ recommendation. The Ubuntu-specific UCM compatibility steps
are documented in `docs/hardware-validation.md`.

Run the complete repeatable build and test suite with `scripts/verify.sh`.

## Linux Live read-only bootstrap

For a temporary Fedora or Ubuntu Live hardware check that does not install
Linux or change BEACN settings, run:

```bash
curl -fsSL https://raw.githubusercontent.com/dilllxd/StudioBridge/main/scripts/bootstrap-linux-live.sh | bash
```

The script installs build/diagnostic packages only in the disposable live
session, clones this repository, and writes a sanitized report to
`~/studiobridge-live-report.txt`. It deliberately does not install PipeWeaver,
start StudioBridge, or enable hardware writes.

After the sanitized bootstrap report passes, continue with the PipeWeaver
installation, port 14565 check, read-only StudioBridge install, and Link route
test in [docs/hardware-validation.md](docs/hardware-validation.md#continue-an-ubuntu-live-validation).
