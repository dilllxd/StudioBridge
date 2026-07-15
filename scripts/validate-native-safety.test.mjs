import assert from "node:assert/strict";
import path from "node:path";
import test from "node:test";
import { readNativeSafetySources, validateNativeSafety } from "./validate-native-safety.mjs";

const root = path.resolve(import.meta.dirname, "..");

function validSources() {
  return readNativeSafetySources(root);
}

function expectFailure(sources, fragment) {
  const result = validateNativeSafety(sources);
  assert(result.failures.some((failure) => failure.includes(fragment)), `${fragment}\n${result.failures.join("\n")}`);
}

test("accepts the repository's explicit native safety boundaries", () => {
  const result = validateNativeSafety(validSources());
  assert.deepEqual(result.failures, []);
  assert(result.checked.callbacks > 0);
  assert(result.checked.clientEndpoints > 0);
});

test("rejects every static write gate in a baseline service", () => {
  for (const argument of ["--allow-hardware-writes", "--enable-link-host", "--enable-dsp-write compressor"]) {
    const sources = validSources();
    const name = "packaging/systemd/studiobridge.service";
    sources[name] = sources[name].replace(" --mixer pipeweaver", ` --mixer pipeweaver ${argument}`);
    expectFailure(sources, "forbidden baseline argument");
  }
});

test("rejects a baseline launcher that delegates safety to an unreviewed wrapper", () => {
  const sources = validSources();
  const name = "packaging/systemd/studiobridge-packaged.service";
  sources[name] = sources[name].replace("/usr/lib/studiobridge/studiobridge-daemon", "/usr/lib/studiobridge/start-daemon-with-writes");
  expectFailure(sources, "read-only BEACN/PipeWeaver baseline");
});

test("rejects a shell prefix even when the approved daemon command remains an exact suffix", () => {
  const sources = validSources();
  const name = "packaging/systemd/studiobridge.service";
  sources[name] = sources[name].replace("ExecStart=%h/", "ExecStart=/bin/sh -c %h/");
  expectFailure(sources, "exact read-only BEACN/PipeWeaver baseline");
});

test("rejects unsafe daemon flags that default on", () => {
  for (const field of ["allow_hardware_writes", "enable_link_host"]) {
    const sources = validSources();
    const name = "crates/studiobridge-daemon/src/main.rs";
    sources[name] = sources[name].replace(new RegExp(`(#\\[arg\\([\\s\\S]*?)(\\)\\]\\s*${field}: bool,)`), "$1, default_value_t = true$2");
    expectFailure(sources, "must default to false");
  }
});

test("rejects a static default DSP module allowlist", () => {
  const sources = validSources();
  const name = "crates/studiobridge-daemon/src/main.rs";
  sources[name] = sources[name].replace("help = \"Enable writes for exactly one validated DSP module; repeat for additional modules\"", "default_value = \"compressor\", help = \"Enable writes for exactly one validated DSP module; repeat for additional modules\"");
  expectFailure(sources, "must not receive a static default");
});

test("rejects removing or moving each USB write guard", () => {
  const mutations = [
    ["self.ensure_writes_enabled()?;", "general write guard removed", "set_microphone must call"],
    ["self.ensure_link_control_enabled()?;", "Link write guard removed", "set_link_assignment must call"],
    ["self.ensure_dsp_write_enabled(update.module())?;", "DSP write guard removed", "set_microphone_dsp must call"],
  ];
  for (const [guard, replacement, expected] of mutations) {
    const sources = validSources();
    const name = "crates/studiobridge-beacn/src/lib.rs";
    sources[name] = sources[name].replace(guard, `// ${replacement}`);
    expectFailure(sources, expected);
  }
});

test("rejects weakening the implementation of every BEACN write gate", () => {
  const mutations = [
    ["if self.allow_writes {", "if true {", "conditional on allow_writes"],
    ["if self.link_control_enabled || self.allow_writes {", "if true {", "conditional on the narrow Link gate"],
    ["if self.enabled_dsp_writes.contains(&module)", "if true", "conditional on a module allowlist"],
  ];
  for (const [before, after, expected] of mutations) {
    const sources = validSources();
    const name = "crates/studiobridge-beacn/src/lib.rs";
    sources[name] = sources[name].replace(before, after);
    expectFailure(sources, expected);
  }
});

