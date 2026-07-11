# StudioBridge architecture

## Supported topology

StudioBridge first targets a Linux streaming PC connected to the BEACN Studio
USB1 port. A Windows gaming PC remains connected to USB2 and continues running
the official BEACN Link helper.

```text
Windows games                  Linux stream applications
     |                                  |
BEACN Link                        PipeWire/PipeWeaver
     | 4x stereo out + 4x stereo in     |
     +----------- Studio USB2            |
                         BEACN Studio ----+ USB1
                              |
                         XLR + headphones
```

## Components

### Studio adapter

Uses `beacn-lib` as a dependency rather than duplicating its reverse-engineered
USB protocol. The alpha implements Studio discovery; serial and firmware reads;
microphone gain and phantom-power reads/writes; headphone and mic-monitor reads;
and Link application discovery/assignment. All writes are disabled by default.

Full microphone DSP editing, headphone writes, lighting, profiles, firmware
updates, factory reset, and unrestricted storage access are not part of the
hardware-ready alpha. DSP can remain configured in the Studio's onboard state
while the first Linux validation focuses on transport, routing, and safe core
controls.

Early builds must exclude firmware updates and unrestricted raw storage writes.

### Mixer adapter

Treats PipeWeaver as a separately installed audio engine. StudioBridge talks to
its HTTP/WebSocket interface on localhost. This preserves PipeWeaver updates and
keeps StudioBridge independent from PipeWire implementation details.

StudioBridge exposes the channels and routes in the independently managed
PipeWeaver profile; it does not overwrite or silently provision that profile.
For the intended studio layout, configure PipeWeaver with:

- System, Music, Browser, Chat, Game, Alerts, and Microphone sources.
- Personal and Audience submix levels.
- Headphones as the personal target.
- Audience Mix as the default OBS source.
- Four mapped Link inputs from the Windows gaming PC.
- Voice Chat Mic as the default Discord microphone.

### Local daemon

The daemon merges hardware and mixer state into a stable API for the desktop UI.
Both dependencies are represented by traits and can be replaced with mock
backends. This makes UI work and automated testing possible without Linux or
connected hardware.

### Desktop UI

The UI is a local web application served by the daemon and optionally wrapped in
a small Linux desktop shell. It should remain usable in a normal browser so the
mixer can also be adjusted from a tablet on the local network later.

## Delivery phases

1. Mock daemon and UI shell — implemented and tested.
2. Read-only Studio detection and core state inspection — implemented; hardware validation pending.
3. Opt-in gain, phantom-power, and Link assignment — implemented; hardware validation pending.
4. PipeWeaver discovery, state, volume, mute, and routing integration — implemented and wire-tested.
5. Studio Link application discovery and assignment — implemented; hardware validation pending.
6. Installer, least-privilege USB1 udev rule, systemd user service, desktop launcher, and recovery behavior — implemented and validated on Ubuntu; physical desktop validation pending.

## Linux requirements

- PipeWire 1.4 or later is preferred by PipeWeaver.
- `alsa-ucm-conf` 1.2.15 or later contains the upstream BEACN Studio profiles.
- Older distributions may need the profiles from
  `beacn-on-linux/beacn-ucm-profiles` installed manually.
- Ubuntu 24.04 also needs the known `split.conf` compatibility correction
  documented in `hardware-validation.md`.
