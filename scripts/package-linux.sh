#!/usr/bin/env bash
set -euo pipefail

format=all
skip_build=false
output_directory=""

usage() {
  printf 'Usage: %s [--format all|deb|tar] [--output DIRECTORY] [--skip-build]\n' "$0"
}

while (($#)); do
  case "$1" in
    --format|--output)
      if (($# < 2)); then
        printf '%s requires a value.\n' "$1" >&2
        usage >&2
        exit 2
      fi
      if [[ "$1" == --format ]]; then format="$2"; else output_directory="$2"; fi
      shift 2
      ;;
    --skip-build) skip_build=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$format" in all|deb|tar) ;; *) printf 'Unsupported format: %s\n' "$format" >&2; exit 2 ;; esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="$(awk '
  /^\[workspace\.package\]$/ { in_workspace_package=1; next }
  /^\[/ { in_workspace_package=0 }
  in_workspace_package && /^version = "/ {
    value=$0
    sub(/^version = "/, "", value)
    sub(/".*$/, "", value)
    print value
    exit
  }
' Cargo.toml)"
if [[ -z "$version" ]]; then
  printf 'Unable to read [workspace.package] version from Cargo.toml.\n' >&2
  exit 1
fi
machine="$(uname -m)"
case "$machine" in
  x86_64) deb_arch=amd64 ;;
  aarch64) deb_arch=arm64 ;;
  armv7l) deb_arch=armhf ;;
  *)
    printf 'Unsupported Debian architecture: %s\n' "$machine" >&2
    exit 1
    ;;
esac

if [[ -z "$output_directory" ]]; then output_directory="$root/dist"; fi
mkdir -p "$output_directory"
output_directory="$(cd "$output_directory" && pwd -P)"
if [[ "$output_directory" == / ]]; then
  printf 'Refusing to write package artifacts into /.\n' >&2
  exit 1
fi

if [[ "$skip_build" == false ]]; then
  npm --prefix web ci
  npm --prefix web run build
  cargo build --workspace --release --locked
fi

stage="$(mktemp -d /tmp/studiobridge-package.XXXXXX)"
cleanup() {
  case "$stage" in /tmp/studiobridge-package.*) rm -rf -- "$stage" ;; esac
}
trap cleanup EXIT
bash scripts/stage-package-root.sh "$stage/root"

if [[ "$format" == all || "$format" == tar ]]; then
  source_date_epoch="${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct 2>/dev/null || stat -c %Y Cargo.toml)}"
  if [[ ! "$source_date_epoch" =~ ^[0-9]+$ ]]; then
    printf 'SOURCE_DATE_EPOCH must be an integer Unix timestamp.\n' >&2
    exit 1
  fi
  tar --sort=name --mtime="@$source_date_epoch" --owner=0 --group=0 --numeric-owner \
    -C "$stage/root" -czf "$output_directory/studiobridge-$version-$machine.tar.gz" .
fi

if [[ "$format" == all || "$format" == deb ]]; then
  if ! command -v dpkg-deb >/dev/null 2>&1 || ! command -v dpkg-shlibdeps >/dev/null 2>&1; then
    if [[ "$format" == deb ]]; then
      printf 'dpkg-deb and dpkg-shlibdeps (dpkg-dev) are required for --format deb.\n' >&2
      exit 1
    fi
    printf 'Skipping .deb because dpkg-deb or dpkg-shlibdeps is unavailable.\n' >&2
  else
    mkdir -p "$stage/debian"
    cat > "$stage/debian/control" <<'CONTROL'
Source: studiobridge
Section: sound
Priority: optional

Package: studiobridge
Architecture: any
Description: StudioBridge dependency scan
CONTROL
    dependency_line="$(
      cd "$stage"
      dpkg-shlibdeps -O \
        -e"root/usr/bin/studiobridge-desktop" \
        -e"root/usr/lib/studiobridge/studiobridge-daemon"
    )"
    dependencies="${dependency_line#shlibs:Depends=}"
    rm -rf -- "$stage/debian"
    if [[ -z "$dependencies" || "$dependencies" == "$dependency_line" ]]; then
      printf 'Unable to derive Debian shared-library dependencies.\n' >&2
      exit 1
    fi
    mkdir -p "$stage/root/DEBIAN"
    sed -e "s/@VERSION@/$version/g" -e "s/@ARCH@/$deb_arch/g" \
      -e "s/@DEPENDS@/$dependencies/g" \
      packaging/debian/control.in > "$stage/root/DEBIAN/control"
    install -m755 packaging/debian/postinst "$stage/root/DEBIAN/postinst"
    install -m755 packaging/debian/postrm "$stage/root/DEBIAN/postrm"
    dpkg-deb --root-owner-group --build "$stage/root" \
      "$output_directory/studiobridge_${version}_${deb_arch}.deb"
  fi
fi

printf 'Packages written to %s\n' "$output_directory"
