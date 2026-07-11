import fs from "node:fs";
import { pathToFileURL } from "node:url";

export function validateReadonly(health, state) {
  const failures = [];
  const warnings = [];
  const check = (condition, message) => {
    if (!condition) failures.push(message);
  };

  check(health.ok === true, "daemon health is not ready");
  check(health.studio_mode === "beacn", "daemon is not using the real BEACN backend");
  check(health.mixer_mode === "pipeweaver", "daemon is not using the real PipeWeaver backend");
  check(health.hardware_writes_enabled === false, "hardware writes are enabled; stop and restart in read-only mode");

  const studio = state.studio ?? {};
  const identity = studio.identity ?? {};
  check(identity.status === "connected", `BEACN Studio is not connected: ${identity.error ?? "unknown error"}`);
  check(typeof identity.serial === "string" && identity.serial.length > 0, "Studio serial number could not be read");
  check(typeof identity.firmware === "string" && identity.firmware.length > 0, "Studio firmware version could not be read");
  check(Number.isInteger(studio.microphone?.gain_db) && studio.microphone.gain_db >= 0 && studio.microphone.gain_db <= 69, "microphone gain is outside 0–69 dB");
  check(typeof studio.microphone?.phantom_power === "boolean", "phantom-power state is invalid");
  check(Number.isInteger(studio.headphones?.volume) && studio.headphones.volume >= 0 && studio.headphones.volume <= 100, "headphone level is outside 0–100%");
  check(Array.isArray(studio.linked_applications), "Link application list is invalid");

  const mixer = state.mixer ?? {};
  check(mixer.status === "connected", `PipeWeaver is not connected: ${mixer.error ?? "unknown error"}`);
  check(Array.isArray(mixer.channels) && mixer.channels.length > 0, "PipeWeaver exposes no source channels");
  check(Array.isArray(mixer.targets) && mixer.targets.length > 0, "PipeWeaver exposes no output targets");
  check(Array.isArray(mixer.routes), "PipeWeaver route list is invalid");
  for (const channel of mixer.channels ?? []) {
    check(Number.isInteger(channel.personal_volume) && channel.personal_volume >= 0 && channel.personal_volume <= 100, `${channel.name ?? "unnamed channel"} has an invalid Personal volume`);
    check(Number.isInteger(channel.audience_volume) && channel.audience_volume >= 0 && channel.audience_volume <= 100, `${channel.name ?? "unnamed channel"} has an invalid Audience volume`);
  }

  if ((studio.linked_applications ?? []).length === 0) warnings.push("no Windows Link applications were reported");
  if ((mixer.routes ?? []).length === 0) warnings.push("PipeWeaver currently reports no routes");

  return {
    failures,
    warnings,
    summary: {
      product: identity.product ?? "BEACN Studio",
      firmware: identity.firmware ?? "unknown firmware",
      linkApplications: (studio.linked_applications ?? []).length,
      sources: (mixer.channels ?? []).length,
      targets: (mixer.targets ?? []).length,
      routes: (mixer.routes ?? []).length,
    },
  };
}

function printResult(result) {
  const { summary } = result;
  console.log("StudioBridge read-only validation");
  console.log("  Hardware writes: disabled");
  console.log(`  Studio: ${summary.product} (${summary.firmware})`);
  console.log(`  Link applications: ${summary.linkApplications}`);
  console.log(`  PipeWeaver: ${summary.sources} source(s), ${summary.targets} target(s), ${summary.routes} route(s)`);
  for (const warning of result.warnings) console.log(`  WARN: ${warning}`);
  for (const failure of result.failures) console.error(`  FAIL: ${failure}`);
  if (result.failures.length) return 1;
  console.log("  PASS: read-only hardware baseline is internally consistent");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const health = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  const state = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
  process.exitCode = printResult(validateReadonly(health, state));
}