test("rejects callbacks for every excluded native control family", () => {
  const callbacks = [
    ["set-phantom-power", "phantom-power write"],
    ["update-firmware", "firmware update"],
    ["factory-reset-device", "factory/device reset"],
    ["set-lighting-colour", "lighting write"],
    ["set-limiter-threshold", "unverified limiter write"],
    ["write-unrestricted-device-storage", "unrestricted device storage write"],
    ["enable-hardware-writes", "general hardware write"],
    ["set-driverless-mode", "driverless-mode write"],
    ["set-microphone-gain", "microphone gain write"],
    ["set-microphone-output-gain", "microphone output-gain write"],
    ["set-headphone-volume", "headphone level/amp write"],
    ["set-mic-monitor", "microphone monitor-level write"],
    ["set-output-mode", "hardware output-mode write"],
    ["set-amp-power", "headphone amp-power write"],
    ["enable-writes", "write-gate enablement"],
    ["allow-device-writes", "write-gate enablement"],
  ];
  for (const [callback, expected] of callbacks) {
    const sources = validSources();
    const name = "crates/studiobridge-desktop/ui/app-window.slint";
    sources[name] += `\nexport component UnsafeMutation { callback ${callback}(bool); }\n`;
    expectFailure(sources, expected);
  }
});

test("rejects an excluded Rust handler even if the Slint declaration is hidden", () => {
  const sources = validSources();
  const name = "crates/studiobridge-desktop/src/main.rs";
  sources[name] += "\nfn unsafe_mutation(app: &MainWindow) { app.on_set_lighting_colour(|_| {}); }\n";
  expectFailure(sources, "desktop handler set_lighting_colour exposes excluded lighting write");
});

test("rejects excluded desktop client methods and non-allowlisted BEACN endpoints", () => {
  const sources = validSources();
  const name = "crates/studiobridge-desktop/src/client.rs";
  sources[name] += '\nimpl DaemonClient { pub fn set_phantom_power(&self) { let _ = "/api/studio/microphone"; } }\n';
  expectFailure(sources, "desktop client method set_phantom_power");
  expectFailure(sources, "non-allowlisted BEACN write surface /api/studio/microphone");
});

test("rejects an excluded async desktop client method", () => {
  const sources = validSources();
  const name = "crates/studiobridge-desktop/src/client.rs";
  sources[name] += "\nimpl DaemonClient { pub async fn flash_firmware(&self) {} }\n";
  expectFailure(sources, "desktop client method flash_firmware exposes excluded firmware update");
});

test("rejects an excluded daemon route handler behind a generic path", () => {
  const sources = validSources();
  const name = "crates/studiobridge-daemon/src/main.rs";
  sources[name] += '\nfn unsafe_router() { Router::new().route("/api/device/control", post(set_lighting_colour)); }\n';
  expectFailure(sources, "daemon route handler set_lighting_colour exposes excluded lighting write");
});

test("rejects every excluded daemon endpoint family", () => {
  for (const [endpoint, expected] of [
    ["/api/device/firmware", "firmware update"],
    ["/api/device/factory-reset", "factory/device reset"],
    ["/api/studio/lighting", "lighting write"],
    ["/api/studio/limiter", "unverified limiter write"],
    ["/api/studio/unrestricted-storage", "unrestricted device storage write"],
  ]) {
    const sources = validSources();
    const name = "crates/studiobridge-daemon/src/main.rs";
    sources[name] += `\nconst UNSAFE_MUTATION: &str = "${endpoint}";\n`;
    expectFailure(sources, expected);
  }
});

