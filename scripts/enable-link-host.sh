#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
unit_dir="$HOME/.config/systemd/user/studiobridge.service.d"
override="$unit_dir/link-host.conf"

if curl -fsS --max-time 2 http://127.0.0.1:17840/api/health 2>/dev/null \
  | node -e 'let input=""; process.stdin.on("data", chunk => input += chunk); process.stdin.on("end", () => process.exit(JSON.parse(input).link_control_enabled === true ? 0 : 1));'
then
  "$root/scripts/validate-readonly.sh" --link-control
  printf 'Link host control is already enabled. General BEACN hardware writes remain disabled.\n'
  exit 0
fi

printf 'Validating the fully read-only baseline before enabling Link host control...\n'
"$root/scripts/validate-readonly.sh"

install -d "$unit_dir"
temp_override="$(mktemp "$unit_dir/.link-host.conf.XXXXXX")"
cleanup() {
  rm -f -- "$temp_override"
}
trap cleanup EXIT

cat >"$temp_override" <<'EOF'
[Service]
ExecStart=
ExecStart=%h/.local/bin/studiobridge-daemon --studio beacn --mixer pipeweaver --enable-link-host
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

"$root/scripts/validate-readonly.sh" --link-control
printf 'Link host control enabled. General BEACN hardware writes remain disabled.\n'
