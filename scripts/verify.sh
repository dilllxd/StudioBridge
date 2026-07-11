#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release --locked

npm --prefix web ci
npm --prefix web test
npm --prefix web run build
npm --prefix web audit --omit=dev
node --test scripts/validate-readonly.test.mjs

bash -n scripts/preflight-linux.sh
bash -n scripts/collect-linux-audio-info.sh
bash -n scripts/install-linux.sh
bash -n scripts/validate-readonly.sh

if [[ "$(uname -s)" == "Linux" ]]; then
  stage_root="$(mktemp -d /tmp/studiobridge-stage.XXXXXX)"
  staged_daemon_pid=""
  cleanup_stage() {
    if [[ -n "$staged_daemon_pid" ]]; then
      kill "$staged_daemon_pid" 2>/dev/null || true
      wait "$staged_daemon_pid" 2>/dev/null || true
    fi
    case "$stage_root" in
      /tmp/studiobridge-stage.*) rm -rf -- "$stage_root" ;;
      *) printf 'Refusing to remove unexpected staging path: %s\n' "$stage_root" >&2 ;;
    esac
  }
  trap cleanup_stage EXIT
  bash scripts/install-linux.sh --stage "$stage_root" --skip-build
  test -x "$stage_root$HOME/.local/bin/studiobridge-daemon"
  test -r "$stage_root$HOME/.local/share/studiobridge/web/dist/index.html"
  test -r "$stage_root$HOME/.config/systemd/user/studiobridge.service"
  test -r "$stage_root$HOME/.local/share/applications/studiobridge.desktop"
  test -r "$stage_root/etc/udev/rules.d/70-studiobridge.rules"

  verify_port=$((20000 + ($$ % 20000)))
  (
    cd "$stage_root$HOME/.local/share/studiobridge"
    exec "$stage_root$HOME/.local/bin/studiobridge-daemon" --port "$verify_port"
  ) >"$stage_root/staged-daemon.log" 2>&1 &
  staged_daemon_pid=$!
  curl -fsS --retry 10 --retry-connrefused --retry-delay 0 \
    "http://127.0.0.1:$verify_port/api/health" | grep -Fq '"ok":true'
  curl -fsS "http://127.0.0.1:$verify_port/" | grep -Fq '<title>StudioBridge</title>'
  kill "$staged_daemon_pid"
  wait "$staged_daemon_pid" 2>/dev/null || true
  staged_daemon_pid=""
fi

printf 'StudioBridge verification completed successfully.\n'
