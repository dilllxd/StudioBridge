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

This repository is independent of BEACN and PipeWeaver. It does not contain
BEACN firmware or code copied from the official application.

## Current status

The alpha contains mock and real backends behind the same local daemon. The real
Studio adapter discovers USB1, reads serial/firmware, microphone gain, phantom
power, headphone level, mic-monitor level, and Windows Link assignments. Its
writes are limited to gain, phantom power, and Link assignment and remain
disabled unless `--allow-hardware-writes` is supplied. The real PipeWeaver
adapter reads and controls channels, Personal/Audience volumes, mute targets,
applications, output targets, and routes.

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
POST /api/studio/microphone
POST /api/studio/link-assignment
POST /api/mixer/volume
POST /api/mixer/mute
POST /api/mixer/route
```

Run the interface in a second terminal:

```powershell
cd web
npm install
npm run dev
```

Then open `http://127.0.0.1:5173`. Production builds are served directly by the
daemon at `http://127.0.0.1:17840`. The frontend currently mirrors the generated
operator-console reference at [docs/design/mixer-reference.png](docs/design/mixer-reference.png).

On Linux, after completing the read-only checks, start with real read-only
Studio access and PipeWeaver:

```bash
cargo run -p studiobridge-daemon -- \
  --studio beacn \
  --mixer pipeweaver
```

Only after state reads and Link discovery have been validated, opt into gain,
phantom-power, and Link-assignment writes with `--allow-hardware-writes`.

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

`scripts/install-linux.sh` builds and installs a systemd user service in
read-only hardware mode plus a desktop launcher. It expects PipeWeaver to be
installed separately.
Run `scripts/preflight-linux.sh` first for Ubuntu/Debian or Arch-family dependency,
PipeWire, UCM-profile, USB1, and PipeWeaver API checks.

For the first hardware test, current Arch is recommended over Ubuntu 24.04:
Arch already carries the BEACN UCM profiles and a PipeWire version meeting
PipeWeaver's 1.4+ recommendation. The Ubuntu-specific UCM compatibility steps
are documented in `docs/hardware-validation.md`.

Run the complete repeatable build and test suite with `scripts/verify.sh`.

## Fedora Live read-only bootstrap

For a temporary Fedora Live hardware check that does not install Fedora or
change BEACN settings, run:

```bash
curl -fsSL https://raw.githubusercontent.com/dilllxd/StudioBridge/main/scripts/bootstrap-fedora-live.sh | bash
```

The script installs build/diagnostic packages only in the disposable live
session, clones this repository, and writes a sanitized report to
`~/studiobridge-live-report.txt`. It deliberately does not install PipeWeaver,
start StudioBridge, or enable hardware writes.
