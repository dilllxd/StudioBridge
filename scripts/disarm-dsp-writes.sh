#!/usr/bin/env bash
set -euo pipefail

override="$HOME/.config/systemd/user/studiobridge.service.d/zz-dsp-write.conf"
rm -f -- "$override"
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
  const safe = health.hardware_writes_enabled === false
    && health.link_control_enabled === true
    && health.dsp_write_modules?.length === 0;
  if (!safe) {
    console.error(`Unsafe or unexpected runtime state: ${JSON.stringify(health)}`);
    process.exit(1);
  }
});
'

printf 'All microphone DSP writes disarmed. Link control remains isolated and active.\n'
