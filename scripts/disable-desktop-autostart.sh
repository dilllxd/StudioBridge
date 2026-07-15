#!/usr/bin/env bash
set -euo pipefail

destination="$HOME/.config/autostart/studiobridge.desktop"
rm -f -- "$destination"
printf 'StudioBridge desktop autostart disabled. The daemon service is unchanged.\n'
