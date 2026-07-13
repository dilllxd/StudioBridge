import assert from "node:assert/strict";
import test from "node:test";
import { micChainMarkup } from "./mic-chain.js";

const profile = (mode) => ({
  mode,
  bands: Array.from({ length: 8 }, (_, index) => ({
    band: index + 1,
    band_type: index === 0 ? "high_pass" : "bell",
    gain_db: index - 3,
    frequency_hz: 80 * (index + 1),
    q: 1.25,
    enabled: true,
  })),
});

const dynamics = (mode) => ({
  mode,
  enabled: true,
  threshold_db: -20,
  ratio: 4,
  attack_ms: 10,
  release_ms: 100,
  makeup_gain_db: 2,
});

const dsp = {
  equalizer: { active_mode: "advanced", simple: profile("simple"), advanced: profile("advanced") },
  compressor: { active_mode: "simple", simple: dynamics("simple"), advanced: dynamics("advanced") },
  expander: { active_mode: "advanced", simple: dynamics("simple"), advanced: dynamics("advanced") },
  noise_suppression: { enabled: true, style: "adaptive", amount_percent: 40, sensitivity_db: -90, adapt_time_ms: 1000 },
  enhancement_suite: {
    bass: { enabled: false, preset: 1, amount: 2, drive: 4, mix_percent: 30 },
    de_esser: { enabled: true, amount_percent: 25 },
    exciter: { enabled: false, amount_percent: 10, frequency_hz: 2500 },
  },
  headphone_equalizer: {
    bands: ["bass", "mids", "treble"].map((band) => ({ band, enabled: true, amount_db: 1 })),
    subwoofer: { enabled: false, amount: 0 },
  },
};

const snapshot = {
  studio: { microphone: { gain_db: 45, phantom_power: false } },
  mixer: { channels: [{ name: "Microphone", meter_id: "mic-meter" }] },
};

function render(activeModule, viewModes = {}, options = {}) {
  return micChainMarkup({
    snapshot,
    dsp,
    captured: structuredClone(dsp),
    loading: false,
    error: null,
    activeModule,
    viewModes,
    draft: options.draft,
    saving: false,
    writeModules: options.writeModules || [],
  });
}

test("renders the complete microphone module navigation and safety state", () => {
  const markup = render("setup");
  for (const label of ["Mic setup", "Noise suppression", "Equalizer", "Compressor", "Expander / gate", "Enhancement suite", "Headphone EQ"]) {
    assert.match(markup, new RegExp(label.replace("/", "\\/")));
  }
  assert.match(markup, /General hardware writes disabled/);
  assert.match(markup, /Phantom power is never changed automatically/);
  assert.match(markup, /data-meter-id="mic-meter"/);
});

test("only exposes controls and apply actions for an explicitly armed module", () => {
  const readOnly = render("headphones");
  assert.doesNotMatch(readOnly, /data-action="dsp-apply"/);
  assert.match(readOnly, /data-action="dsp-control"[^>]+disabled/);

  const draft = structuredClone(dsp);
  draft.headphone_equalizer.bands[0].amount_db = 1.5;
  const armed = render("headphones", {}, {
    draft,
    writeModules: ["headphone_equalizer"],
  });
  assert.match(armed, /Headphone Equalizer gate armed/);
  assert.match(armed, /data-action="dsp-apply"/);
  assert.match(armed, /Unsaved module changes/);
});

test("renders all eight bands from the selected EQ profile", () => {
  const markup = render("equalizer", { equalizer: "advanced" });
  assert.match(markup, /Eight-band parametric EQ/);
  assert.match(markup, /Advanced captured profile/);
  assert.equal((markup.match(/<circle /g) || []).length, 8);
  assert.equal((markup.match(/class="module-state is-enabled"/g) || []).length, 8);
  assert.match(markup, /points="[^\"]+"/);
});

test("renders all three enhancement modules and headphone bands", () => {
  const enhancements = render("enhancement");
  assert.match(enhancements, /Bass enhancement/);
  assert.match(enhancements, /De-esser/);
  assert.match(enhancements, /Exciter/);

  const headphones = render("headphones");
  assert.match(headphones, />Bass</);
  assert.match(headphones, />Mids</);
  assert.match(headphones, />Treble</);
  assert.match(headphones, />Subwoofer</);
  assert.match(headphones, /headphone_equalizer\.subwoofer\.amount/);
});
