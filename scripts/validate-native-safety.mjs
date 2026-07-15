import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REQUIRED_FILES = [
  "crates/studiobridge-beacn/src/lib.rs",
  "crates/studiobridge-daemon/src/main.rs",
  "crates/studiobridge-desktop/src/client.rs",
  "crates/studiobridge-desktop/src/main.rs",
  "crates/studiobridge-desktop/ui/app-window.slint",
  "packaging/desktop/studiobridge-autostart.desktop",
  "packaging/desktop/studiobridge.desktop",
  "packaging/systemd/studiobridge-packaged.service",
  "packaging/systemd/studiobridge.service",
];

const BASELINE_SERVICE_COMMANDS = new Map([
  ["packaging/systemd/studiobridge.service", "%h/.local/bin/studiobridge-daemon --studio beacn --mixer pipeweaver"],
  ["packaging/systemd/studiobridge-packaged.service", "/usr/lib/studiobridge/studiobridge-daemon --studio beacn --mixer pipeweaver"],
]);
const BASELINE_DESKTOP_COMMANDS = new Map([
  ["packaging/desktop/studiobridge.desktop", "studiobridge-desktop"],
  ["packaging/desktop/studiobridge-autostart.desktop", "studiobridge-desktop --background"],
]);
const FORBIDDEN_BASELINE_ARGUMENTS = [
  "--allow-hardware-writes",
  "--enable-link-host",
  "--enable-dsp-write",
];
const ALLOWED_DESKTOP_STUDIO_ENDPOINTS = new Set([
  "/api/studio/dsp-arm",
  "/api/studio/dsp-disarm",
  "/api/studio/link-assignment",
  "/api/studio/microphone-dsp",
]);
const ALLOWED_DAEMON_STUDIO_ENDPOINTS = new Set([
  ...ALLOWED_DESKTOP_STUDIO_ENDPOINTS,
  // The daemon keeps this general-write endpoint for deliberately gated
  // command-line validation. The native desktop must never call it.
  "/api/studio/microphone",
]);

function normalizeIdentifier(value) {
  return value.toLowerCase().replaceAll("-", "_");
}

function forbiddenSurfaceReason(identifier) {
  const name = normalizeIdentifier(identifier);
  const rules = [
    [/(^|_)phantom(_|$)/, "phantom-power write"],
    [/(^|_)firmware(_|$)/, "firmware update"],
    [/(factory.*reset|reset.*factory|device.*reset|reset.*device)/, "factory/device reset"],
    [/(^|_)(lighting|led)(_|$)/, "lighting write"],
    [/(^|_)limiter(_|$)/, "unverified limiter write"],
    [/(write.*(device|raw|unrestricted).*storage|(device|raw|unrestricted).*storage.*write|unrestricted.*(storage|memory))/, "unrestricted device storage write"],
    [/(hardware.*write|write.*hardware)/, "general hardware write"],
    [/(^|_)driverless(_|$)/, "driverless-mode write"],
    [/((microphone|mic).*output.*gain|output.*gain.*(microphone|mic))/, "microphone output-gain write"],
    [/((microphone|mic).*gain|gain.*(microphone|mic))/, "microphone gain write"],
    [/(^|_)mic_monitor(_|$)/, "microphone monitor-level write"],
    [/(^|_)output_mode(_|$)/, "hardware output-mode write"],
    [/(^|_)amp_power(_|$)/, "headphone amp-power write"],
    [/(headphone.*(volume|level|monitor|amp|power|output.*mode))/, "headphone level/amp write"],
    [/(^|_)(enable|allow)(?:_[a-z0-9]+)*_writes?(_|$)|(^|_)writes?_(enabled|allowed)(_|$)/, "write-gate enablement"],
  ];
  return rules.find(([pattern]) => pattern.test(name))?.[1];
}

function endpointReason(endpoint) {
  const semantic = endpoint.replaceAll("/", "_").replaceAll("-", "_");
  return forbiddenSurfaceReason(semantic);
}

function allMatches(source, pattern, group = 1) {
  return [...source.matchAll(pattern)].map((match) => match[group]);
}

function rustFunctionSection(source, name) {
  const start = source.search(new RegExp(`(?:async\\s+)?fn\\s+${name}\\b`));
  if (start < 0) return "";
  const next = source.slice(start + 1).search(/\n\s*(?:async\s+)?fn\s+[A-Za-z0-9_]+\b/);
  return next < 0 ? source.slice(start) : source.slice(start, start + 1 + next);
}

