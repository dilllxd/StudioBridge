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
    --format) format="$2"; shift 2 ;;
    --output) output_directory="$2"; shift 2 ;;
    --skip-build) skip_build=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'Unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$format" in all|deb|tar) ;; *) printf 'Unsupported format: %s\n' "$format" >&2; exit 2 ;; esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' crates/studiobridge-core/Cargo.toml | head -n1)"
if [[ -z "$version" ]]; then version="0.1.0"; fi
machine="$(uname -m)"
case "$machine" in x86_64) deb_arch=amd64 ;; aarch64) deb_arch=arm64 ;; *) deb_arch="$machine" ;; esac

if [[ -z "$output_directory" ]]; then output_directory="$root/dist"; fi
mkdir -p "$output_directory"
output_directory="$(cd "$output_directory" && pwd -P)"

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
  tar -C "$stage/root" -czf "$output_directory/studiobridge-$version-$machine.tar.gz" .
fi

if [[ "$format" == all || "$format" == deb ]]; then
  if ! command -v dpkg-deb >/dev/null 2>&1; then
    if [[ "$format" == deb ]]; then
      printf 'dpkg-deb is required for --format deb.\n' >&2
      exit 1
    fi
    printf 'Skipping .deb because dpkg-deb is unavailable.\n' >&2
  else
    mkdir -p "$stage/root/DEBIAN"
    sed -e "s/@VERSION@/$version/g" -e "s/@ARCH@/$deb_arch/g" \
      packaging/debian/control.in > "$stage/root/DEBIAN/control"
    install -m755 packaging/debian/postinst "$stage/root/DEBIAN/postinst"
    install -m755 packaging/debian/postrm "$stage/root/DEBIAN/postrm"
    dpkg-deb --root-owner-group --build "$stage/root" \
      "$output_directory/studiobridge_${version}_${deb_arch}.deb"
  fi
fi

printf 'Packages written to %s\n' "$output_directory"
