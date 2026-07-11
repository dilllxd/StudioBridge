# Linux packaging

All package formats use `scripts/stage-package-root.sh` so installed paths and
safety defaults cannot drift between distributions.

## Debian and portable tar

```bash
bash scripts/package-linux.sh
```

Artifacts are written to `dist/`. The `.deb` is created when `dpkg-deb` is
available; the `.tar.gz` is always created. Use `--format deb` or `--format tar`
to request one format. `--skip-build` reuses existing release and web builds.

## Fedora / RPM

Create a `StudioBridge-0.1.0.tar.gz` source archive, put it in `SOURCES`, copy
`packaging/rpm/studiobridge.spec` to `SPECS`, then run:

```bash
rpmbuild -ba SPECS/studiobridge.spec
```

## Arch Linux

Copy `packaging/arch/PKGBUILD` into an empty package directory after publishing
the matching `v0.1.0` tag, then run:

```bash
makepkg --syncdeps --cleanbuild
```

## After installation

The packages deliberately do not start a user service from a root package
hook. Each desktop user opts in with:

```bash
systemctl --user enable --now studiobridge.service
```

The packaged unit starts the daemon with the real BEACN and PipeWeaver
backends, but without `--allow-hardware-writes`, `--enable-link-host`, or any
DSP write module. Link hosting and attended DSP leases remain separate,
explicit actions. Firmware, factory reset, and unrestricted storage writes are
not packaged.
