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

Do not use `--allow-hardware-writes` during this workflow. After the fully
read-only baseline passes, use the separately gated Link-host step documented
below.

## Continue an Ubuntu Live validation

The following sequence is for Ubuntu 26.04 after USB1, PipeWire 1.6 or later,
the BEACN UCM `Link` profile, Rust, and Node have already passed preflight.
PipeWeaver 0.1.9 is the version used by StudioBridge's locked IPC dependencies.

Install PipeWeaver from its upstream APT repository. The installer is
interactive; select `Debian/Ubuntu (.deb via apt)` if it offers more than one
method:

```bash
curl -fsSL https://pipeweaver.github.io/pipeweaver-repo/scripts/install.sh | bash
pipeweaver-daemon --version
```

Start the daemon for this live session, wait for its API, and verify that TCP
port 14565 is listening:

```bash
pipeweaver-daemon --background >~/pipeweaver.log 2>&1 &
for attempt in {1..20}; do
  curl -fsS http://127.0.0.1:14565/api/get-devices >/dev/null && break
  sleep 0.5
done
curl -fsS http://127.0.0.1:14565/api/get-devices >/dev/null
ss -ltnp | grep ':14565'
```

Open <http://127.0.0.1:14565>. In PipeWeaver, keep its default Personal/Audience
mapping (`Mix A` is Personal and `Mix B` is Audience), then configure:

1. Add the four BEACN capture endpoints `Line5` through `Line8` as physical
   sources. These are the four Windows-to-Linux Link channels.
2. Give those sources clear names matching the Windows BEACN Link assignments.
3. Add `Headphones` as a physical target on Personal / Mix A.
4. Keep or create a virtual target such as `Audience Mix` on Audience / Mix B.
5. Route at least one test source to both targets. Do not change Studio hardware
   settings during this check.

Return to the StudioBridge checkout and install it. The base systemd unit
contains neither write-enabling flag:

```bash
cd ~/StudioBridge
if grep -Fq -- '--allow-hardware-writes' packaging/systemd/studiobridge.service; then
  echo 'Unsafe service argument found; stopping' >&2
  exit 1
fi
if grep -Fq -- '--enable-link-host' packaging/systemd/studiobridge.service; then
  echo 'Link host control must not be enabled before the baseline; stopping' >&2
  exit 1
fi
chmod +x scripts/install-linux.sh scripts/validate-readonly.sh
./scripts/install-linux.sh
systemctl --user status studiobridge.service --no-pager
curl -fsS http://127.0.0.1:17840/api/health
./scripts/validate-readonly.sh
```

The validator is GET-only. It requires Headphones, Mic, Link Line1-Line8, the
real BEACN/PipeWeaver backends, both USB write gates disabled, valid mixer graph
references, and at least one route to each of the Personal and Audience buses.

Only after that passes, enable the narrowly scoped USB1 heartbeat and Link
assignments. The helper reruns the baseline before installing its systemd
drop-in, then verifies that the broad hardware-write gate is still disabled:

```bash
chmod +x scripts/enable-link-host.sh
./scripts/enable-link-host.sh
curl -fsS http://127.0.0.1:17840/api/health
```

The health response must report `hardware_writes_enabled:false` and
`link_control_enabled:true`. The Link heartbeat is the fixed packet
`00 00 00 AC` sent at one-second intervals. Firmware update, factory reset,
raw storage, gain, phantom-power, and other unrestricted hardware writes remain
disabled.

## Microphone DSP validation and scoped writes

Capture and validate the complete onboard processing chain while all DSP module
gates remain empty:

```bash
chmod +x scripts/validate-microphone-dsp.sh
./scripts/validate-microphone-dsp.sh
curl -fsS http://127.0.0.1:17840/api/health
```

The validator is GET-only and requires 16 EQ band states across both profiles,
two compressor profiles, two expander profiles, noise suppression, bass
enhancement, de-esser, exciter, and all three headphone EQ bands. Health must
still report `hardware_writes_enabled:false` and `dsp_write_modules:[]`. The
same snapshot validates the subwoofer enabled flag and its bounded integer
amount from 0 through 10.

To edit a module, arm exactly that module after the two read-only validators
pass. Valid names are `equalizer`, `compressor`, `expander`,
`noise-suppression`, `enhancement-suite`, and `headphone-equalizer`:

```bash
chmod +x scripts/arm-dsp-module.sh scripts/disarm-dsp-writes.sh
./scripts/arm-dsp-module.sh headphone-equalizer
curl -fsS http://127.0.0.1:17840/api/health
```

The Mic Chain UI then enables only the selected module. Apply validates the
complete module before any USB command, reads the entire chain back from the
device, and refuses success unless the selected module matches. Revert writes
the captured module state and performs the same read-back check. General gain,
phantom power, firmware, reset, lighting, limiter guesses, and raw storage are
not reachable through this path. The Headphone Equalizer module includes the
three exact playback-EQ bands and BEACN's exact subwoofer amount message bundle;
it is available only under this same attended, exclusive lease. Headphone and
monitor levels, channel linking, and amp mode remain read-only and cannot be
changed through the DSP lease. The adapter compares captured subwoofer state
before writing, so band-only changes and the reversible bass probe do not emit
unchanged subwoofer commands.

The Headphone EQ protocol has a bounded automated physical probe. It requires
an explicit acknowledgement, moves only the bass playback EQ by 0.1 dB, verifies
an independent read-back, and restores the complete captured Headphone
Equalizer state, including subwoofer state, in a
`finally` path:

```bash
STUDIOBRIDGE_VALIDATE_DSP_WRITE=I_UNDERSTAND_THIS_CHANGES_HEADPHONE_EQ \
  ./scripts/validate-headphone-eq-write.sh
./scripts/disarm-dsp-writes.sh
./scripts/validate-readonly.sh --link-control
./scripts/validate-microphone-dsp.sh
```

Always leave the Live PC with `dsp_write_modules:[]` unless an attended edit is
actively being made.

For the audible test, play a known source on the Windows gaming PC and assign it
to each BEACN Link channel in turn. Confirm activity on the corresponding
PipeWeaver source and the real StudioBridge meter, plus audio in Headphones. Then
monitor or record `Audience Mix`
in OBS and confirm the same signal follows its independent Audience level. Keep
the broad Studio hardware controls disabled during this pass. PipeWeaver volume
and routing controls remain software-only; StudioBridge exposes just the four
application selectors through the constrained Link-control gate.

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
