#!/usr/bin/env bash
set -euo pipefail

stage_root=""
skip_build=false

usage() {
  printf 'Usage: %s [--stage DIRECTORY [--skip-build]]\n' "$0"
  printf '  --stage installs into a DESTDIR-style tree without sudo or service changes.\n'
  printf '  --skip-build is accepted only with --stage and uses existing build artifacts.\n'
}

while (($#)); do
  case "$1" in
    --stage)
      if (($# < 2)); then
        printf '%s requires a directory.\n' "$1" >&2
        usage >&2
        exit 2
      fi
      stage_root="$2"
      shift 2
      ;;
    --skip-build)
      skip_build=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'Unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ -z "$stage_root" && "$skip_build" == true ]]; then
  printf '%s\n' '--skip-build may only be used with --stage.' >&2
  exit 2
fi

if [[ -z "$stage_root" && "${EUID}" -eq 0 ]]; then
  printf 'Run this installer as your normal desktop user, not as root.\n' >&2
  exit 1
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ "$(uname -s)" != "Linux" ]]; then
  printf 'StudioBridge installation requires Linux.\n' >&2
  exit 1
fi

if [[ -z "$stage_root" ]]; then
  printf 'Running Linux preflight...\n'
  if ! bash scripts/preflight-linux.sh; then
    printf '\nResolve the preflight errors before installing StudioBridge.\n' >&2
    exit 1
  fi
else
  printf 'Preparing a non-mutating staged installation...\n'
fi

required_commands=(find install)
if [[ "$skip_build" == false ]]; then
  required_commands+=(cargo npm)
fi
if [[ -z "$stage_root" ]]; then
  required_commands+=(systemctl sudo udevadm)
fi
for command in "${required_commands[@]}"; do
  if ! command -v "$command" >/dev/null 2>&1; then
    printf 'Missing required command: %s\n' "$command" >&2
    exit 1
  fi
done

if [[ "$skip_build" == false ]]; then
  printf 'Building StudioBridge web interface...\n'
  npm --prefix web ci
  npm --prefix web run build

  printf 'Building StudioBridge daemon...\n'
  cargo build --workspace --release --locked
fi

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
  target_dir="$root/$target_dir"
fi
daemon_binary="$target_dir/release/studiobridge-daemon"
if [[ ! -x "$daemon_binary" ]]; then
  printf 'Missing Linux daemon build: %s\n' "$daemon_binary" >&2
  exit 1
fi
if [[ ! -r web/dist/index.html ]]; then
  printf 'Missing frontend build: %s/web/dist/index.html\n' "$root" >&2
  exit 1
fi

if [[ -n "$stage_root" ]]; then
  mkdir -p "$stage_root"
  stage_root="$(cd "$stage_root" && pwd -P)"
  if [[ "$stage_root" == / ]]; then
    printf 'Refusing to use / as a staging directory.\n' >&2
    exit 1
  fi
fi

destination() {
  printf '%s%s\n' "$stage_root" "$1"
}

printf 'Installing user files...\n'
install -Dm755 "$daemon_binary" "$(destination "$HOME/.local/bin/studiobridge-daemon")"
web_destination="$(destination "$HOME/.local/share/studiobridge/web/dist")"
install -d "$web_destination"
while IFS= read -r -d '' asset; do
  relative_asset="${asset#web/dist/}"
  install -Dm644 "$asset" "$web_destination/$relative_asset"
done < <(find web/dist -type f -print0)
install -Dm644 packaging/systemd/studiobridge.service \
  "$(destination "$HOME/.config/systemd/user/studiobridge.service")"
install -Dm644 packaging/desktop/studiobridge.desktop \
  "$(destination "$HOME/.local/share/applications/studiobridge.desktop")"

if [[ -n "$stage_root" ]]; then
  install -Dm644 packaging/udev/70-studiobridge.rules \
    "$(destination /etc/udev/rules.d/70-studiobridge.rules)"
  printf '\nStudioBridge staged without changing services or the live udev configuration.\n'
  printf 'Staging root: %s\n' "$stage_root"
  exit 0
fi

printf 'Installing USB permission rules (sudo may prompt)...\n'
sudo install -Dm644 packaging/udev/70-studiobridge.rules \
  /etc/udev/rules.d/70-studiobridge.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=usb --attr-match=idVendor=33ae

systemctl --user daemon-reload
systemctl --user enable --now studiobridge.service
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$HOME/.local/share/applications" >/dev/null 2>&1 || true
fi

printf '\nStudioBridge installed in read-only hardware mode.\n'
printf 'Open http://127.0.0.1:17840 after BEACN Studio USB1 and PipeWeaver are running.\n'
printf 'Run scripts/validate-readonly.sh before scripts/enable-link-host.sh.\n'
printf 'Do not enable general hardware writes during this validation.\n'
