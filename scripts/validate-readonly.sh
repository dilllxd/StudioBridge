#!/usr/bin/env bash
set -euo pipefail

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

for command in curl node; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$command" >&2
    exit 1
  fi
done

curl -fsS --max-time 8 "$base_url/api/health" >"$work/health.json"
curl -fsS --max-time 8 "$base_url/api/state" >"$work/state.json"

node "$root/scripts/validate-readonly.mjs" "$work/health.json" "$work/state.json"
