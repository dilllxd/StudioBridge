#!/usr/bin/env bash
set -euo pipefail

if (($# != 1)); then
  printf 'Usage: %s PACKAGE_ROOT\n' "$0" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
package_root="$1"
if [[ ! -d "$package_root" ]]; then
  printf 'Package root does not exist: %s\n' "$package_root" >&2
  exit 1
fi
package_root="$(cd "$package_root" && pwd -P)"
if [[ "$package_root" == / ]]; then
  printf 'Refusing to inspect / as a package root.\n' >&2
  exit 1
fi

expected_fixed_files=(
  /usr/bin/studiobridge-desktop
  /usr/lib/studiobridge/studiobridge-daemon
  /usr/lib/systemd/user/studiobridge.service
  /usr/lib/udev/rules.d/70-studiobridge.rules
  /usr/share/applications/studiobridge.desktop
  /usr/share/doc/studiobridge/README.md
  /usr/share/doc/studiobridge/native-desktop.md
  /usr/share/icons/hicolor/scalable/apps/studiobridge.svg
  /usr/share/licenses/studiobridge/LICENSE
  /usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml
)

actual_fixed_files="$(
  find "$package_root" -type f \
    ! -path "$package_root/usr/share/studiobridge/web/dist/*" \
    -printf '/%P\n' | sort
)"
expected_fixed_text="$(printf '%s\n' "${expected_fixed_files[@]}" | sort)"
if [[ "$actual_fixed_files" != "$expected_fixed_text" ]]; then
  printf 'Staged fixed-file manifest differs from the allowlist.\n' >&2
  diff -u <(printf '%s\n' "$expected_fixed_text") <(printf '%s\n' "$actual_fixed_files") >&2 || true
  exit 1
fi

web_root="$package_root/usr/share/studiobridge/web/dist"
if [[ ! -r "$web_root/index.html" ]]; then
  printf 'Packaged web fallback is missing index.html.\n' >&2
  exit 1
fi
if find "$package_root" -type l -print -quit | grep -q .; then
  printf 'Package root contains an unexpected symbolic link.\n' >&2
  exit 1
fi
if find "$package_root" -type f -perm /022 -print -quit | grep -q .; then
  printf 'Package root contains a group- or world-writable file.\n' >&2
  exit 1
fi

for executable in \
  "$package_root/usr/bin/studiobridge-desktop" \
  "$package_root/usr/lib/studiobridge/studiobridge-daemon"; do
  if [[ "$(stat -c %a "$executable")" != 755 ]]; then
    printf 'Executable has the wrong mode: %s\n' "$executable" >&2
    exit 1
  fi
done
while IFS= read -r data_file; do
  if [[ "$(stat -c %a "$data_file")" != 644 ]]; then
    printf 'Data file has the wrong mode: %s\n' "$data_file" >&2
    exit 1
  fi
done < <(find "$package_root" -type f \
  ! -path "$package_root/usr/bin/studiobridge-desktop" \
  ! -path "$package_root/usr/lib/studiobridge/studiobridge-daemon")

service="$package_root/usr/lib/systemd/user/studiobridge.service"
if ! cmp -s "$service" "$repository_root/packaging/systemd/studiobridge-packaged.service"; then
  printf 'Staged service differs from the reviewed packaged unit.\n' >&2
  exit 1
fi
if [[ "$(grep -c '^ExecStart=' "$service")" != 1 ]] || \
  ! grep -Fxq 'ExecStart=/usr/lib/studiobridge/studiobridge-daemon --studio beacn --mixer pipeweaver' "$service"; then
  printf 'Packaged service ExecStart is not the exact read-only command.\n' >&2
  exit 1
fi
if grep -Eq -- '--(allow-hardware-writes|enable-link-host|enable-dsp-write)' "$service"; then
  printf 'Packaged service contains a forbidden write-enabling argument.\n' >&2
  exit 1
fi

if ! cmp -s "$package_root/usr/lib/udev/rules.d/70-studiobridge.rules" \
  "$repository_root/packaging/udev/70-studiobridge.rules"; then
  printf 'Staged udev policy differs from the reviewed rule.\n' >&2
  exit 1
fi
if ! cmp -s "$package_root/usr/share/applications/studiobridge.desktop" \
  "$repository_root/packaging/desktop/studiobridge.desktop"; then
  printf 'Staged desktop launcher differs from the reviewed metadata.\n' >&2
  exit 1
fi

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$package_root/usr/share/applications/studiobridge.desktop"
fi
if command -v appstreamcli >/dev/null 2>&1; then
  appstreamcli validate --no-net \
    "$package_root/usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml"
fi
if command -v systemd-analyze >/dev/null 2>&1; then
  if ! systemd-analyze security --offline=yes "$service" >/dev/null; then
    printf 'systemd-analyze could not parse the packaged user service.\n' >&2
    exit 1
  fi
fi

printf 'Verified staged package layout at %s\n' "$package_root"