function rustFieldDeclaration(source, name) {
  const field = source.search(new RegExp(`\\n\\s*${name}:`));
  if (field < 0) return "";
  const nearestAttribute = source.lastIndexOf("#[arg(", field);
  const nearestBlank = source.lastIndexOf("\n\n", field);
  const start = nearestAttribute > nearestBlank ? nearestAttribute : nearestBlank + 2;
  const end = source.indexOf(",", field);
  return source.slice(start, end < 0 ? field + name.length : end + 1);
}

function checkGateBeforeQueue(check, source, functionName, gateCall, queueCall) {
  const section = rustFunctionSection(source, functionName);
  check(Boolean(section), `BEACN backend is missing ${functionName}`);
  const gateIndex = section.indexOf(gateCall);
  const queueIndex = section.indexOf(queueCall);
  check(gateIndex >= 0, `${functionName} must call ${gateCall}`);
  check(queueIndex >= 0, `${functionName} is missing its expected USB command queue`);
  check(gateIndex >= 0 && queueIndex >= 0 && gateIndex < queueIndex, `${functionName} must enforce ${gateCall} before queueing USB work`);
}

export function validateNativeSafety(files) {
  const failures = [];
  const check = (condition, message) => {
    if (!condition) failures.push(message);
  };
  const read = (name) => {
    const source = files[name];
    check(typeof source === "string", `missing native safety source: ${name}`);
    return source ?? "";
  };

  for (const name of REQUIRED_FILES) read(name);

  for (const [serviceName, expectedCommand] of BASELINE_SERVICE_COMMANDS) {
    const service = read(serviceName);
    const commands = allMatches(service, /^ExecStart=(.+)$/gm);
    check(commands.length === 1, `${serviceName} must have exactly one baseline ExecStart`);
    check(commands[0] === expectedCommand, `${serviceName} must launch only the exact read-only BEACN/PipeWeaver baseline`);
    for (const argument of FORBIDDEN_BASELINE_ARGUMENTS) {
      check(!service.includes(argument), `${serviceName} enables forbidden baseline argument ${argument}`);
    }
    check(!/^Environment=.*(?:HARDWARE_WRITE|LINK_HOST|DSP_WRITE)[^=]*=(?:1|true|yes)$/gim.test(service), `${serviceName} enables a write gate through its environment`);
  }

  for (const [desktopName, expected] of BASELINE_DESKTOP_COMMANDS) {
    const desktop = read(desktopName);
    const commands = allMatches(desktop, /^Exec=(.+)$/gm);
    check(commands.length === 1, `${desktopName} must have exactly one Exec entry`);
    check(commands[0] === expected, `${desktopName} must launch only ${expected}`);
    for (const argument of FORBIDDEN_BASELINE_ARGUMENTS) {
      check(!desktop.includes(argument), `${desktopName} enables forbidden baseline argument ${argument}`);
    }
  }

  const daemon = read("crates/studiobridge-daemon/src/main.rs");
  for (const fieldName of ["allow_hardware_writes", "enable_link_host"]) {
    const declaration = rustFieldDeclaration(daemon, fieldName);
    check(Boolean(declaration), `daemon Args is missing ${fieldName}`);
    check(/:\s*bool\s*,/.test(declaration), `daemon ${fieldName} must remain an opt-in boolean`);
    check(!/default_value/.test(declaration), `daemon ${fieldName} must default to false without a static override`);
  }
  const dspField = rustFieldDeclaration(daemon, "enabled_dsp_writes");
  check(/Vec\s*<\s*DspWriteModuleArg\s*>/.test(dspField), "daemon DSP write modules must remain an empty-by-default repeated allowlist");
  check(!/default_value/.test(dspField), "daemon DSP write modules must not receive a static default");

  const compactDaemon = daemon.replace(/\s+/g, " ");
  check(compactDaemon.includes("hardware_writes_enabled: args.allow_hardware_writes,"), "runtime hardware-write health must reflect only the explicit CLI gate");
  check(compactDaemon.includes("link_control_enabled: args.enable_link_host || args.allow_hardware_writes,"), "runtime Link health must reflect only explicit Link/general-write gates");
  check(/BeacnStudioBackend::spawn\(\s*args\.allow_hardware_writes,\s*args\.enable_link_host,\s*enabled_dsp_writes\.clone\(\),\s*dsp_write_gate\.clone\(\),\s*\)/m.test(daemon), "daemon must pass only explicit write gates into the BEACN backend");

  const beacn = read("crates/studiobridge-beacn/src/lib.rs");
  checkGateBeforeQueue(check, beacn, "set_microphone", "self.ensure_writes_enabled()?", ".send(DeviceCommand::SetMicrophone");
  checkGateBeforeQueue(check, beacn, "set_link_assignment", "self.ensure_link_control_enabled()?", ".send(DeviceCommand::SetLink");
  checkGateBeforeQueue(check, beacn, "set_microphone_dsp", "self.ensure_dsp_write_enabled(update.module())?", ".send(DeviceCommand::SetDsp");
  const generalGate = rustFunctionSection(beacn, "ensure_writes_enabled").replace(/\s+/g, " ");
  const linkGate = rustFunctionSection(beacn, "ensure_link_control_enabled").replace(/\s+/g, " ");
  const dspGate = rustFunctionSection(beacn, "ensure_dsp_write_enabled").replace(/\s+/g, " ");
  check(/if self\.allow_writes \{ Ok\(\(\)\) \} else/.test(generalGate), "general BEACN writes must remain conditional on allow_writes");
  check(linkGate.includes("if self.link_control_enabled || self.allow_writes"), "Link writes must remain conditional on the narrow Link gate or explicit general-write gate");
  check(dspGate.includes("self.enabled_dsp_writes.contains(&module) || self.leased_dsp_writes.enabled_module()? == Some(module)"), "DSP writes must remain conditional on a module allowlist or exclusive timed lease");

  const slint = read("crates/studiobridge-desktop/ui/app-window.slint");
  const desktopMain = read("crates/studiobridge-desktop/src/main.rs");
  const client = read("crates/studiobridge-desktop/src/client.rs");
  const callbackNames = allMatches(slint, /\bcallback\s+([A-Za-z0-9_-]+)/g);
  const handlerNames = allMatches(desktopMain, /\b[A-Za-z_][A-Za-z0-9_]*\.on_([A-Za-z0-9_]+)/g);
  const clientMethods = allMatches(client, /\bpub\s+(?:async\s+)?fn\s+([A-Za-z0-9_]+)/g);
  for (const [kind, names] of [["Slint callback", callbackNames], ["desktop handler", handlerNames], ["desktop client method", clientMethods]]) {
    for (const name of names) {
      const reason = forbiddenSurfaceReason(name);
      check(!reason, `${kind} ${name} exposes excluded ${reason}`);
    }
  }

  const clientEndpoints = new Set(allMatches(client, /["`]((?:\{\})?\/api\/[A-Za-z0-9_\-/{ }]+)["`]/g).map((value) => value.replace(/^\{\}/, "")));
  for (const endpoint of clientEndpoints) {
    const reason = endpointReason(endpoint);
    check(!reason, `desktop client endpoint ${endpoint} exposes excluded ${reason}`);
    if (endpoint.startsWith("/api/studio/")) {
      check(ALLOWED_DESKTOP_STUDIO_ENDPOINTS.has(endpoint), `desktop client adds non-allowlisted BEACN write surface ${endpoint}`);
    }
  }

  const daemonEndpoints = new Set(allMatches(daemon, /["`](\/api\/[A-Za-z0-9_\-/]+)["`]/g));
  for (const endpoint of daemonEndpoints) {
    const reason = endpointReason(endpoint);
    check(!reason, `daemon endpoint ${endpoint} exposes excluded ${reason}`);
    if (endpoint.startsWith("/api/studio/")) {
      check(ALLOWED_DAEMON_STUDIO_ENDPOINTS.has(endpoint), `daemon adds non-allowlisted BEACN write surface ${endpoint}`);
    }
  }
  const daemonRouteHandlers = allMatches(daemon, /\.route\(\s*["`]\/api\/[A-Za-z0-9_\-/]+["`]\s*,\s*(?:get|post)\(\s*([A-Za-z0-9_]+)/g);
  for (const handler of daemonRouteHandlers) {
    const reason = forbiddenSurfaceReason(handler);
    check(!reason, `daemon route handler ${handler} exposes excluded ${reason}`);
  }

  return {
    failures,
    checked: {
      callbacks: callbackNames.length,
      handlers: handlerNames.length,
      clientMethods: clientMethods.length,
      clientEndpoints: clientEndpoints.size,
      daemonEndpoints: daemonEndpoints.size,
      daemonRouteHandlers: daemonRouteHandlers.length,
    },
  };
}

export function readNativeSafetySources(root) {
  return Object.fromEntries(REQUIRED_FILES.map((name) => [name, fs.readFileSync(path.join(root, name), "utf8")]));
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
  const result = validateNativeSafety(readNativeSafetySources(root));
  for (const failure of result.failures) console.error(`FAIL: ${failure}`);
  if (result.failures.length) {
    process.exitCode = 1;
  } else {
    const { checked } = result;
    console.log(`Native safety boundaries verified (${checked.callbacks} callbacks, ${checked.handlers} handlers, ${checked.clientEndpoints} client endpoints, ${checked.daemonEndpoints} daemon endpoints).`);
  }
}
