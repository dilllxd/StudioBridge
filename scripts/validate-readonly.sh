#!/usr/bin/env bash
set -euo pipefail

expect_link_control=""
if [[ "${1:-}" == "--link-control" ]]; then
  expect_link_control="link-control"
  shift
fi
base_url="${1:-http://127.0.0.1:17840}"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work="$(mktemp -d /tmp/studiobridge-readonly.XXXXXX)"
cleanup() {
  case "$work" in
    /tmp/studiobridge-readonly.*) rm -rf -- "$work" ;;
    *) printf 'Refusing to remove unexpected validation path: %s\n' "$work" >&2 ;;
  esac
}
trap cleanup EXIT

for command in curl node pactl; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$command" >&2
    exit 1
  fi
done

sinks="$(pactl list short sinks)"
sources="$(pactl list short sources)"

require_endpoint() {
  local listing="$1"
  local device="$2"
  local kind="$3"
  if ! grep -Eiq "BEACN.*(__|[[:space:]])${device}(__|[[:space:]])" <<<"$listing"; then
    printf 'Missing BEACN %s endpoint: %s\n' "$kind" "$device" >&2
    exit 1
  fi
}

require_endpoint "$sinks" Headphones sink
for line in 1 2 3 4; do
  require_endpoint "$sinks" "Line${line}" sink
done
require_endpoint "$sources" Mic source
for line in 5 6 7 8; do
  require_endpoint "$sources" "Line${line}" source
done
printf 'BEACN Link endpoints: PASS (Headphones, Mic, Line1-Line8)\n'

curl -fsS --max-time 8 "$base_url/api/health" >"$work/health.json"
curl -fsS --max-time 8 "$base_url/api/state" >"$work/state.json"

node "$root/scripts/validate-readonly.mjs" "$work/health.json" "$work/state.json" "$expect_link_control"
