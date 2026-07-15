#!/usr/bin/env bash
set -u

# Read-only diagnostics for the first StudioBridge hardware validation. This
# intentionally avoids verbose USB dumps, serial numbers, and unrelated logs.

section() {
  printf '\n===== %s =====\n' "$1"
}

redact() {
  sed -E 's/(usb-BEACN_BEACN_Studio_)[[:alnum:]]+(-[0-9]+)/\1REDACTED\2/g'
}

run_if_available() {
  if command -v "$1" >/dev/null 2>&1; then
    "$@" 2>&1 | redact || true
  else
    printf '%s is not installed\n' "$1"
  fi
}

section "Operating system"
uname -a
if [[ -r /etc/os-release ]]; then
  grep -E '^(NAME|VERSION|ID|ID_LIKE)=' /etc/os-release || true
fi

section "BEACN USB devices"
if command -v lsusb >/dev/null 2>&1; then
  lsusb -d 33ae: 2>&1 || printf 'No BEACN USB device detected\n'
else
  printf 'lsusb is not installed\n'
fi

section "ALSA cards"
run_if_available aplay -l
run_if_available arecord -l

section "Audio services"
run_if_available pactl info
run_if_available wpctl status
run_if_available pw-cli info 0

section "BEACN UCM profiles"
for directory in \
  /usr/share/alsa/ucm2/USB-Audio/Beacn \
  /usr/share/alsa-card-profile/mixer/profile-sets; do
  if [[ -d "$directory" ]]; then
    find "$directory" -maxdepth 1 -type f -printf '%f\n' 2>/dev/null | sort
  else
    printf 'Missing: %s\n' "$directory"
  fi
done

section "Versions"
run_if_available pipewire --version
if command -v alsaucm >/dev/null 2>&1; then
  alsaucm --version 2>&1 || true
fi
