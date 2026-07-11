# First Linux hardware validation

Do this with the Windows gaming PC left on USB2 and running the official BEACN
Link helper. Connect the Linux streaming PC to USB1.

## Before testing writes

1. Back up the existing `%USERPROFILE%/Documents/BEACN` folder on Windows.
2. Record the current Studio firmware version and microphone gain.
3. Leave phantom power off unless the connected microphone requires 48 V.
4. Do not run the official BEACN app and StudioBridge against USB1 at the same
   time.

## Collect the baseline

Run the preflight first. It recognizes Ubuntu/Debian and Arch-family systems and
prints the appropriate dependency command when something required is missing:

```bash
chmod +x scripts/preflight-linux.sh
./scripts/preflight-linux.sh
```

StudioBridge requires Rust 1.85 or later and Node.js 20.19 or later. On Ubuntu
24.04, install Rust with the official `rustup` installer because Ubuntu's Rust
1.75 package cannot compile this Edition 2024 workspace. Use a current Node.js
LTS release rather than an older distribution package.

For this particular audio setup, current Arch is the lower-friction choice:
`alsa-ucm-conf` 1.2.15.1 already includes the BEACN profiles, and its rolling
PipeWire package satisfies PipeWeaver's 1.4+ recommendation. Ubuntu 24.04 ships
older PipeWire and `alsa-ucm-conf` 1.2.10, so it needs the manual BEACN profiles
from <https://github.com/beacn-on-linux/beacn-ucm-profiles>. Ubuntu 24.04 also
needs this one-line correction in
`/usr/share/alsa/ucm2/common/pcm/split.conf`:

```diff
- Empty "${var:__Device}"
+ Empty "${var:-__Device}"
```

The preflight treats a missing BEACN profile or the known broken split setting
as an installation error because PipeWeaver cannot reliably expose the named
Studio channels without them.

Run the read-only collector and save its output:

```bash
chmod +x scripts/collect-linux-audio-info.sh
./scripts/collect-linux-audio-info.sh | tee studiobridge-audio-info.txt
```

Expected USB IDs:

- USB1: `33ae:0003`
- USB2: `33ae:4003`

On current distributions, the ALSA UCM profile should expose a `Basic` profile
and a `Link` profile. Select `Link` for the USB1 Studio device when the gaming PC
is connected.

## Safe implementation order

1. Detect USB1 and read serial/firmware only.
2. Read current microphone gain, phantom power, headphone level, and mic-monitor level.
3. Query the Windows Link application list without changing assignments.
4. Change one non-destructive value, read it back, then restore it.
5. Change one test application's Link assignment and restore it.
6. Attach PipeWeaver to the named UCM channels.

Firmware update, factory reset, and unrestricted onboard-storage writes remain
out of scope.

Full DSP and lighting editing are also deferred until the core USB1 reads and
safe opt-in writes have been validated on this exact Studio firmware.

## Install the read-only service

Once the baseline looks correct and PipeWeaver is installed:

```bash
chmod +x scripts/install-linux.sh
./scripts/install-linux.sh
```

To inspect the exact filesystem layout without sudo, udev reloads, or systemd
changes, stage it first (the build still runs unless `--skip-build` is added):

```bash
./scripts/install-linux.sh --stage /tmp/studiobridge-package
find /tmp/studiobridge-package -type f -print
```

The installed user service deliberately omits `--allow-hardware-writes`. Inspect
its status with:

```bash
systemctl --user status studiobridge.service
journalctl --user -u studiobridge.service -n 100 --no-pager
```

Then run the automated GET-only baseline. It refuses to pass unless the daemon
reports the real BEACN and PipeWeaver backends with hardware writes disabled,
and it checks the USB identity/state and mixer graph without printing the
Studio serial number:

```bash
chmod +x scripts/validate-readonly.sh
./scripts/validate-readonly.sh
```

Run the daemon manually with `--allow-hardware-writes` only after the read-only
state and Link application list match the existing Windows configuration.

Backend operations are capped at five seconds. If USB access or PipeWeaver
stalls, `/api/state` reports that backend as timed out while the daemon and UI
remain responsive.

## Linux Live first pass

When testing from Fedora or Ubuntu Live without modifying the Windows
installation, the public-repository bootstrap performs package setup and the
read-only collection in one step:

```bash
curl -fsSL https://raw.githubusercontent.com/dilllxd/StudioBridge/main/scripts/bootstrap-linux-live.sh | bash
```

Paste `~/studiobridge-live-report.txt` back into the development conversation
before installing PipeWeaver or starting the real Studio backend.
