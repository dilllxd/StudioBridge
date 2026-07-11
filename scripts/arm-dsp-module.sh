#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
module="${1:-}"
unit_dir="$HOME/.config/systemd/user/studiobridge.service.d"
override="$unit_dir/zz-dsp-write.conf"

case "$module" in
  equalizer|compressor|expander|noise-suppression|enhancement-suite|headphone-equalizer) ;;
  *)
    printf 'Usage: %s {equalizer|compressor|expander|noise-suppression|enhancement-suite|headphone-equalizer}\n' "$0" >&2
    exit 2
    ;;
esac

printf 'Validating read-only and complete DSP baselines before arming %s...\n' "$module"
"$root/scripts/validate-readonly.sh" --link-control
"$root/scripts/validate-microphone-dsp.sh"

install -d "$unit_dir"
temp_override="$(mktemp "$unit_dir/.zz-dsp-write.conf.XXXXXX")"
cleanup() {
  rm -f -- "$temp_override"
}
trap cleanup EXIT

cat >"$temp_override" <<EOF
[Service]
ExecStart=
ExecStart=%h/.local/bin/studiobridge-daemon --studio beacn --mixer pipeweaver --enable-link-host --enable-dsp-write $module
EOF
chmod 0644 "$temp_override"
mv -f -- "$temp_override" "$override"

systemctl --user daemon-reload
systemctl --user restart studiobridge.service
for attempt in {1..20}; do
  if curl -fsS --max-time 2 http://127.0.0.1:17840/api/health >/dev/null 2>&1; then
    break
  fi
  sleep 0.5
done

curl -fsS http://127.0.0.1:17840/api/health \
  | node -e '
let input = "";
process.stdin.on("data", chunk => input += chunk);
process.stdin.on("end", () => {
  const health = JSON.parse(input);
  const expected = process.argv[1].replaceAll("-", "_");
  const safe = health.hardware_writes_enabled === false
    && health.link_control_enabled === true
    && health.dsp_write_modules?.length === 1
    && health.dsp_write_modules[0] === expected;
  if (!safe) {
    console.error(`Unsafe or unexpected runtime state: ${JSON.stringify(health)}`);
    process.exit(1);
  }
});
' "$module"

printf '%s DSP writes armed exclusively. General hardware writes remain disabled.\n' "$module"
