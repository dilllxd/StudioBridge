#!/usr/bin/env bash
set -euo pipefail

repository="https://github.com/dilllxd/StudioBridge.git"
workspace="$HOME/StudioBridge"
report="$HOME/studiobridge-live-report.txt"

if [[ "$(uname -s)" != "Linux" ]]; then
  printf 'This bootstrap must run inside a Linux live session.\n' >&2
  exit 1
fi

os_id="unknown"
version_id="unknown"
if [[ -r /etc/os-release ]]; then
  # shellcheck disable=SC1091
  source /etc/os-release
  os_id="${ID:-unknown}"
  version_id="${VERSION_ID:-unknown}"
fi

case "$os_id" in
  fedora)
    package_setup() {
      sudo dnf install -y \
        git gcc gcc-c++ make \
        rust cargo nodejs npm \
        pkgconf-pkg-config libusb1-devel \
        pipewire alsa-ucm alsa-utils usbutils curl
    }
    ;;
  ubuntu)
    package_setup() {
      sudo apt-get update
      sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y \
        git build-essential rustc cargo nodejs npm \
        pkg-config libusb-1.0-0-dev \
        pipewire alsa-ucm-conf alsa-utils usbutils curl
    }
    ;;
  *)
    printf 'Supported live systems are Fedora and Ubuntu; detected %s.\n' "$os_id" >&2
    exit 1
    ;;
esac

cat <<MESSAGE
StudioBridge Linux Live bootstrap

Detected: $os_id $version_id

This installs build and diagnostic packages into the disposable live session,
clones the public repository, and gathers read-only USB/audio information.
It does not install Linux, modify partitions, start StudioBridge, install
PipeWeaver, enable hardware writes, or change BEACN settings.
MESSAGE

package_setup

if [[ -e "$workspace" && ! -d "$workspace/.git" ]]; then
  printf 'Refusing to replace existing non-Git path: %s\n' "$workspace" >&2
  exit 1
fi

if [[ -d "$workspace/.git" ]]; then
  git -C "$workspace" pull --ff-only
else
  git clone --depth 1 "$repository" "$workspace"
fi

cd "$workspace"
{
  printf '===== STUDIOBRIDGE LINUX LIVE REPORT =====\n'
  date --iso-8601=seconds
  printf 'Repository commit: '
  git rev-parse HEAD
  printf '\n'

  bash scripts/preflight-linux.sh || true
  bash scripts/collect-linux-audio-info.sh

  printf '\n===== DIRECT USB CHECK =====\n'
  lsusb -d 33ae: || true

  printf '\n===== PIPEWIRE STATUS =====\n'
  wpctl status || true
} 2>&1 | tee "$report"

cat <<MESSAGE

Read-only collection finished.
Report: $report

Open this ChatGPT conversation in the live browser and paste the report with:
  cat "$report"

Do not run the installer or enable hardware writes yet.
MESSAGE
