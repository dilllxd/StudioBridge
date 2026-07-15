#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release --locked

npm --prefix web ci
npm --prefix web test
npm --prefix web run build
npm --prefix web audit --omit=dev
node --test scripts/validate-readonly.test.mjs
node --test scripts/validate-microphone-dsp.test.mjs
node --test scripts/validate-packaging.test.mjs
node scripts/validate-packaging.mjs

bash -n scripts/preflight-linux.sh
bash -n scripts/bootstrap-linux-live.sh
bash -n scripts/collect-linux-audio-info.sh
bash -n scripts/install-linux.sh
bash -n scripts/validate-readonly.sh
bash -n scripts/validate-microphone-dsp.sh
bash -n scripts/enable-link-host.sh
bash -n scripts/arm-dsp-module.sh
bash -n scripts/disarm-dsp-writes.sh
bash -n scripts/validate-headphone-eq-write.sh
bash -n scripts/enable-desktop-autostart.sh
bash -n scripts/disable-desktop-autostart.sh
bash -n scripts/stage-package-root.sh
bash -n scripts/package-linux.sh
bash -n scripts/verify-package-layout.sh
bash -n packaging/debian/postinst
bash -n packaging/debian/postrm
bash -n packaging/arch/studiobridge.install
test -r packaging/README.md
test -r packaging/arch/PKGBUILD
test -r packaging/rpm/studiobridge.spec
test -r packaging/debian/control.in
test -r packaging/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml

if command -v shellcheck >/dev/null 2>&1; then
  shellcheck \
    scripts/install-linux.sh \
    scripts/stage-package-root.sh \
    scripts/package-linux.sh \
    scripts/verify-package-layout.sh \
    packaging/debian/postinst \
    packaging/debian/postrm \
    packaging/arch/studiobridge.install
fi
if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate packaging/desktop/studiobridge.desktop
  desktop-file-validate packaging/desktop/studiobridge-autostart.desktop
fi
if command -v appstreamcli >/dev/null 2>&1; then
  appstreamcli validate --no-net packaging/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml
fi

if grep -Fq -- '--enable-link-host' packaging/systemd/studiobridge.service; then
  printf 'The base service must remain fully read-only.\n' >&2
  exit 1
fi
if grep -Fq -- '--allow-hardware-writes' packaging/systemd/studiobridge.service scripts/enable-link-host.sh; then
  printf 'A packaged service path enables general hardware writes.\n' >&2
  exit 1
fi
if grep -Fq -- '--allow-hardware-writes' packaging/systemd/studiobridge-packaged.service; then
  printf 'The distro-packaged service enables general hardware writes.\n' >&2
  exit 1
fi

