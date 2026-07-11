# Server Codex handoff

Branch: `codex/native-desktop`

The latest commit is intentionally marked WIP because the gaming PC was not to
receive additional build dependencies. Browser tests and shell syntax pass, but
the Rust/Slint changes after the BEACN reference session still require a full
compile on the server development machine.

## First actions

```bash
git fetch origin
git switch codex/native-desktop
git pull --ff-only
cargo fmt --all
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix web test
```

Fix every Rust and Slint compiler error before making further visual changes.
Then render/run the native client and compare it with the interaction contract
in `docs/beacn-workflow-reference.md`.

## Current design direction

- Mixer is the primary workspace.
- Assignments and Routing Table are contextual tabs below it, not top-level
  pages.
- Routing is a target-row/source-column matrix.
- Windows and Linux application assignment supports many apps per destination.
- The microphone workspace keeps its EQ/spectrum overview and output meter
  visible while a horizontal module tab swaps the lower editor.
- StudioBridge uses original assets and does not ship captured BEACN images.

## New output-bus work

The WIP adds arbitrary target strips, target-master volume API plumbing, and
mock targets for `Voice Chat Mic` and `VOD Track`. Confirm that PipeWeaver's
`APICommand::SetVolumeByName(target, None, volume)` is the correct target-master
command through the existing adapter test. Read `docs/output-buses.md` before
physical configuration.

## Safety invariants

- Never add `--allow-hardware-writes` to any service or package.
- Leave phantom power uneditable.
- DSP leases remain memory-only, exclusive to one module, five minutes maximum,
  and read-back verified.
- Voice Chat Mic and VOD Track are PipeWeaver/Link routes; they do not justify
  broader USB writes.
- Firmware, factory reset, lighting, limiter guesses, and unrestricted storage
  stay excluded.

## Packaging

Package definitions under `packaging/` and `scripts/package-linux.sh` have only
received static syntax validation. On Linux, build release/web assets, run the
stage/package scripts, inspect package contents, install into a disposable
environment, and verify the packaged user service still starts read-only.
