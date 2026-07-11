#!/usr/bin/env bash
set -euo pipefail

if (($# != 1)); then
  printf 'Usage: %s PACKAGE_ROOT\n' "$0" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
package_root="$1"
mkdir -p "$package_root"
package_root="$(cd "$package_root" && pwd -P)"
if [[ "$package_root" == / ]]; then
  printf 'Refusing to stage a package into /.\n' >&2
  exit 1
fi

target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
  target_dir="$root/$target_dir"
fi
daemon="$target_dir/release/studiobridge-daemon"
desktop="$target_dir/release/studiobridge-desktop"

for required in "$daemon" "$desktop" "$root/web/dist/index.html"; do
  if [[ ! -r "$required" ]]; then
    printf 'Missing package input: %s\n' "$required" >&2
    exit 1
  fi
done

install -Dm755 "$desktop" "$package_root/usr/bin/studiobridge-desktop"
install -Dm755 "$daemon" "$package_root/usr/lib/studiobridge/studiobridge-daemon"
install -Dm644 "$root/packaging/systemd/studiobridge-packaged.service" \
  "$package_root/usr/lib/systemd/user/studiobridge.service"
install -Dm644 "$root/packaging/udev/70-studiobridge.rules" \
  "$package_root/usr/lib/udev/rules.d/70-studiobridge.rules"
install -Dm644 "$root/packaging/desktop/studiobridge.desktop" \
  "$package_root/usr/share/applications/studiobridge.desktop"
install -Dm644 "$root/crates/studiobridge-desktop/assets/studiobridge.svg" \
  "$package_root/usr/share/icons/hicolor/scalable/apps/studiobridge.svg"
install -Dm644 "$root/packaging/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml" \
  "$package_root/usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml"
install -Dm644 "$root/LICENSE" "$package_root/usr/share/licenses/studiobridge/LICENSE"
install -Dm644 "$root/README.md" "$package_root/usr/share/doc/studiobridge/README.md"
install -Dm644 "$root/docs/native-desktop.md" \
  "$package_root/usr/share/doc/studiobridge/native-desktop.md"

while IFS= read -r -d '' asset; do
  relative="${asset#"$root/web/dist/"}"
  install -Dm644 "$asset" "$package_root/usr/share/studiobridge/web/dist/$relative"
done < <(find "$root/web/dist" -type f -print0)