test("does not ban explanatory labels, comments, or read-back properties", () => {
  const sources = validSources();
  const slintName = "crates/studiobridge-desktop/ui/app-window.slint";
  const rustName = "crates/studiobridge-desktop/src/main.rs";
  sources[slintName] += '\nexport component SafetyLabel { in property <bool> phantom-power-readback; in property <float> mic-monitor-readback; in property <string> output-mode-readback; in property <string> amp-power-readback; Text { text: "Firmware, reset, lighting, limiter, storage, output mode, amp power, mic monitor, and enable writes controls are excluded"; } }\n';
  sources[rustName] += "\n// General hardware writes, Link host, DSP writes, and device storage stay gated.\n";
  const result = validateNativeSafety(sources);
  assert.deepEqual(result.failures, []);
});

test("rejects real comparison integration dependencies without banning test dependencies", () => {
  for (const dependency of ["pipewire", "studiobridge-beacn", "reqwest"]) {
    const sources = validSources();
    const name = "crates/studiobridge-comparison/Cargo.toml";
    sources[name] = sources[name].replace("[dependencies]", `[dependencies]\n${dependency} = "1"`);
    expectFailure(sources, `forbidden runtime dependency ${dependency}`);
  }

  const testOnly = validSources();
  testOnly["crates/studiobridge-comparison/Cargo.toml"] += "\n[dev-dependencies]\nproptest = \"1\"\n";
  assert.deepEqual(validateNativeSafety(testOnly).failures, []);

  const realDeviceTest = validSources();
  realDeviceTest["crates/studiobridge-comparison/Cargo.toml"] += "\n[dev-dependencies]\npipewire = \"0.9\"\n";
  expectFailure(realDeviceTest, "forbidden integration test dependency pipewire");

  const targetSpecific = validSources();
  targetSpecific["crates/studiobridge-comparison/Cargo.toml"] += "\n[target.'cfg(target_os = \"linux\")'.dependencies]\npipewire = \"0.9\"\n";
  expectFailure(targetSpecific, "forbidden target.'cfg(target_os = \"linux\")'.dependencies dependency pipewire");
});

test("rejects production audio drivers and risky standard-library integration paths", () => {
  for (const [code, expected] of [
    ["struct LinuxDriver; impl AudioDriver for LinuxDriver {}", "production AudioDriver implementation LinuxDriver"],
    ["struct QualifiedDriver; impl crate::AudioDriver for QualifiedDriver {}", "production AudioDriver implementation QualifiedDriver"],
    ["fn probe() { let _ = std::fs::read(\"/proc/asound/cards\"); }", "filesystem, network, process, environment, or I/O API"],
    ["fn open() { pipewire::init(); }", "device, audio, control, or network crate path"],
    ["unsafe { open_audio_device(); }", "unsafe integration code"],
  ]) {
    const sources = validSources();
    sources["crates/studiobridge-comparison/src/linux.rs"] = code;
    expectFailure(sources, expected);
  }
});

test("allows comparison safety prose, fake drivers, and mutation-test strings", () => {
  const sources = validSources();
  sources["crates/studiobridge-comparison/src/fake_test_support.rs"] = `
    // pipewire::Stream and std::fs are forbidden in production.
    const EXPLANATION: &str = "reqwest and BEACN Link/DSP are intentionally absent";
    struct TestAudioDriver;
    impl AudioDriver for TestAudioDriver {}
  `;
  assert.deepEqual(validateNativeSafety(sources).failures, []);
});

test("rejects comparison production consumption before destination policy review", () => {
  const sources = validSources();
  sources["crates/studiobridge-desktop/Cargo.toml"] += '\nstudiobridge-comparison = { path = "../studiobridge-comparison" }\n';
  expectFailure(sources, "wires the comparison core into production before destination-generation policy review");
});

test("rejects weakening comparison teardown, thread, or scrubbing boundaries", () => {
  for (const [before, after, expected] of [
    ["pub trait AudioDriver: Send + 'static", "pub trait AudioDriver", "worker-thread safe"],
    ["fn deactivate_all(&mut self);", "fn deactivate_all(&mut self) -> Result<(), AudioError>;", "infallible ambiguous-stream teardown"],
    ["self.samples.zeroize();", "self.samples.fill(0.0);", "non-elidable zeroization"],
  ]) {
    const sources = validSources();
    const name = "crates/studiobridge-comparison/src/lib.rs";
    sources[name] = sources[name].replace(before, after);
    expectFailure(sources, expected);
  }
});
