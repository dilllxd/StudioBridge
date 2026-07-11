#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
destination="$HOME/.config/autostart/studiobridge.desktop"
install -Dm644 "$root/packaging/desktop/studiobridge-autostart.desktop" "$destination"
printf 'StudioBridge will start in the tray at the next graphical login.\n'
