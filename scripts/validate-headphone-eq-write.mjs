#!/usr/bin/env node

const baseUrl = process.env.STUDIOBRIDGE_URL || "http://127.0.0.1:17840";
const acknowledgement = "I_UNDERSTAND_THIS_CHANGES_HEADPHONE_EQ";

if (process.env.STUDIOBRIDGE_VALIDATE_DSP_WRITE !== acknowledgement) {
  console.error(`Refusing to write. Set STUDIOBRIDGE_VALIDATE_DSP_WRITE=${acknowledgement}`);
  process.exit(2);
}

async function json(path, options = {}) {
  const response = await fetch(`${baseUrl}${path}`, {
    headers: { "content-type": "application/json" },
    ...options,
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.error || `request failed (${response.status})`);
  return body;
}

const postHeadphoneEq = (state) => json("/api/studio/microphone-dsp", {
  method: "POST",
  body: JSON.stringify({ module: "headphone_equalizer", state }),
});

const health = await json("/api/health");
if (health.hardware_writes_enabled !== false
  || health.link_control_enabled !== true
  || JSON.stringify(health.dsp_write_modules) !== JSON.stringify(["headphone_equalizer"])) {
  throw new Error(`unsafe runtime gate state: ${JSON.stringify(health)}`);
}

const baseline = (await json("/api/studio/microphone-dsp")).headphone_equalizer;
const candidate = structuredClone(baseline);
const bass = candidate.bands.find((band) => band.band === "bass");
if (!bass) throw new Error("captured headphone EQ has no bass band");
const direction = bass.amount_db <= 11.8 ? 0.1 : -0.1;
bass.amount_db = Math.round((bass.amount_db + direction) * 10) / 10;

let writeAttempted = false;
try {
  writeAttempted = true;
  const result = await postHeadphoneEq(candidate);
  if (result.verified !== true) throw new Error("candidate write was not verified");
  const observed = (await json("/api/studio/microphone-dsp")).headphone_equalizer;
  if (JSON.stringify(observed) !== JSON.stringify(candidate)) {
    throw new Error("candidate state did not survive an independent read-back");
  }
  console.log(`PASS: reversible Headphone EQ probe verified (${direction > 0 ? "+" : ""}${direction.toFixed(1)} dB)`);
} finally {
  if (writeAttempted) {
    const restored = await postHeadphoneEq(baseline);
    if (restored.verified !== true) throw new Error("baseline restore was not verified");
    const observed = (await json("/api/studio/microphone-dsp")).headphone_equalizer;
    if (JSON.stringify(observed) !== JSON.stringify(baseline)) {
      throw new Error("captured Headphone EQ baseline was not restored exactly");
    }
    console.log("PASS: captured Headphone EQ baseline restored exactly");
  }
}
