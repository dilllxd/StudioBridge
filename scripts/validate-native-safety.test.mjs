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

function withSimulatedDesktopFeature() {
  const sources = validSources();
  const manifest = "crates/studiobridge-desktop/Cargo.toml";
  if (!sources[manifest].includes('[features]')) {
    sources[manifest] = sources[manifest].replace("[dependencies]", '[features]\ndefault = []\nextended-workspaces = []\nsimulated-comparison = ["extended-workspaces", "dep:studiobridge-comparison"]\n\n[dependencies]');
  }
  if (!sources[manifest].includes('studiobridge-comparison = {')) {
    sources[manifest] = sources[manifest].replace("[target.'cfg(target_os", 'studiobridge-comparison = { path = "../studiobridge-comparison", optional = true }\n\n[target.\'cfg(target_os');
  }
  if (!sources["crates/studiobridge-desktop/src/main.rs"].includes("ComparisonWorker")) {
    sources["crates/studiobridge-desktop/src/main.rs"] += `
      #[cfg(feature = "simulated-comparison")]
      fn install_simulated_comparison() {
        let _ = studiobridge_comparison::ComparisonWorker::spawn_simulated(
          studiobridge_comparison::FakeAudioDriver::default(), 1
        );
        let _label = "Simulated comparison — no device audio";
      }
    `;
  }
  return sources;
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

test("rejects comparison consumption outside the reviewed optional desktop feature", () => {
  const sources = validSources();
  sources["crates/studiobridge-desktop/Cargo.toml"] = sources["crates/studiobridge-desktop/Cargo.toml"]
    .replace('simulated-comparison = ["extended-workspaces", "dep:studiobridge-comparison"]', "simulated-comparison = []")
    .replace(', optional = true', '');
  expectFailure(sources, "must depend exactly on extended-workspaces and the optional comparison core");
  expectFailure(sources, "must remain optional");
});

test("allows only the exact Mixer-default, extended, and simulated desktop feature contract", () => {
  assert.deepEqual(validateNativeSafety(withSimulatedDesktopFeature()).failures, []);

  const mutations = [
    ['default = []', 'default = ["extended-workspaces"]', "default feature set must remain empty"],
    ['extended-workspaces = []', 'extended-workspaces = ["dep:studiobridge-comparison"]', "extended-workspaces feature must not activate dependencies"],
    [', optional = true', '', "must remain optional"],
    ['["extended-workspaces", "dep:studiobridge-comparison"]', '["dep:studiobridge-comparison"]', "must depend exactly on extended-workspaces"],
    ['optional = true', 'optional = true, features = ["production"]', "only path and optional metadata"],
  ];
  for (const [before, after, expected] of mutations) {
    const sources = withSimulatedDesktopFeature();
    const name = "crates/studiobridge-desktop/Cargo.toml";
    sources[name] = sources[name].replace(before, after);
    expectFailure(sources, expected);
  }

  const alias = withSimulatedDesktopFeature();
  alias["crates/studiobridge-desktop/Cargo.toml"] = alias["crates/studiobridge-desktop/Cargo.toml"].replace(
    'simulated-comparison = ["extended-workspaces", "dep:studiobridge-comparison"]',
    'simulated-comparison = ["extended-workspaces", "dep:studiobridge-comparison"]\nproduction = []',
  );
  expectFailure(alias, "feature table may contain only");
});

test("rejects default UI workspace rails that escape Mixer-only mode", () => {
  const uiName = "crates/studiobridge-desktop/ui/app-window.slint";

  const defaultExtended = validSources();
  defaultExtended[uiName] = defaultExtended[uiName].replace(
    "in property <bool> extended-workspaces-enabled: false;",
    "in property <bool> extended-workspaces-enabled: true;",
  );
  expectFailure(defaultExtended, "default UI must keep extended workspaces disabled");

  for (const label of ["STUDIO", "MIC", "LIGHTING", "DEVICE"]) {
    const sources = validSources();
    const line = sources[uiName]
      .split(/\r?\n/)
      .find((candidate) => candidate.includes("RailButton") && candidate.includes(`label: "${label}"`));
    assert(line, `missing ${label} mutation fixture`);
    sources[uiName] = sources[uiName].replace(
      line,
      line.replace("interactive: root.extended-workspaces-enabled;", "interactive: true;"),
    );
    expectFailure(sources, `${label} rail must be noninteractive unless extended-workspaces is enabled`);
  }

  const settings = validSources();
  const settingsLine = settings[uiName]
    .split(/\r?\n/)
    .find((candidate) => candidate.includes("RailButton") && candidate.includes('label: "SETTINGS"'));
  assert(settingsLine, "missing Settings mutation fixture");
  settings[uiName] = settings[uiName].replace(
    settingsLine,
    settingsLine.replace("active: root.active-page == 4;", "active: root.active-page == 4; interactive: false;"),
  );
  expectFailure(settings, "Settings rail must remain interactive in the default UI");
});

test("rejects accessibility or profile gating regressions in disabled workspaces", () => {
  const uiName = "crates/studiobridge-desktop/ui/app-window.slint";
  const accessibility = validSources();
  accessibility[uiName] = accessibility[uiName].replace(
    "accessible-enabled: root.interactive;",
    "accessible-enabled: true;",
  );
  expectFailure(accessibility, "keyboard-, pointer-, and accessibility-disabled");

  const profiles = validSources();
  profiles[uiName] = profiles[uiName].replace(
    "if root.extended-workspaces-enabled: Rectangle {",
    "Rectangle {",
  );
  expectFailure(profiles, "Studio profile section must remain gated by extended-workspaces");
});

test("rejects ungated non-Mixer startup and DSP loading", () => {
  const name = "crates/studiobridge-desktop/src/main.rs";

  const selector = validSources();
  selector[name] = selector[name].replace(
    'if cfg!(feature = "extended-workspaces") || requested_page == 4 {',
    "if true || requested_page == 4 {",
  );
  expectFailure(selector, "clamp non-Mixer, non-Settings workspaces to Mixer");

  const startup = validSources();
  startup[name] = startup[name].replace(
    "window.set_active_page(product_workspace_page(0));",
    "window.set_active_page(1);",
  );
  expectFailure(startup, "start through the Mixer-only workspace clamp");

  const dsp = validSources();
  dsp[name] = dsp[name].replace(
    '#[cfg(feature = "extended-workspaces")]\n    load_dsp(\n        window.as_weak()',
    "load_dsp(\n        window.as_weak()",
  );
  expectFailure(dsp, "startup DSP loading must remain gated by extended-workspaces");
});

test("rejects desktop comparison constructors and imports outside the feature cfg", () => {
  for (const addition of [
    "fn bypass() { let _ = ComparisonWorker::spawn_simulated(FakeAudioDriver::default(), 1); }",
    "use studiobridge_comparison::ComparisonWorker;",
    '#[cfg(feature = "other")] fn wrong_gate() { let _ = studiobridge_comparison::ComparisonWorker::spawn_simulated(studiobridge_comparison::FakeAudioDriver::default(), 1); }',
    '#[cfg(any(feature = "simulated-comparison", target_os = "linux"))] fn alternative_gate() { let _ = studiobridge_comparison::ComparisonWorker::spawn_simulated(studiobridge_comparison::FakeAudioDriver::default(), 1); }',
  ]) {
    const sources = withSimulatedDesktopFeature();
    sources["crates/studiobridge-desktop/src/main.rs"] += `\n${addition}\n`;
    expectFailure(sources, "outside the simulated-comparison cfg gate");
  }
});

test("rejects raw samples and real integration from cfg-gated desktop comparison code", () => {
  const mutations = [
    ["let samples: Vec<f32> = vec![];", "raw comparison sample ingress"],
    ["let _ = std::fs::read(\"clip.raw\");", "filesystem, network, process"],
    ["pipewire::init();", "real audio, device"],
    ["let _: Option<DaemonClient> = None;", "real driver, audio, device"],
    ["reqwest::blocking::get(\"https://example.invalid\");", "real audio, device"],
    ["let _ = studiobridge_beacn::BeacnDevice::open();", "real audio, device"],
    ["set_link_assignment();", "route or default mutation"],
    ["set_default();", "route or default mutation"],
    ["load_preferences();", "existing filesystem, network, device"],
  ];
  for (const [body, expected] of mutations) {
    const sources = withSimulatedDesktopFeature();
    sources["crates/studiobridge-desktop/src/main.rs"] += `
      #[cfg(feature = "simulated-comparison")]
      fn forbidden_comparison_integration() { ${body} }
    `;
    expectFailure(sources, expected);
  }
});

test("rejects indirect integration helpers and renamed route or default setters", () => {
  for (const [helperBody, helperCall] of [
    ['let _ = std::fs::read("/tmp/audio");', "harmless_name"],
    ["refresh_all(window.as_weak(), client.clone());", "innocent_bridge"],
    ["set_link_assignment();", "metadata_helper"],
  ]) {
    const sources = withSimulatedDesktopFeature();
    sources["crates/studiobridge-desktop/src/main.rs"] += `
      fn ${helperCall}() { ${helperBody} }
      #[cfg(feature = "simulated-comparison")]
      fn comparison_indirect_bypass() { ${helperCall}(); }
    `;
    expectFailure(sources, `calls non-allowlisted helper ${helperCall}`);
  }

  for (const call of [
    "window.set_default_mix(1)",
    "window.update_personal_route(1)",
    "window.reset_bus_target(1)",
    "window.apply_routing_assignment(1)",
  ]) {
    const sources = withSimulatedDesktopFeature();
    sources["crates/studiobridge-desktop/src/main.rs"] += `
      #[cfg(feature = "simulated-comparison")]
      fn comparison_route_bypass(window: &MainWindow) { ${call}; }
    `;
    expectFailure(sources, "route or default mutation from comparison code");
  }
});

test("requires a visible Simulated comparison label and mutation-free comparison callbacks", () => {
  const label = withSimulatedDesktopFeature();
  for (const name of ["crates/studiobridge-desktop/src/main.rs", "crates/studiobridge-desktop/ui/app-window.slint"]) {
    label[name] = label[name].replaceAll("Simulated comparison", "Audio comparison");
  }
  expectFailure(label, "must visibly retain the Simulated comparison label from feature-gated code");

  const callback = withSimulatedDesktopFeature();
  callback["crates/studiobridge-desktop/ui/app-window.slint"] += "\nexport component UnsafeComparison { callback comparison-set-default-route(); }\n";
  expectFailure(callback, "must not mutate routes or defaults");
});

test("rejects packaging and release builds that enable any non-default desktop feature", () => {
  for (const [name, addition] of [
    ["scripts/package-linux.sh", "\ncargo build --release --features simulated-comparison\n"],
    ["scripts/package-extended.sh", "\ncargo build --release --features extended-workspaces\n"],
    ["packaging/arch/PKGBUILD", "\ncargo build --release --all-features\n"],
    [".github/workflows/verify.yml", "\n# release build\n# cargo build --release --features simulated-comparison\n"],
    ["scripts/release-linux.sh", "\ncargo build --release --features=simulated-comparison\n"],
    ["scripts/build-release.ps1", "\ncargo build --release --all-features\n"],
    ["scripts/publish.cmd", "\ncargo build --release --features simulated-comparison\n"],
    [".cargo/config.toml", '\n[alias]\nrelease-build = "build --all-features"\n'],
    ["package.json", '\n{"scripts":{"release":"cargo build --release --features simulated-comparison"}}\n'],
  ]) {
    const sources = withSimulatedDesktopFeature();
    sources[name] = (sources[name] ?? "") + addition;
    expectFailure(sources, "enables a non-default desktop feature in a packaging or release build");
  }
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

test("rejects public comparison sample ingress and non-Fake worker constructors", () => {
  for (const [addition, expected] of [
    ["impl ComparisonWorker { pub fn inject_samples(&self, samples: &[f32]) {} }", "publicly exposes raw samples through inject_samples"],
    ["impl ComparisonWorker { pub fn expose_samples(&self) -> Vec<f32> { vec![] } }", "publicly exposes raw samples through expose_samples"],
    ["pub struct SampleCommand { pub samples: Vec<f32> }", "public type SampleCommand exposes raw samples"],
    ["pub enum AddedCommand { Safe, Samples { values: Vec<f32> } }", "public type AddedCommand exposes raw samples"],
    ["impl ComparisonWorker { pub fn spawn_linux(driver: LinuxDriver) -> Self { todo!() } }", "constructor spawn_linux accepts a non-Fake driver"],
  ]) {
    const sources = validSources();
    sources["crates/studiobridge-comparison/src/worker.rs"] = sources["crates/studiobridge-comparison/src/worker.rs"].replace("#[cfg(test)]", `${addition}\n#[cfg(test)]`);
    expectFailure(sources, expected);
  }
});

test("rejects raw or externally driven simulated worker progression", () => {
  for (const [before, after, expected] of [
    ["fn simulated_audio_tick(&mut self)", "fn simulated_audio_tick(&mut self, input: &[f32])", "must not accept external samples or frame input"],
    ["let silence = [0.0; SIMULATED_FRAMES_PER_TICK];", "let silence = [0.5; SIMULATED_FRAMES_PER_TICK];", "must generate only fixed internal silence"],
    ["self.accept_capture(&silence);", "self.accept_capture(input);", "must consume only its fixed internal silence"],
    ["test_capture_frames: Option<usize>", "test_capture: Option<Box<[f32]>>", "must not accept or preload raw samples"],
    ["Self::spawn_inner(driver, endpoint_generation, None, None, None)", "Self::spawn_inner(driver, endpoint_generation, None, None, Some(4800))", "must not preload frames or test hooks"],
    ["core.simulated_audio_tick();", "core.accept_capture(&[0.0; 4800]);", "must not expose a raw capture preload path"],
  ]) {
    const sources = validSources();
    const name = "crates/studiobridge-comparison/src/worker.rs";
    assert(sources[name].includes(before), `mutation fixture missing: ${before}`);
    sources[name] = sources[name].replace(before, after);
    expectFailure(sources, expected);
  }
});

test("rejects non-simulated comparison mode or label weakening", () => {
  const mode = validSources();
  mode["crates/studiobridge-comparison/src/worker.rs"] = mode["crates/studiobridge-comparison/src/worker.rs"].replace(
    "pub enum ComparisonMode {\n    Simulated,",
    "pub enum ComparisonMode {\n    Simulated,\n    Production,",
  );
  expectFailure(mode, "must not expose a non-simulated mode");

  const label = validSources();
  label["crates/studiobridge-comparison/src/worker.rs"] = label["crates/studiobridge-comparison/src/worker.rs"].replace(
    'availability_label: "Simulated comparison"',
    'availability_label: "Comparison available"',
  );
  expectFailure(label, "must retain the Simulated comparison label");
});

test("rejects bypassing destination-generation policy before playback dispatch", () => {
  for (const [before, after, expected] of [
    ["if !self.authorize_playback(destination_acknowledgement) {", "if false {", "must authorize destination generation before playback dispatch"],
    ["if !snapshot.complete {", "if false {", "must fail closed on incomplete or inaudible snapshots"],
    ["acknowledgement.snapshot == current", "true", "must reject stale destination generations and content"],
    ["acknowledgement.confirmation_id,", "self.next_confirmation_id,", "must bind acknowledgements to a pending confirmation ID"],
    ["TerminalEvent::DestinationConfirmationRequired", "TerminalEvent::Diagnostic", "must request confirmation without dispatching playback"],
  ]) {
    const sources = validSources();
    sources["crates/studiobridge-comparison/src/worker.rs"] = sources["crates/studiobridge-comparison/src/worker.rs"].replace(before, after);
    expectFailure(sources, expected);
  }
});

test("rejects teardown and semantic snapshot regressions", () => {
  for (const [before, after, expected] of [
    ["let _ = self.owner_teardown.send(());", "let _ = &self.owner_teardown;", "must use the out-of-band teardown path"],
    ["self.thread\n            .take()", "self.thread\n            .as_mut()", "must take and join the worker thread"],
    ["handle.join().is_ok()", "true", "must take and join the worker thread"],
    ["self.terminal.send(TerminalEvent::Snapshot(self.snapshot()))", "self.latest_timer.publish(self.snapshot())", "semantic snapshots must use the lossless terminal queue"],
    ["if changed || confirmation_resolved {\n            self.publish_semantic_snapshot();", "if changed {\n            self.publish_semantic_snapshot();", "must publish authoritative snapshots after fail-closed transitions and confirmation cancellation"],
    ["self.engine.accept_capture(token, input);", "self.engine.accept_capture(token, input); return;", "capture completion must publish its semantic transition"],
    ["self.engine.shutdown();", "self.engine.driver().active_stream_count();", "forced shutdown must scrub and deactivate"],
  ]) {
    const sources = validSources();
    sources["crates/studiobridge-comparison/src/worker.rs"] = sources["crates/studiobridge-comparison/src/worker.rs"].replace(before, after);
    expectFailure(sources, expected);
  }
});
