import test from "node:test";
import assert from "node:assert/strict";
import { validateMicrophoneDsp } from "./validate-microphone-dsp.mjs";

function fixture() {
  const eqProfile = (mode) => ({
    mode,
    bands: Array.from({ length: 8 }, (_, index) => ({
      band: index + 1,
      band_type: "bell",
      gain_db: 0,
      frequency_hz: 80 * 2 ** index,
      q: 1,
      enabled: true,
    })),
  });
  const compressor = (mode) => ({ mode, enabled: true, threshold_db: -18, ratio: 3, attack_ms: 10, release_ms: 120, makeup_gain_db: 3 });
  const expander = (mode) => ({ mode, enabled: true, threshold_db: -50, ratio: 2, attack_ms: 10, release_ms: 180 });
  return {
    health: { studio_mode: "beacn", hardware_writes_enabled: false },
    dsp: {
      equalizer: { active_mode: "advanced", simple: eqProfile("simple"), advanced: eqProfile("advanced") },
      compressor: { active_mode: "advanced", simple: compressor("simple"), advanced: compressor("advanced") },
      expander: { active_mode: "advanced", simple: expander("simple"), advanced: expander("advanced") },
      noise_suppression: { enabled: true, style: "adaptive", amount_percent: 70, sensitivity_db: -85, adapt_time_ms: 1000 },
      enhancement_suite: {
        bass: { enabled: false, preset: 1 },
        de_esser: { enabled: true, amount_percent: 35 },
        exciter: { enabled: false, amount_percent: 0, frequency_hz: 3000 },
      },
      headphone_equalizer: { bands: ["bass", "mids", "treble"].map((band) => ({ band, enabled: true, amount_db: 0 })) },
    },
  };
}

test("accepts a complete DSP snapshot", () => {
  const value = fixture();
  assert.deepEqual(validateMicrophoneDsp(value.health, value.dsp).failures, []);
});

test("rejects incomplete EQ and enabled general writes", () => {
  const value = fixture();
  value.health.hardware_writes_enabled = true;
  value.dsp.equalizer.advanced.bands.pop();
  const result = validateMicrophoneDsp(value.health, value.dsp);
  assert(result.failures.some((failure) => failure.includes("general hardware writes")));
  assert(result.failures.some((failure) => failure.includes("eight bands")));
});
