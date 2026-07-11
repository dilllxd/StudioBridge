#!/usr/bin/env bash
set -u

errors=0
warnings=0

ok() { printf '[ ok ] %s\n' "$1"; }
warn() { printf '[warn] %s\n' "$1"; warnings=$((warnings + 1)); }
fail() { printf '[fail] %s\n' "$1" >&2; errors=$((errors + 1)); }

print_install_guidance() {
  case " $os_id $os_like " in
    *" arch "*|*" cachyos "*|*" manjaro "*)
      printf '       System packages: sudo pacman -S --needed base-devel rust nodejs npm pkgconf libusb pipewire alsa-ucm-conf usbutils fontconfig\n'
      ;;
    *" fedora "*|*" rhel "*|*" centos "*)
      printf '       System packages: sudo dnf install gcc gcc-c++ make rust cargo nodejs npm pkgconf-pkg-config libusb1-devel pipewire alsa-ucm usbutils fontconfig-devel\n'
      ;;
    *" ubuntu "*|*" debian "*)
      printf '       System packages: sudo apt install build-essential curl pkg-config libusb-1.0-0-dev pipewire alsa-ucm-conf usbutils libfontconfig1-dev\n'
      printf '       Rust >=1.92: install rustup from https://rustup.rs when the distro toolchain is older.\n'
      printf '       Node >=20.19: install a current Node.js LTS release; do not rely on an older distro package.\n'
      ;;
    *)
      printf '       Install Rust >=1.92, Node.js >=20.19/npm, pkg-config, libusb and fontconfig development files, PipeWire, alsa-ucm-conf, and usbutils.\n'
      ;;
  esac
}

if [[ "$(uname -s)" != "Linux" ]]; then
  fail "StudioBridge real mode requires Linux"
fi

os_id="unknown"
os_like=""
if [[ -r /etc/os-release ]]; then
  # The distribution file contains shell assignments by specification.
  # shellcheck disable=SC1091
  source /etc/os-release
  os_id="${ID:-unknown}"
  os_like="${ID_LIKE:-}"
  ok "Distribution: ${PRETTY_NAME:-$os_id}"
else
  warn "Unable to identify the Linux distribution"
fi

missing=()
for command in cargo rustc node npm pkg-config systemctl install sudo udevadm; do
  command -v "$command" >/dev/null 2>&1 || missing+=("$command")
done

if ((${#missing[@]})); then
  fail "Missing build/runtime commands: ${missing[*]}"
  print_install_guidance
else
  ok "Build and service-management commands are available"
fi

version_at_least() {
  local actual="$1"
  local minimum="$2"
  [[ "$(printf '%s\n%s\n' "$minimum" "$actual" | sort -V | head -n 1)" == "$minimum" ]]
}

if command -v rustc >/dev/null 2>&1; then
  rust_version="$(rustc --version | awk '{print $2}')"
  if version_at_least "$rust_version" "1.92.0"; then
    ok "Rust $rust_version supports the native Slint desktop client"
  else
    fail "Rust $rust_version is too old; the native client requires Rust 1.92 or later"
    print_install_guidance
  fi
fi

if command -v node >/dev/null 2>&1; then
  node_version="$(node --version | sed 's/^v//')"
  if version_at_least "$node_version" "20.19.0"; then
    ok "Node.js $node_version supports the frontend build"
  else
    fail "Node.js $node_version is too old; the frontend build requires Node.js 20.19 or later"
    print_install_guidance
  fi
fi

if command -v pipewire >/dev/null 2>&1; then
  pipewire_version="$(pipewire --version 2>/dev/null | tail -n 1 | awk '{print $NF}')"
  ok "PipeWire command available${pipewire_version:+ ($pipewire_version)}"
  if [[ -n "$pipewire_version" ]] && ! version_at_least "$pipewire_version" "1.4.0"; then
    warn "PipeWire $pipewire_version is older than PipeWeaver's recommended 1.4.0"
  fi
else
  fail "PipeWire is not installed"
fi

if systemctl --user is-active --quiet pipewire.service 2>/dev/null; then
  ok "PipeWire user service is active"
else
  warn "PipeWire user service is not currently active"
fi

ucm_profile="/usr/share/alsa/ucm2/USB-Audio/Beacn/Beacn-Studio.conf"
if [[ -r "$ucm_profile" ]]; then
  ok "BEACN Studio ALSA UCM profile is installed"
else
  fail "BEACN Studio UCM profile is missing: $ucm_profile"
  printf '       Install alsa-ucm-conf 1.2.15 or later, or install the profiles from\n'
  printf '       https://github.com/beacn-on-linux/beacn-ucm-profiles as documented in docs/hardware-validation.md.\n'
fi

if command -v dpkg-query >/dev/null 2>&1; then
  ucm_version="$(dpkg-query -W -f='${Version}' alsa-ucm-conf 2>/dev/null || true)"
elif command -v pacman >/dev/null 2>&1; then
  ucm_version="$(pacman -Q alsa-ucm-conf 2>/dev/null | awk '{print $2}' || true)"
else
  ucm_version=""
fi
if [[ -n "$ucm_version" ]]; then
  ok "alsa-ucm-conf package version: $ucm_version"
fi

split_config="/usr/share/alsa/ucm2/common/pcm/split.conf"
if [[ -r "$split_config" ]]; then
  if grep -Fq "Empty \"\${var:-__Device}\"" "$split_config"; then
    ok "ALSA UCM split-device compatibility fix is present"
  elif grep -Fq "Empty \"\${var:__Device}\"" "$split_config"; then
    fail "ALSA UCM split-device compatibility fix is missing in $split_config"
    printf "       Change Empty \"\${var:__Device}\" to Empty \"\${var:-__Device}\" as described in docs/hardware-validation.md.\n"
  else
    warn "Unable to identify the split-device compatibility state in $split_config"
  fi
fi

if command -v lsusb >/dev/null 2>&1; then
  if lsusb -d 33ae:0003 2>/dev/null | grep -q .; then
    ok "BEACN Studio USB1 detected (33ae:0003)"
  else
    warn "BEACN Studio USB1 is not currently detected"
  fi
else
  warn "lsusb is unavailable; USB1 detection was skipped"
fi

if command -v curl >/dev/null 2>&1 && \
  curl -fsS --max-time 1 http://127.0.0.1:14565/api/get-devices >/dev/null 2>&1; then
  ok "PipeWeaver HTTP API is reachable"
else
  warn "PipeWeaver HTTP API is not reachable at http://127.0.0.1:14565"
fi

printf '\nPreflight result: %d error(s), %d warning(s)\n' "$errors" "$warnings"
if ((errors)); then
  exit 1
fi
