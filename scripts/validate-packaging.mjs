import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REQUIRED_FILES = [
  "Cargo.toml",
  "web/package.json",
  "scripts/package-linux.sh",
  "scripts/stage-package-root.sh",
  "packaging/arch/PKGBUILD",
  "packaging/arch/studiobridge.install",
  "packaging/debian/control.in",
  "packaging/debian/postinst",
  "packaging/debian/postrm",
  "packaging/desktop/studiobridge-autostart.desktop",
  "packaging/desktop/studiobridge.desktop",
  "packaging/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml",
  "packaging/rpm/studiobridge.spec",
  "packaging/systemd/studiobridge-packaged.service",
  "packaging/systemd/studiobridge.service",
  "packaging/udev/70-studiobridge.rules",
];

const STAGED_PATHS = [
  "/usr/bin/studiobridge-desktop",
  "/usr/lib/studiobridge/studiobridge-daemon",
  "/usr/lib/systemd/user/studiobridge.service",
  "/usr/lib/udev/rules.d/70-studiobridge.rules",
  "/usr/share/applications/studiobridge.desktop",
  "/usr/share/icons/hicolor/scalable/apps/studiobridge.svg",
  "/usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml",
  "/usr/share/licenses/studiobridge/LICENSE",
  "/usr/share/doc/studiobridge/README.md",
  "/usr/share/doc/studiobridge/native-desktop.md",
  "/usr/share/studiobridge/web/dist/",
];

const FORBIDDEN_SERVICE_ARGUMENTS = [
  "--allow-hardware-writes",
  "--enable-link-host",
  "--enable-dsp-write",
];

function valueFromSection(toml, section, key) {
  const escaped = section.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const block = toml.match(new RegExp(`^\\[${escaped}\\]\\s*$([\\s\\S]*?)(?=^\\[|\\Z)`, "m"))?.[1];
  return block?.match(new RegExp(`^${key}\\s*=\\s*"([^"]+)"`, "m"))?.[1];
}

function field(text, name) {
  return text.match(new RegExp(`^${name}\\s*[:=]\\s*([^\\s#]+)`, "m"))?.[1];
}

