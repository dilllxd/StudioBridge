import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REQUIRED_FILES = [
  "Cargo.toml",
  "crates/studiobridge-comparison/Cargo.toml",
  "crates/studiobridge-comparison/src/lib.rs",
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

const COMPARISON_ALLOWED_RUNTIME_DEPENDENCIES = new Set(["zeroize"]);
const COMPARISON_FORBIDDEN_INTEGRATION_DEPENDENCY = /^(?:pipewire|libspa|spa-sys|alsa|cpal|rodio|jack|beacn-lib|studiobridge-beacn|studiobridge-pipeweaver|reqwest|axum|hyper|ureq|tungstenite|tokio-tungstenite|libusb|rusb|hidapi|windows|windows-sys)$/;
const COMPARISON_FORBIDDEN_INTEGRATION_PATHS = [
  [/(?:^|\W)(?:pipewire|libspa|spa_sys|alsa|cpal|rodio|jack|beacn_lib|studiobridge_beacn|studiobridge_pipeweaver|reqwest|axum|hyper|ureq|tungstenite|tokio_tungstenite)\s*::/, "device, audio, control, or network crate path"],
  [/\bstd\s*::\s*(?:fs|net|path|process|env|io)\b/, "filesystem, network, process, environment, or I/O API"],
  [/\buse\s+std\s*::\s*\{[^}]*\b(?:fs|net|path|process|env|io)\b/, "filesystem, network, process, environment, or I/O import"],
  [/\bextern\s+"C"\b|#\s*\[\s*link\b/, "foreign device or audio API"],
  [/#\s*\[\s*path\s*=|\b(?:include|include_bytes|include_str)!\s*\(/, "unreviewed external source or data inclusion"],
  [/\bunsafe\s*\{/, "unsafe integration code"],
];

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

function tomlSection(source, name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const header = new RegExp(`^\\[${escaped}\\]\\s*$`, "m").exec(source);
  if (!header) return "";
  const bodyStart = header.index + header[0].length;
  const nextHeader = /^\[[^\]]+\]\s*$/m.exec(source.slice(bodyStart));
  return source.slice(bodyStart, nextHeader ? bodyStart + nextHeader.index : source.length);
}

function tomlDependencySections(source) {
  const headers = [...source.matchAll(/^\[([^\]]*dependencies)\]\s*$/gm)];
  return headers.map((header, index) => {
    const bodyStart = header.index + header[0].length;
    const bodyEnd = headers[index + 1]?.index ?? source.length;
    const nextAnyHeader = /^\[[^\]]+\]\s*$/m.exec(source.slice(bodyStart, bodyEnd));
    return {
      name: header[1],
      body: source.slice(bodyStart, nextAnyHeader ? bodyStart + nextAnyHeader.index : bodyEnd),
    };
  });
}

function dependencyNames(body) {
  return body
    .split(/\r?\n/)
    .map((line) => line.replace(/#.*/, "").trim())
    .filter(Boolean)
    .map((line) => line.match(/^([A-Za-z0-9_-]+)(?:\.workspace)?\s*=/)?.[1])
    .filter(Boolean);
}

// Safety checks inspect code tokens, not prose. This deliberately removes
// comments and string/character literals so documentation and mutation-test
// explanations may name forbidden integrations without creating false alarms.
function rustCodeOnly(source) {
  return source
    .replace(/r(#{0,16})"[\s\S]*?"\1/g, " ")
    .replace(/"(?:\\.|[^"\\])*"/g, " ")
    .replace(/'(?:\\.|[^'\\])'/g, " ")
    .replace(/\/\*[\s\S]*?\*\//g, " ")
    .replace(/\/\/[^\n]*/g, " ");
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

  const comparisonManifest = read("crates/studiobridge-comparison/Cargo.toml");
  const runtimeDependencies = dependencyNames(tomlSection(comparisonManifest, "dependencies"));
  for (const dependency of runtimeDependencies) {
    check(COMPARISON_ALLOWED_RUNTIME_DEPENDENCIES.has(dependency), `comparison policy core has forbidden runtime dependency ${dependency}`);
  }
  for (const section of tomlDependencySections(comparisonManifest)) {
    for (const dependency of dependencyNames(section.body)) {
      if (section.name.endsWith("dev-dependencies")) {
        check(!COMPARISON_FORBIDDEN_INTEGRATION_DEPENDENCY.test(dependency), `comparison policy core has forbidden integration test dependency ${dependency}`);
      } else {
        check(COMPARISON_ALLOWED_RUNTIME_DEPENDENCIES.has(dependency), `comparison policy core has forbidden ${section.name} dependency ${dependency}`);
      }
    }
  }
  check(runtimeDependencies.includes("zeroize"), "comparison policy core must retain non-elidable sample scrubbing");
  for (const [name, source] of Object.entries(files)) {
    if (name !== "Cargo.toml" && name.endsWith("Cargo.toml") && !name.startsWith("crates/studiobridge-comparison/")) {
      check(!source.includes("studiobridge-comparison"), `${name} wires the comparison core into production before destination-generation policy review`);
    }
  }

  const comparisonRustSources = Object.entries(files).filter(([name]) =>
    name.startsWith("crates/studiobridge-comparison/") && name.endsWith(".rs")
  );
  check(comparisonRustSources.length > 0, "comparison policy core has no reviewed Rust source");
  for (const [name, source] of comparisonRustSources) {
    const code = rustCodeOnly(source);
    for (const [pattern, description] of COMPARISON_FORBIDDEN_INTEGRATION_PATHS) {
      check(!pattern.test(code), `${name} contains forbidden ${description}`);
    }
    for (const implementation of allMatches(code, /\bimpl(?:\s*<[^>{}]*>)?\s+(?:(?:crate|self|super|studiobridge_comparison)\s*::\s*)?AudioDriver\s+for\s+([A-Za-z_][A-Za-z0-9_]*)/g)) {
      check(/(?:Fake|Mock|Test)/.test(implementation), `${name} adds production AudioDriver implementation ${implementation} before destination-generation policy review`);
    }
  }
  const comparisonLib = rustCodeOnly(read("crates/studiobridge-comparison/src/lib.rs"));
  check(/pub\s+trait\s+AudioDriver\s*:\s*Send\s*\+\s*'static/.test(comparisonLib), "comparison AudioDriver must remain worker-thread safe");
  check(/fn\s+deactivate_all\s*\(\s*&mut\s+self\s*\)\s*;/.test(comparisonLib), "comparison AudioDriver must retain infallible ambiguous-stream teardown");
  check(/self\.samples\.zeroize\s*\(\s*\)/.test(comparisonLib), "comparison sample slots must use non-elidable zeroization");

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
  const names = new Set(REQUIRED_FILES);
  const collect = (directory, include) => {
    for (const entry of fs.readdirSync(path.join(root, directory), { withFileTypes: true })) {
      const relative = path.posix.join(directory.replaceAll("\\", "/"), entry.name);
      if (entry.isDirectory()) collect(relative, include);
      else if (include(relative)) names.add(relative);
    }
  };
  collect("crates", (name) => name.endsWith("/Cargo.toml"));
  collect("crates/studiobridge-comparison", (name) => name.endsWith(".rs"));
  return Object.fromEntries([...names].map((name) => [name, fs.readFileSync(path.join(root, name), "utf8")]));
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
