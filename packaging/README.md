# Linux packaging

All package formats use `scripts/stage-package-root.sh` so installed paths and
safety defaults cannot drift between distributions.

The canonical version is `[workspace.package].version` in the root
`Cargo.toml`. `scripts/package-linux.sh` reads that value directly, Debian
dependencies are generated from the built ELF binaries with `dpkg-shlibdeps`,
and the offline packaging validator rejects version drift in the Arch, RPM, or
web metadata.

## Debian and portable tar

```bash
bash scripts/package-linux.sh
```

Artifacts are written to `dist/`. The `.deb` is created when `dpkg-deb` and
`dpkg-shlibdeps` (from `dpkg-dev`) are available; the `.tar.gz` is always
created. Use `--format deb` or `--format tar` to request one format.
`--skip-build` reuses existing release and web builds. The tar archive has
deterministic ownership, ordering, and timestamps when `SOURCE_DATE_EPOCH` is
set; otherwise it uses the current Git commit timestamp.

## Fedora / RPM

Copy `packaging/rpm/studiobridge.spec` to `SPECS`; its `Source0` downloads the
matching Git tag. Then run:

```bash
rpmbuild -ba SPECS/studiobridge.spec
```

## Arch Linux

Copy `packaging/arch/PKGBUILD` and `packaging/arch/studiobridge.install` into an
empty package directory after publishing the matching `v0.1.0` tag, then run:

```bash
makepkg --syncdeps --cleanbuild
```

## After installation

The packages deliberately do not start a user service from a root package
hook. Each desktop user opts in with:

```bash
systemctl --user enable --now studiobridge.service
```

The Debian, RPM, and Arch lifecycle hooks only reload udev rule metadata. They
do not start a user service. The native app's **Start at Login** setting writes
or removes its own per-user XDG autostart entry; the package does not turn that
setting on.

The packaged unit starts the daemon with the real BEACN and PipeWeaver
backends, but without `--allow-hardware-writes`, `--enable-link-host`, or any
DSP write module. Link hosting and attended DSP leases remain separate,
explicit actions. Firmware, factory reset, and unrestricted storage writes are
not packaged.

## Portable archive lifecycle

The tar archive is a transparent `/usr` package root. Inspect it before use:

```bash
tar -tzf studiobridge-VERSION-ARCH.tar.gz
```

Install it as root only after inspection, reload udev metadata, and then opt in
to the user service as the desktop user:

```bash
sudo tar -xzf studiobridge-VERSION-ARCH.tar.gz -C /
sudo udevadm control --reload-rules
systemctl --user daemon-reload
systemctl --user enable --now studiobridge.service
```

The portable archive is intentionally not registered with a package database.
To remove it, first disable the user service, then remove only its fixed
manifest. User profiles and settings under `~/.config/studiobridge` are
preserved:

```bash
systemctl --user disable --now studiobridge.service
rm -f ~/.config/autostart/studiobridge.desktop
sudo rm -f \
  /usr/bin/studiobridge-desktop \
  /usr/lib/studiobridge/studiobridge-daemon \
  /usr/lib/systemd/user/studiobridge.service \
  /usr/lib/udev/rules.d/70-studiobridge.rules \
  /usr/share/applications/studiobridge.desktop \
  /usr/share/icons/hicolor/scalable/apps/studiobridge.svg \
  /usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml
sudo rm -rf \
  /usr/share/studiobridge \
  /usr/share/doc/studiobridge \
  /usr/share/licenses/studiobridge
sudo udevadm control --reload-rules
systemctl --user daemon-reload
```

## Verification

`scripts/verify.sh` builds and exercises a staged user install, creates real
tar and Debian artifacts, extracts both, checks their exact fixed-file
allowlist and permissions, validates desktop/AppStream metadata, confirms all
ELF dependencies resolve, and rejects unsafe service flags. CI uploads packages
only after those checks pass. RPM and Arch builds still require their native
distribution tools before release.
