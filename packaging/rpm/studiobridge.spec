Name:           studiobridge
Version:        0.1.0
Release:        1%{?dist}
Summary:        Native Linux control surface for BEACN Studio
License:        MIT
URL:            https://github.com/dilllxd/StudioBridge
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo >= 1.92
BuildRequires:  npm
BuildRequires:  gcc
BuildRequires:  fontconfig-devel
BuildRequires:  libusb1-devel
Requires:       fontconfig
Requires:       libusb1

%description
StudioBridge integrates BEACN Studio USB1 with PipeWeaver and provides a
native mixer, routing, application assignment, and guarded microphone DSP UI.

%prep
%autosetup -n StudioBridge-%{version}

%build
npm --prefix web ci
npm --prefix web run build
cargo build --workspace --release --locked

%install
bash scripts/stage-package-root.sh %{buildroot}

%post
udevadm control --reload-rules >/dev/null 2>&1 || :

%postun
udevadm control --reload-rules >/dev/null 2>&1 || :

%files
%license LICENSE
%doc README.md docs/native-desktop.md
/usr/bin/studiobridge-desktop
/usr/lib/studiobridge/studiobridge-daemon
/usr/lib/systemd/user/studiobridge.service
/usr/lib/udev/rules.d/70-studiobridge.rules
/usr/share/applications/studiobridge.desktop
/usr/share/icons/hicolor/scalable/apps/studiobridge.svg
/usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml
/usr/share/studiobridge/web/dist

%changelog
* Sat Jul 11 2026 StudioBridge contributors - 0.1.0-1
- Initial native Linux package