export function validatePackaging(files) {
  const failures = [];
  const check = (condition, message) => {
    if (!condition) failures.push(message);
  };
  const read = (name) => {
    const value = files[name];
    check(typeof value === "string", `missing packaging source: ${name}`);
    return value ?? "";
  };

  for (const name of REQUIRED_FILES) read(name);

  const workspaceVersion = valueFromSection(read("Cargo.toml"), "workspace.package", "version");
  check(Boolean(workspaceVersion), "workspace package version is missing");
  let webVersion;
  try {
    webVersion = JSON.parse(read("web/package.json")).version;
  } catch {
    failures.push("web/package.json is not valid JSON");
  }
  check(webVersion === workspaceVersion, `web version ${webVersion ?? "(missing)"} does not match workspace ${workspaceVersion ?? "(missing)"}`);

  const archVersion = field(read("packaging/arch/PKGBUILD"), "pkgver");
  const rpmVersion = field(read("packaging/rpm/studiobridge.spec"), "Version");
  check(archVersion === workspaceVersion, `Arch version ${archVersion ?? "(missing)"} does not match workspace ${workspaceVersion ?? "(missing)"}`);
  check(rpmVersion === workspaceVersion, `RPM version ${rpmVersion ?? "(missing)"} does not match workspace ${workspaceVersion ?? "(missing)"}`);
  check(read("packaging/debian/control.in").includes("Version: @VERSION@"), "Debian control must receive the workspace version placeholder");
  check(read("packaging/debian/control.in").includes("Architecture: @ARCH@"), "Debian control must receive the detected architecture placeholder");

  const packageScript = read("scripts/package-linux.sh");
  check(packageScript.includes("workspace\\.package"), "package-linux.sh must read the workspace package version");
  check(!/version=[^\n]*0\.1\.0/.test(packageScript), "package-linux.sh must not silently fall back to a hard-coded version");

  const stageScript = read("scripts/stage-package-root.sh");
  for (const installedPath of STAGED_PATHS) {
    check(stageScript.includes(installedPath), `package staging omits ${installedPath}`);
  }
  check(stageScript.includes("Refusing to stage into a nonempty package root"), "package staging must reject stale files in a nonempty root");

  for (const serviceName of [
    "packaging/systemd/studiobridge.service",
    "packaging/systemd/studiobridge-packaged.service",
  ]) {
    const service = read(serviceName);
    const execLines = service.match(/^ExecStart=.*$/gm) ?? [];
    check(execLines.length === 1, `${serviceName} must have exactly one ExecStart`);
    check(execLines[0]?.endsWith(" --studio beacn --mixer pipeweaver"), `${serviceName} must start only the real read-only backends`);
    check(service.includes("NoNewPrivileges=true"), `${serviceName} must retain NoNewPrivileges`);
    check(service.includes("UMask=0077"), `${serviceName} must retain its private umask`);
    for (const argument of FORBIDDEN_SERVICE_ARGUMENTS) {
      check(!service.includes(argument), `${serviceName} contains forbidden argument ${argument}`);
    }
  }

  const udev = read("packaging/udev/70-studiobridge.rules");
  const activeRules = udev.split(/\r?\n/).map((line) => line.trim()).filter((line) => line && !line.startsWith("#"));
  check(activeRules.length === 1, "udev policy must contain exactly one active rule");
  check(activeRules[0]?.includes('SUBSYSTEM=="usb"'), "udev rule must be restricted to USB devices");
  check(activeRules[0]?.includes('ATTR{idVendor}=="33ae"'), "udev rule must be restricted to BEACN vendor 33ae");
  check(activeRules[0]?.includes('ATTR{idProduct}=="0003"'), "udev rule must be restricted to Studio USB1 product 0003");
  check(activeRules[0]?.includes('TAG+="uaccess"'), "udev rule must grant the active desktop user through uaccess");
  check(!/\b(?:RUN|PROGRAM|OWNER|GROUP|MODE)\s*[+:]?=/.test(udev), "udev rule must not execute programs or grant broad persistent permissions");

  const desktop = read("packaging/desktop/studiobridge.desktop");
  const autostart = read("packaging/desktop/studiobridge-autostart.desktop");
  check(/^Exec=studiobridge-desktop$/m.test(desktop), "desktop launcher must run only studiobridge-desktop");
  check(/^TryExec=studiobridge-desktop$/m.test(desktop), "desktop launcher must declare TryExec");
  check(/^Exec=studiobridge-desktop --background$/m.test(autostart), "autostart launcher must use only the desktop background mode");
  for (const argument of FORBIDDEN_SERVICE_ARGUMENTS) {
    check(!desktop.includes(argument) && !autostart.includes(argument), `desktop metadata contains forbidden daemon argument ${argument}`);
  }

  for (const hookName of ["packaging/debian/postinst", "packaging/debian/postrm"]) {
    const hook = read(hookName);
    check(!/^\s*systemctl[^\n]*(?:enable|start)/m.test(hook), `${hookName} must not start a per-user service from a root package hook`);
    for (const argument of FORBIDDEN_SERVICE_ARGUMENTS) {
      check(!hook.includes(argument), `${hookName} contains forbidden argument ${argument}`);
    }
  }

  const archHook = read("packaging/arch/studiobridge.install");
  check(read("packaging/arch/PKGBUILD").includes("install=studiobridge.install"), "Arch package must register its udev lifecycle hook");
  check(!/^\s*systemctl[^\n]*(?:enable|start)/m.test(archHook), "Arch package hook must not start a per-user service from a root package hook");
  for (const argument of FORBIDDEN_SERVICE_ARGUMENTS) {
    check(!archHook.includes(argument), `Arch package hook contains forbidden argument ${argument}`);
  }

  for (const definition of ["packaging/arch/PKGBUILD", "packaging/rpm/studiobridge.spec"]) {
    check(read(definition).includes("scripts/stage-package-root.sh"), `${definition} must use the shared package staging script`);
  }

  return { failures, version: workspaceVersion, stagedPaths: STAGED_PATHS };
}

export function readPackagingSources(root) {
  return Object.fromEntries(REQUIRED_FILES.map((name) => [name, fs.readFileSync(path.join(root, name), "utf8")]));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const result = validatePackaging(readPackagingSources(root));
  for (const failure of result.failures) console.error(`FAIL: ${failure}`);
  if (result.failures.length) {
    process.exitCode = 1;
  } else {
    console.log(`Packaging definitions are consistent for StudioBridge ${result.version}.`);
  }
}