if [[ "$(uname -s)" == "Linux" ]]; then
  stage_root="$(mktemp -d /tmp/studiobridge-stage.XXXXXX)"
  package_work="$(mktemp -d /tmp/studiobridge-package-verify.XXXXXX)"
  staged_daemon_pid=""
  cleanup_stage() {
    if [[ -n "$staged_daemon_pid" ]]; then
      kill "$staged_daemon_pid" 2>/dev/null || true
      wait "$staged_daemon_pid" 2>/dev/null || true
    fi
    case "$stage_root" in
      /tmp/studiobridge-stage.*) rm -rf -- "$stage_root" ;;
      *) printf 'Refusing to remove unexpected staging path: %s\n' "$stage_root" >&2 ;;
    esac
    case "$package_work" in
      /tmp/studiobridge-package-verify.*) rm -rf -- "$package_work" ;;
      *) printf 'Refusing to remove unexpected package path: %s\n' "$package_work" >&2 ;;
    esac
  }
  trap cleanup_stage EXIT
  bash scripts/install-linux.sh --stage "$stage_root" --skip-build
  test -x "$stage_root$HOME/.local/bin/studiobridge-daemon"
  test -x "$stage_root$HOME/.local/bin/studiobridge-desktop"
  test -r "$stage_root$HOME/.local/share/studiobridge/web/dist/index.html"
  test -r "$stage_root$HOME/.config/systemd/user/studiobridge.service"
  test -r "$stage_root$HOME/.local/share/applications/studiobridge.desktop"
  test -r "$stage_root$HOME/.local/share/icons/hicolor/scalable/apps/studiobridge.svg"
  test -r "$stage_root/etc/udev/rules.d/70-studiobridge.rules"

  verify_port=$((20000 + ($$ % 20000)))
  (
    cd "$stage_root$HOME/.local/share/studiobridge"
    exec "$stage_root$HOME/.local/bin/studiobridge-daemon" --port "$verify_port"
  ) >"$stage_root/staged-daemon.log" 2>&1 &
  staged_daemon_pid=$!
  curl -fsS --retry 10 --retry-connrefused --retry-delay 0 \
    "http://127.0.0.1:$verify_port/api/health" | grep -Fq '"ok":true'
  curl -fsS "http://127.0.0.1:$verify_port/" | grep -Fq '<title>StudioBridge</title>'
  kill "$staged_daemon_pid"
  wait "$staged_daemon_pid" 2>/dev/null || true
  staged_daemon_pid=""

  bash scripts/package-linux.sh --output "$package_work/artifacts" --skip-build
  shopt -s nullglob
  tar_packages=("$package_work"/artifacts/studiobridge-*.tar.gz)
  deb_packages=("$package_work"/artifacts/studiobridge_*.deb)
  shopt -u nullglob
  if ((${#tar_packages[@]} != 1 || ${#deb_packages[@]} != 1)); then
    printf 'Expected exactly one verified tar package and one Debian package.\n' >&2
    exit 1
  fi

  mkdir -p "$package_work/tar-root" "$package_work/deb-root" "$package_work/deb-control"
  tar -xzf "${tar_packages[0]}" -C "$package_work/tar-root"
  bash scripts/verify-package-layout.sh "$package_work/tar-root"
  if tar -tzf "${tar_packages[0]}" | grep -Eq '(^|/)DEBIAN(/|$)'; then
    printf 'Portable tar package unexpectedly contains Debian control files.\n' >&2
    exit 1
  fi

  dpkg-deb --extract "${deb_packages[0]}" "$package_work/deb-root"
  dpkg-deb --control "${deb_packages[0]}" "$package_work/deb-control"
  bash scripts/verify-package-layout.sh "$package_work/deb-root"
  expected_deb_arch="$(dpkg --print-architecture)"
  expected_package_version="$(awk '
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
  grep -Fxq "Version: $expected_package_version" "$package_work/deb-control/control"
  grep -Fxq "Architecture: $expected_deb_arch" "$package_work/deb-control/control"
  grep -Eq '^Depends: [^@[:space:]]' "$package_work/deb-control/control"
  if grep -R -Eq '@(VERSION|ARCH|DEPENDS)@|--(allow-hardware-writes|enable-link-host|enable-dsp-write)' \
    "$package_work/deb-control"; then
    printf 'Debian control archive contains a placeholder or unsafe argument.\n' >&2
    exit 1
  fi
  if grep -R -Eq '^[[:space:]]*systemctl[^[:cntrl:]]*(enable|start)' "$package_work/deb-control"; then
    printf 'Debian maintainer hooks must not start or enable a user service.\n' >&2
    exit 1
  fi
  diff -qr "$package_work/tar-root" "$package_work/deb-root"

  for binary in \
    "$package_work/deb-root/usr/bin/studiobridge-desktop" \
    "$package_work/deb-root/usr/lib/studiobridge/studiobridge-daemon"; do
    if ldd "$binary" | grep -Fq 'not found'; then
      printf 'Packaged binary has an unresolved shared library: %s\n' "$binary" >&2
      ldd "$binary" >&2
      exit 1
    fi
  done

  if [[ -n "${STUDIOBRIDGE_VERIFIED_ARTIFACT_DIR:-}" ]]; then
    mkdir -p "$STUDIOBRIDGE_VERIFIED_ARTIFACT_DIR"
    cp -- "${tar_packages[0]}" "${deb_packages[0]}" "$STUDIOBRIDGE_VERIFIED_ARTIFACT_DIR/"
  fi
fi

printf 'StudioBridge verification completed successfully.\n'
