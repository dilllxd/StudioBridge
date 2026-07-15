import fs from "node:fs";
import { pathToFileURL } from "node:url";

export function validateMicrophoneDsp(health, dsp) {
  const failures = [];
  const check = (condition, message) => {
    if (!condition) failures.push(message);
  };
  const numberIn = (value, min, max) => Number.isFinite(value) && value >= min && value <= max;

  check(health.hardware_writes_enabled === false, "general hardware writes are enabled");
  check(health.studio_mode === "beacn", "daemon is not using the real BEACN backend");

  const validateEqProfile = (profile, name) => {
    check(profile?.mode === name, `${name} EQ profile mode is invalid`);
    check(Array.isArray(profile?.bands) && profile.bands.length === 8, `${name} EQ does not expose eight bands`);
    check(new Set((profile?.bands ?? []).map((band) => band.band)).size === 8, `${name} EQ band identifiers are not unique`);
    for (const band of profile?.bands ?? []) {
      check(Number.isInteger(band.band) && band.band >= 1 && band.band <= 8, `${name} EQ band number is invalid`);
      check(["not_set", "low_pass", "high_pass", "notch", "bell", "low_shelf", "high_shelf"].includes(band.band_type), `${name} EQ band ${band.band} has an invalid type`);
      check(numberIn(band.gain_db, -12, 12), `${name} EQ band ${band.band} gain is out of range`);
      check(numberIn(band.frequency_hz, 20, 20_000), `${name} EQ band ${band.band} frequency is out of range`);
      check(numberIn(band.q, -0.1, 10), `${name} EQ band ${band.band} Q is out of range`);
      check(typeof band.enabled === "boolean", `${name} EQ band ${band.band} enabled state is invalid`);
    }
  };
  validateEqProfile(dsp.equalizer?.simple, "simple");
  validateEqProfile(dsp.equalizer?.advanced, "advanced");
  check(["simple", "advanced"].includes(dsp.equalizer?.active_mode), "active EQ mode is invalid");

  const validateCompressor = (profile, name) => {
    check(profile?.mode === name, `${name} compressor mode is invalid`);
    check(typeof profile?.enabled === "boolean", `${name} compressor enabled state is invalid`);
    check(numberIn(profile?.threshold_db, -50, 0), `${name} compressor threshold is out of range`);
    check(numberIn(profile?.ratio, 1, 16), `${name} compressor ratio is out of range`);
    check(numberIn(profile?.attack_ms, 1, 2_000), `${name} compressor attack is out of range`);
    check(numberIn(profile?.release_ms, 1, 2_000), `${name} compressor release is out of range`);
    check(numberIn(profile?.makeup_gain_db, 0, 12), `${name} compressor makeup gain is out of range`);
  };
  validateCompressor(dsp.compressor?.simple, "simple");
  validateCompressor(dsp.compressor?.advanced, "advanced");

  const validateExpander = (profile, name) => {
    check(profile?.mode === name, `${name} expander mode is invalid`);
    check(typeof profile?.enabled === "boolean", `${name} expander enabled state is invalid`);
    check(numberIn(profile?.threshold_db, -90, 0), `${name} expander threshold is out of range`);
    check(numberIn(profile?.ratio, 1, 10), `${name} expander ratio is out of range`);
    check(numberIn(profile?.attack_ms, 1, 2_000), `${name} expander attack is out of range`);
    check(numberIn(profile?.release_ms, 1, 2_000), `${name} expander release is out of range`);
  };
  validateExpander(dsp.expander?.simple, "simple");
  validateExpander(dsp.expander?.advanced, "advanced");

  const noise = dsp.noise_suppression ?? {};
  check(typeof noise.enabled === "boolean", "noise suppression enabled state is invalid");
  check(["off", "adaptive", "snapshot"].includes(noise.style), "noise suppression style is invalid");
  check(numberIn(noise.amount_percent, 0, 100), "noise suppression amount is out of range");
  check(numberIn(noise.sensitivity_db, -120, -60), "noise suppression sensitivity is out of range");
  check(numberIn(noise.adapt_time_ms, 100, 5_000), "noise suppression adaptation time is out of range");

  const enhancement = dsp.enhancement_suite ?? {};
  check(typeof enhancement.bass?.enabled === "boolean", "bass enhancement enabled state is invalid");
  check(Number.isInteger(enhancement.bass?.preset) && enhancement.bass.preset >= 1 && enhancement.bass.preset <= 4, "bass enhancement preset is invalid");
  check(numberIn(enhancement.de_esser?.amount_percent, 0, 100), "de-esser amount is out of range");
  check(typeof enhancement.de_esser?.enabled === "boolean", "de-esser enabled state is invalid");
  check(numberIn(enhancement.exciter?.amount_percent, 0, 100), "exciter amount is out of range");
  check(numberIn(enhancement.exciter?.frequency_hz, 0, 5_000), "exciter frequency is out of range");
  check(typeof enhancement.exciter?.enabled === "boolean", "exciter enabled state is invalid");

  const headphoneBands = dsp.headphone_equalizer?.bands ?? [];
  check(Array.isArray(headphoneBands) && headphoneBands.length === 3, "headphone EQ does not expose three bands");
  check(new Set(headphoneBands.map((band) => band.band)).size === 3, "headphone EQ bands are not unique");
  for (const band of headphoneBands) {
    check(["bass", "mids", "treble"].includes(band.band), "headphone EQ band is invalid");
    check(typeof band.enabled === "boolean", `${band.band} headphone EQ enabled state is invalid`);
    check(numberIn(band.amount_db, -12, 12), `${band.band} headphone EQ amount is out of range`);
  }
  const subwoofer = dsp.headphone_equalizer?.subwoofer ?? {};
  check(typeof subwoofer.enabled === "boolean", "subwoofer enabled state is invalid");
  check(Number.isInteger(subwoofer.amount) && subwoofer.amount >= 0 && subwoofer.amount <= 10, "subwoofer amount is out of range");

  return {
    failures,
    summary: {
      eqBands: (dsp.equalizer?.simple?.bands?.length ?? 0) + (dsp.equalizer?.advanced?.bands?.length ?? 0),
      compressorProfiles: [dsp.compressor?.simple, dsp.compressor?.advanced].filter(Boolean).length,
      expanderProfiles: [dsp.expander?.simple, dsp.expander?.advanced].filter(Boolean).length,
      enhancementModules: [enhancement.bass, enhancement.de_esser, enhancement.exciter].filter(Boolean).length,
      headphoneEqBands: headphoneBands.length,
      subwooferAmount: subwoofer.amount,
    },
  };
}

function printResult(result) {
  console.log("StudioBridge microphone DSP read-only validation");
  console.log("  General hardware writes: disabled");
  console.log(`  Equalizer states: ${result.summary.eqBands} bands across Simple/Advanced`);
  console.log(`  Dynamics: ${result.summary.compressorProfiles} compressor + ${result.summary.expanderProfiles} expander profiles`);
  console.log(`  Enhancement modules: ${result.summary.enhancementModules}`);
  console.log(`  Headphone EQ bands: ${result.summary.headphoneEqBands}`);
  console.log(`  Subwoofer amount: ${result.summary.subwooferAmount}`);
  for (const failure of result.failures) console.error(`  FAIL: ${failure}`);
  if (result.failures.length) return 1;
  console.log("  PASS: complete DSP state is readable and internally valid");
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const health = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
  const dsp = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
  process.exitCode = printResult(validateMicrophoneDsp(health, dsp));
}
