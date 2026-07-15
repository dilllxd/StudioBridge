import fs from "node:fs";
import { pathToFileURL } from "node:url";

export function validateReadonly(health, state, { expectLinkControl = false } = {}) {
  const failures = [];
  const warnings = [];
  const check = (condition, message) => {
    if (!condition) failures.push(message);
  };

  check(health.ok === true, "daemon health is not ready");
  check(health.studio_mode === "beacn", "daemon is not using the real BEACN backend");
  check(health.mixer_mode === "pipeweaver", "daemon is not using the real PipeWeaver backend");
  check(health.hardware_writes_enabled === false, "hardware writes are enabled; stop and restart in read-only mode");
  check(
    health.link_control_enabled === expectLinkControl,
    expectLinkControl
      ? "USB1 Link host control is not enabled"
      : "USB1 Link host control is enabled before the read-only baseline passed",
  );

  const studio = state.studio ?? {};
  const identity = studio.identity ?? {};
  check(identity.status === "connected", `BEACN Studio is not connected: ${identity.error ?? "unknown error"}`);
  check(typeof identity.serial === "string" && identity.serial.length > 0, "Studio serial number could not be read");
  check(typeof identity.firmware === "string" && identity.firmware.length > 0, "Studio firmware version could not be read");
  check(typeof identity.driverless_mode === "boolean", "USB2 driverless-mode state is invalid");
  check(Number.isInteger(studio.microphone?.gain_db) && studio.microphone.gain_db >= 0 && studio.microphone.gain_db <= 69, "microphone gain is outside 0–69 dB");
  check(typeof studio.microphone?.phantom_power === "boolean", "phantom-power state is invalid");
  check(Number.isInteger(studio.headphones?.volume) && studio.headphones.volume >= 0 && studio.headphones.volume <= 100, "headphone level is outside 0–100%");
  check(Number.isInteger(studio.headphones?.mic_monitor) && studio.headphones.mic_monitor >= 0 && studio.headphones.mic_monitor <= 100, "microphone monitor level is outside 0–100%");
  check(typeof studio.headphones?.channels_linked === "boolean", "headphone channel-link state is invalid");
  check(["in_ear_monitors", "line_level", "normal_power", "high_impedance"].includes(studio.headphones?.output_mode), "headphone output mode is invalid");
  check(Number.isInteger(studio.headphones?.mic_output_gain_tenths_db) && studio.headphones.mic_output_gain_tenths_db >= 0 && studio.headphones.mic_output_gain_tenths_db <= 120, "microphone output gain is outside 0–12 dB");
  check(Array.isArray(studio.linked_applications), "Link application list is invalid");

  const mixer = state.mixer ?? {};
  check(mixer.status === "connected", `PipeWeaver is not connected: ${mixer.error ?? "unknown error"}`);
  check(Array.isArray(mixer.channels) && mixer.channels.length > 0, "PipeWeaver exposes no source channels");
  check(Array.isArray(mixer.targets) && mixer.targets.length > 0, "PipeWeaver exposes no output targets");
  check(Array.isArray(mixer.routes), "PipeWeaver route list is invalid");
  for (const channel of mixer.channels ?? []) {
    check(Number.isInteger(channel.personal_volume) && channel.personal_volume >= 0 && channel.personal_volume <= 100, `${channel.name ?? "unnamed channel"} has an invalid Personal volume`);
    check(Number.isInteger(channel.audience_volume) && channel.audience_volume >= 0 && channel.audience_volume <= 100, `${channel.name ?? "unnamed channel"} has an invalid Audience volume`);
    check(typeof channel.meter_id === "string" && channel.meter_id.length > 0, `${channel.name ?? "unnamed channel"} has no meter identifier`);
    check(channel.source_kind === "physical" || channel.source_kind === "virtual", `${channel.name ?? "unnamed channel"} has an invalid source kind`);
    check(typeof channel.volumes_linked === "boolean", `${channel.name ?? "unnamed channel"} has an invalid volume-link state`);
  }

  const channelIds = new Set((mixer.channels ?? []).map((channel) => channel.id));
  const targetsById = new Map((mixer.targets ?? []).map((target) => [target.id, target]));
  const targetMixes = new Set();
  const routedMixes = new Set();
  for (const target of mixer.targets ?? []) {
    check(target.mix === "personal" || target.mix === "audience", `${target.name ?? "unnamed target"} has an invalid mix bus`);
    check(typeof target.meter_id === "string" && target.meter_id.length > 0, `${target.name ?? "unnamed target"} has no meter identifier`);
    targetMixes.add(target.mix);
  }
  for (const route of mixer.routes ?? []) {
    check(channelIds.has(route.source_id), `route refers to unknown source ${route.source_id ?? "(missing)"}`);
    check(targetsById.has(route.target_id), `route refers to unknown target ${route.target_id ?? "(missing)"}`);
    const mix = targetsById.get(route.target_id)?.mix;
    if (mix) routedMixes.add(mix);
  }
  check(targetMixes.has("personal"), "PipeWeaver exposes no Personal output target");
  check(targetMixes.has("audience"), "PipeWeaver exposes no Audience output target");
  check(routedMixes.has("personal"), "PipeWeaver has no route to a Personal target");
  check(routedMixes.has("audience"), "PipeWeaver has no route to an Audience target");

  if ((studio.linked_applications ?? []).length === 0) warnings.push("no Windows Link applications were reported");
  if ((mixer.routes ?? []).length === 0) warnings.push("PipeWeaver currently reports no routes");

  return {
    failures,
    warnings,
    summary: {
      product: identity.product ?? "BEACN Studio",
      firmware: identity.firmware ?? "unknown firmware",
      driverlessMode: identity.driverless_mode,
      micOutputGainDb: (studio.headphones?.mic_output_gain_tenths_db ?? 0) / 10,
      linkApplications: (studio.linked_applications ?? []).length,
      sources: (mixer.channels ?? []).length,
      targets: (mixer.targets ?? []).length,
      routes: (mixer.routes ?? []).length,
      personalRoutes: (mixer.routes ?? []).filter((route) => targetsById.get(route.target_id)?.mix === "personal").length,
      audienceRoutes: (mixer.routes ?? []).filter((route) => targetsById.get(route.target_id)?.mix === "audience").length,
    },
  };
}

function printResult(result, expectLinkControl) {
  const { summary } = result;
  console.log("StudioBridge read-only validation");
  console.log("  Hardware writes: disabled");
  console.log(`  Link host control: ${expectLinkControl ? "enabled (heartbeat and assignments only)" : "disabled"}`);
  console.log(`  Studio: ${summary.product} (${summary.firmware})`);
  console.log(`  USB2 driverless mode: ${summary.driverlessMode ? "on" : "off"} (read only)`);
  console.log(`  Mic output gain: ${summary.micOutputGainDb} dB (read only)`);
  console.log(`  Link applications: ${summary.linkApplications}`);
  console.log(`  PipeWeaver: ${summary.sources} source(s), ${summary.targets} target(s), ${summary.routes} route(s)`);
  console.log(`  Mix routes: ${summary.personalRoutes} Personal, ${summary.audienceRoutes} Audience`);
  for (const warning of result.warnings) console.log(`  WARN: ${warning}`);
  for (const failure of result.failures) console.error(`  FAIL: ${failure}`);
  if (result.failures.length) return 1;
  console.log("  PASS: read-only hardware baseline is internally consistent");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const health = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  const state = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
  const expectLinkControl = process.argv[4] === "link-control";
  process.exitCode = printResult(validateReadonly(health, state, { expectLinkControl }), expectLinkControl);
}
