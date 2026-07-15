import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { readPackagingSources, validatePackaging } from "./validate-packaging.mjs";

const root = path.resolve(import.meta.dirname, "..");

function validSources() {
  return readPackagingSources(root);
}

test("accepts the repository packaging definitions", () => {
  const result = validatePackaging(validSources());
  assert.deepEqual(result.failures, []);
  assert.match(result.version, /^\d+\.\d+\.\d+/);
});

test("rejects every unsafe baseline service flag", () => {
  for (const argument of ["--allow-hardware-writes", "--enable-link-host", "--enable-dsp-write microphone-equalizer"]) {
    const sources = validSources();
    sources["packaging/systemd/studiobridge-packaged.service"] = sources["packaging/systemd/studiobridge-packaged.service"].replace(
      " --mixer pipeweaver",
      ` --mixer pipeweaver ${argument}`,
    );
    const result = validatePackaging(sources);
    assert(result.failures.some((failure) => failure.includes("forbidden argument") || failure.includes("read-only backends")), argument);
  }
});

test("rejects version drift in distribution metadata", () => {
  const sources = validSources();
  sources["packaging/arch/PKGBUILD"] = sources["packaging/arch/PKGBUILD"].replace(/^pkgver=.*$/m, "pkgver=9.9.9");
  sources["packaging/rpm/studiobridge.spec"] = sources["packaging/rpm/studiobridge.spec"].replace(/^Version:.*$/m, "Version: 8.8.8");
  const result = validatePackaging(sources);
  assert(result.failures.some((failure) => failure.includes("Arch version")));
  assert(result.failures.some((failure) => failure.includes("RPM version")));
});

test("rejects an incomplete staged package manifest", () => {
  const sources = validSources();
  sources["scripts/stage-package-root.sh"] = sources["scripts/stage-package-root.sh"].replace("/usr/share/metainfo/io.github.dilllxd.StudioBridge.metainfo.xml", "/usr/share/metainfo/missing.xml");
  const result = validatePackaging(sources);
  assert(result.failures.some((failure) => failure.includes("package staging omits /usr/share/metainfo")));
});

test("rejects a broadened or executable udev policy", () => {
  const sources = validSources();
  sources["packaging/udev/70-studiobridge.rules"] = 'SUBSYSTEM=="usb", ATTR{idVendor}=="33ae", MODE="0666", RUN+="/tmp/write-device"\n';
  const result = validatePackaging(sources);
  assert(result.failures.some((failure) => failure.includes("product 0003")));
  assert(result.failures.some((failure) => failure.includes("broad persistent permissions")));
});

test("reports a missing required source instead of throwing", () => {
  const sources = validSources();
  delete sources["packaging/debian/control.in"];
  const result = validatePackaging(sources);
  assert(result.failures.some((failure) => failure.includes("missing packaging source")));
});

test("rejects a package hook that starts the user service", () => {
  const sources = validSources();
  sources["packaging/debian/postinst"] += "\nsystemctl --user enable --now studiobridge.service\n";
  const result = validatePackaging(sources);
  assert(result.failures.some((failure) => failure.includes("must not start a per-user service")));
});
