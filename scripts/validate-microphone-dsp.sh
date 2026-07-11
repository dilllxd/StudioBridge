#!/usr/bin/env bash
set -euo pipefail

base_url="${1:-http://127.0.0.1:17840}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d /tmp/studiobridge-dsp.XXXXXX)"
cleanup() {
  case "$work" in
    /tmp/studiobridge-dsp.*) rm -rf -- "$work" ;;
    *) printf 'Refusing to remove unexpected validation path: %s\n' "$work" >&2 ;;
  esac
}
trap cleanup EXIT

curl -fsS --max-time 8 "$base_url/api/health" >"$work/health.json"
curl -fsS --max-time 35 "$base_url/api/studio/microphone-dsp" >"$work/dsp.json"
node "$root/scripts/validate-microphone-dsp.mjs" "$work/health.json" "$work/dsp.json"
