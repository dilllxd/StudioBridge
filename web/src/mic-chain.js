import { escapeHtml } from "./escape.js";

const MODULES = [
  ["setup", "Mic setup"],
  ["noise", "Noise suppression"],
  ["equalizer", "Equalizer"],
  ["compressor", "Compressor"],
  ["expander", "Expander / gate"],
  ["enhancement", "Enhancement suite"],
  ["headphones", "Headphone EQ"],
];

const number = (value, digits = 1) => Number(value).toFixed(digits).replace(/\.0$/, "");
const title = (value) => String(value).replaceAll("_", " ").replace(/\b\w/g, (letter) => letter.toUpperCase());

function readonlyControl(label, value, unit, min, max, step = 1, options = {}) {
  const { writable = false, path = "" } = options;
  return `
    <label class="dsp-control">
      <span>${escapeHtml(label)}</span><output>${escapeHtml(number(value))}${unit}</output>
      <input type="range" min="${min}" max="${max}" step="${step}" value="${value}" data-action="dsp-control" data-dsp-path="${escapeHtml(path)}" data-unit="${escapeHtml(unit)}" ${writable ? "" : "disabled"} />
      <small><span>${min}${unit}</span><span>${max}${unit}</span></small>
    </label>`;
}

function enabledBadge(enabled, options = {}) {
  const { writable = false, path = "" } = options;
  if (writable) {
    return `<button type="button" class="module-state is-toggle ${enabled ? "is-enabled" : ""}" data-action="dsp-toggle" data-dsp-path="${escapeHtml(path)}" data-enabled="${enabled}" aria-pressed="${enabled}">${enabled ? "Enabled" : "Bypassed"}</button>`;
  }
  return `<span class="module-state ${enabled ? "is-enabled" : ""}">${enabled ? "Enabled" : "Bypassed"}</span>`;
}

function modePicker(module, activeMode, viewedMode, writable) {
  return `
    <div class="dsp-mode-picker" aria-label="${title(module)} profile">
      ${["simple", "advanced"].map((mode) => `<button type="button" data-action="dsp-mode-view" data-dsp-module="${module}" data-dsp-mode="${mode}" class="${viewedMode === mode ? "is-active" : ""}">${title(mode)}${activeMode === mode ? " | active" : ""}</button>`).join("")}
      ${writable && activeMode !== viewedMode ? `<button type="button" class="set-active-mode" data-action="dsp-set-active-mode" data-dsp-module="${module}" data-dsp-mode="${viewedMode}">Make active</button>` : ""}
    </div>`;
}

function dspSelect(label, value, values, path, writable, valueType = "string") {
  return `<label class="dsp-select"><span>${escapeHtml(label)}</span><select data-action="dsp-select" data-dsp-path="${escapeHtml(path)}" data-value-type="${valueType}" ${writable ? "" : "disabled"}>${values.map(([option, optionLabel]) => `<option value="${escapeHtml(option)}" ${String(option) === String(value) ? "selected" : ""}>${escapeHtml(optionLabel)}</option>`).join("")}</select></label>`;
}

function setupMarkup(snapshot) {
  return `
    <section class="dsp-module-grid two-column">
      <article class="dsp-card">
        <header><div><h2>Input setup</h2><p>Physical XLR preamp state</p></div>${enabledBadge(true)}</header>
        ${readonlyControl("Microphone gain", snapshot.studio.microphone.gain_db, " dB", 0, 69)}
        <div class="dsp-fact"><span>Phantom power</span><strong>${snapshot.studio.microphone.phantom_power ? "48V enabled" : "Off"}</strong></div>
        <p class="dsp-warning">Phantom power is never changed automatically.</p>
      </article>
      <article class="dsp-card">
        <header><div><h2>Signal check</h2><p>Live PipeWeaver microphone level</p></div></header>
        <div class="mic-live-meter" data-meter-id="${escapeHtml(snapshot.micMeterId || "")}" data-volume="100"><b></b></div>
        <div class="meter-scale"><span>-60</span><span>-25</span><span>-10</span><span>0 dB</span></div>
        <p>Speak at normal streaming volume. Gain changes remain locked during the read-only phase.</p>
      </article>
    </section>`;
}

function noiseMarkup(dsp, writable) {
  const noise = dsp.noise_suppression;
  return `
    <section class="dsp-module-grid two-column">
      <article class="dsp-card">
        <header><div><h2>${title(noise.style)} suppression</h2><p>Constant room-noise reduction</p></div>${enabledBadge(noise.enabled, { writable, path: "noise_suppression.enabled" })}</header>
        ${dspSelect("Style", noise.style, [["off", "Off"], ["adaptive", "Adaptive"], ["snapshot", "Snapshot"]], "noise_suppression.style", writable)}
        ${readonlyControl("Amount", noise.amount_percent, "%", 0, 100, 1, { writable, path: "noise_suppression.amount_percent" })}
        ${readonlyControl("Sensitivity", noise.sensitivity_db, " dB", -120, -60, 1, { writable, path: "noise_suppression.sensitivity_db" })}
        ${readonlyControl("Adaptation time", noise.adapt_time_ms, " ms", 100, 5000, 100, { writable, path: "noise_suppression.adapt_time_ms" })}
      </article>
      <article class="dsp-card guidance-card"><h2>Captured behavior</h2><dl><div><dt>Style</dt><dd>${title(noise.style)}</dd></div><div><dt>Snapshot action</dt><dd>Locked until explicit write validation</dd></div><div><dt>Recommended use</dt><dd>Fans and consistent room noise</dd></div></dl></article>
    </section>`;
}

function eqGraph(profile) {
  const graphBands = [...profile.bands].sort((left, right) => left.frequency_hz - right.frequency_hz);
  const coordinates = graphBands.map((band) => {
    const x = 30 + ((Math.log10(band.frequency_hz) - Math.log10(20)) / (Math.log10(20000) - Math.log10(20))) * 740;
    const y = 130 - (band.gain_db / 12) * 95;
    return { band, x: number(x), y: number(y) };
  });
  const points = coordinates.map(({ x, y }) => `${x},${y}`).join(" ");
  return `
    <svg class="eq-graph" viewBox="0 0 800 260" role="img" aria-label="Captured equalizer curve">
      <g class="eq-grid"><path d="M30 35H770 M30 130H770 M30 225H770"/><path d="M30 35V225 M277 35V225 M523 35V225 M770 35V225"/></g>
      <polyline points="${points}" />
      ${coordinates.map(({ band, x, y }) => {
        return `<circle cx="${x}" cy="${y}" r="7"><title>Band ${band.band}: ${number(band.frequency_hz)} Hz, ${number(band.gain_db)} dB</title></circle>`;
      }).join("")}
      <g class="eq-labels"><text x="30" y="248">20 Hz</text><text x="265" y="248">200 Hz</text><text x="510" y="248">2 kHz</text><text x="735" y="248">20 kHz</text></g>
    </svg>`;
}

function equalizerMarkup(dsp, viewedMode, writable) {
  const equalizer = dsp.equalizer;
  const profile = equalizer[viewedMode];
  return `
    ${modePicker("equalizer", equalizer.active_mode, viewedMode, writable)}
    <section class="dsp-module-grid">
      <article class="dsp-card eq-card">
        <header><div><h2>Eight-band parametric EQ</h2><p>${title(viewedMode)} captured profile</p></div></header>
        ${eqGraph(profile)}
        <div class="eq-band-table">
          <div class="eq-band-head"><span>Band</span><span>Type</span><span>Frequency</span><span>Gain</span><span>Q</span><span>State</span></div>
          ${profile.bands.map((band, index) => {
            const base = `equalizer.${viewedMode}.bands.${index}`;
            return `<div><strong>${band.band}</strong>${dspSelect("", band.band_type, [["not_set", "Not set"], ["low_pass", "Low pass"], ["high_pass", "High pass"], ["notch", "Notch"], ["bell", "Bell"], ["low_shelf", "Low shelf"], ["high_shelf", "High shelf"]], `${base}.band_type`, writable)}<label><span class="sr-only">Band ${band.band} frequency</span><input class="eq-number" type="number" min="20" max="20000" step="1" value="${band.frequency_hz}" data-action="dsp-control" data-dsp-path="${base}.frequency_hz" ${writable ? "" : "disabled"}> Hz</label><label><span class="sr-only">Band ${band.band} gain</span><input class="eq-number" type="number" min="-12" max="12" step="0.1" value="${band.gain_db}" data-action="dsp-control" data-dsp-path="${base}.gain_db" ${writable ? "" : "disabled"}> dB</label><label><span class="sr-only">Band ${band.band} Q</span><input class="eq-number" type="number" min="0.1" max="10" step="0.1" value="${band.q}" data-action="dsp-control" data-dsp-path="${base}.q" ${writable ? "" : "disabled"}></label>${enabledBadge(band.enabled, { writable, path: `${base}.enabled` })}</div>`;
          }).join("")}
        </div>
      </article>
    </section>`;
}

function compressorMarkup(dsp, viewedMode, writable) {
  const compressor = dsp.compressor;
  const profile = compressor[viewedMode];
  return `
    ${modePicker("compressor", compressor.active_mode, viewedMode, writable)}
    <section class="dsp-module-grid two-column">
      <article class="dsp-card">
        <header><div><h2>Compressor</h2><p>${title(viewedMode)} profile</p></div>${enabledBadge(profile.enabled, { writable, path: `compressor.${viewedMode}.enabled` })}</header>
        ${readonlyControl("Threshold", profile.threshold_db, " dB", -50, 0, 0.1, { writable, path: `compressor.${viewedMode}.threshold_db` })}
        ${readonlyControl("Ratio", profile.ratio, ":1", 1, 16, 0.1, { writable, path: `compressor.${viewedMode}.ratio` })}
        ${readonlyControl("Make-up gain", profile.makeup_gain_db, " dB", 0, 12, 0.1, { writable, path: `compressor.${viewedMode}.makeup_gain_db` })}
      </article>
      <article class="dsp-card">
        <header><div><h2>Timing</h2><p>How compression responds and recovers</p></div></header>
        ${readonlyControl("Attack", profile.attack_ms, " ms", 1, 2000, 1, { writable, path: `compressor.${viewedMode}.attack_ms` })}
        ${readonlyControl("Release", profile.release_ms, " ms", 1, 2000, 1, { writable, path: `compressor.${viewedMode}.release_ms` })}
      </article>
    </section>`;
}

function expanderMarkup(dsp, viewedMode, writable) {
  const expander = dsp.expander;
  const profile = expander[viewedMode];
  return `
    ${modePicker("expander", expander.active_mode, viewedMode, writable)}
    <section class="dsp-module-grid two-column">
      <article class="dsp-card">
        <header><div><h2>Downward expander</h2><p>${title(viewedMode)} profile</p></div>${enabledBadge(profile.enabled, { writable, path: `expander.${viewedMode}.enabled` })}</header>
        ${readonlyControl("Threshold", profile.threshold_db, " dB", -90, 0, 0.1, { writable, path: `expander.${viewedMode}.threshold_db` })}
        ${readonlyControl("Ratio", profile.ratio, ":1", 1, 10, 0.1, { writable, path: `expander.${viewedMode}.ratio` })}
      </article>
      <article class="dsp-card">
        <header><div><h2>Gate timing</h2><p>Transient response below the threshold</p></div></header>
        ${readonlyControl("Attack", profile.attack_ms, " ms", 1, 2000, 1, { writable, path: `expander.${viewedMode}.attack_ms` })}
        ${readonlyControl("Release", profile.release_ms, " ms", 1, 2000, 1, { writable, path: `expander.${viewedMode}.release_ms` })}
      </article>
    </section>`;
}

function enhancementMarkup(dsp, writable) {
  const { bass, de_esser: deEsser, exciter } = dsp.enhancement_suite;
  return `
    <section class="dsp-module-grid three-column">
      <article class="dsp-card"><header><div><h2>Bass enhancement</h2><p>Harmonic low-frequency weight</p></div>${enabledBadge(bass.enabled, { writable, path: "enhancement_suite.bass.enabled" })}</header>${dspSelect("Preset", bass.preset, [[1, "Preset 1"], [2, "Preset 2"], [3, "Preset 3"], [4, "Preset 4"]], "enhancement_suite.bass.preset", writable, "number")}${readonlyControl("Amount", bass.amount, "", 0, 10, 0.1, { writable, path: "enhancement_suite.bass.amount" })}${readonlyControl("Drive", bass.drive, "", 0, 32, 0.1, { writable, path: "enhancement_suite.bass.drive" })}${readonlyControl("Mix", bass.mix_percent, "%", 0, 100, 1, { writable, path: "enhancement_suite.bass.mix_percent" })}</article>
      <article class="dsp-card"><header><div><h2>De-esser</h2><p>Sibilance control</p></div>${enabledBadge(deEsser.enabled, { writable, path: "enhancement_suite.de_esser.enabled" })}</header>${readonlyControl("Amount", deEsser.amount_percent, "%", 0, 100, 1, { writable, path: "enhancement_suite.de_esser.amount_percent" })}</article>
      <article class="dsp-card"><header><div><h2>Exciter</h2><p>High-frequency presence</p></div>${enabledBadge(exciter.enabled, { writable, path: "enhancement_suite.exciter.enabled" })}</header>${readonlyControl("Amount", exciter.amount_percent, "%", 0, 100, 1, { writable, path: "enhancement_suite.exciter.amount_percent" })}${readonlyControl("Frequency", exciter.frequency_hz, " Hz", 0, 5000, 10, { writable, path: "enhancement_suite.exciter.frequency_hz" })}</article>
      <article class="dsp-card bass-advanced"><header><div><h2>Bass dynamics</h2><p>Complete captured engine parameters</p></div></header>${readonlyControl("Attack", bass.attack_ms, " ms", 1, 2000, 1, { writable, path: "enhancement_suite.bass.attack_ms" })}${readonlyControl("Release", bass.release_ms, " ms", 1, 2000, 1, { writable, path: "enhancement_suite.bass.release_ms" })}${readonlyControl("Threshold", bass.threshold_db, " dB", -50, 0, 0.1, { writable, path: "enhancement_suite.bass.threshold_db" })}${readonlyControl("Knee", bass.knee, "", 0, 5, 0.1, { writable, path: "enhancement_suite.bass.knee" })}${readonlyControl("Make-up gain", bass.makeup_gain_db, " dB", 0, 12, 0.1, { writable, path: "enhancement_suite.bass.makeup_gain_db" })}${readonlyControl("Ratio", bass.ratio, ":1", 0, 16, 0.1, { writable, path: "enhancement_suite.bass.ratio" })}</article>
      <article class="dsp-card bass-advanced"><header><div><h2>Bass filters</h2><p>Upper and lower filter geometry</p></div></header>${readonlyControl("Cutoff", bass.cutoff_hz, " Hz", 0, 160, 1, { writable, path: "enhancement_suite.bass.cutoff_hz" })}${readonlyControl("Q", bass.q, "", 0, 16, 0.1, { writable, path: "enhancement_suite.bass.q" })}${readonlyControl("Lower cutoff", bass.lower_cutoff_hz, " Hz", 0, 160, 1, { writable, path: "enhancement_suite.bass.lower_cutoff_hz" })}${readonlyControl("Lower Q", bass.lower_q, "", 0, 16, 0.1, { writable, path: "enhancement_suite.bass.lower_q" })}</article>
    </section>`;
}

function headphoneMarkup(dsp, writable) {
  return `
    <section class="dsp-module-grid three-column">
      ${dsp.headphone_equalizer.bands.map((band, index) => `<article class="dsp-card"><header><div><h2>${title(band.band)}</h2><p>Headphone playback EQ</p></div>${enabledBadge(band.enabled, { writable, path: `headphone_equalizer.bands.${index}.enabled` })}</header>${readonlyControl("Amount", band.amount_db, " dB", -12, 12, 0.1, { writable, path: `headphone_equalizer.bands.${index}.amount_db` })}</article>`).join("")}
    </section>`;
}

export function micChainMarkup({ snapshot, dsp, captured, draft, loading, saving, error, activeModule, viewModes, writeModules = [] }) {
  const micChannel = snapshot.mixer.channels.find((channel) => /mic/i.test(channel.name));
  const matched = Boolean(dsp && captured && JSON.stringify(dsp) === JSON.stringify(captured));
  const moduleNames = { noise: "noise_suppression", equalizer: "equalizer", compressor: "compressor", expander: "expander", enhancement: "enhancement_suite", headphones: "headphone_equalizer" };
  const writeModule = moduleNames[activeModule];
  const writable = Boolean(writeModule && writeModules.includes(writeModule));
  const displayDsp = draft || dsp;
  const dirty = Boolean(writeModule && dsp && displayDsp && JSON.stringify(dsp[writeModule]) !== JSON.stringify(displayDsp[writeModule]));
  let content;
  if (loading) content = `<div class="dsp-loading"><span></span><strong>Reading the complete Studio DSP chain...</strong><p>This is an explicit on-demand USB snapshot.</p></div>`;
  else if (error) content = `<div class="dsp-error"><strong>DSP snapshot unavailable</strong><p>${escapeHtml(error)}</p><button type="button" data-action="dsp-refresh">Try again</button></div>`;
  else if (!dsp) content = `<div class="dsp-loading"><strong>Mic Chain has not been captured yet.</strong></div>`;
  else {
    const snapshotWithMeter = { ...snapshot, micMeterId: micChannel?.meter_id };
    const viewed = {
      equalizer: viewModes.equalizer || displayDsp.equalizer.active_mode,
      compressor: viewModes.compressor || displayDsp.compressor.active_mode,
      expander: viewModes.expander || displayDsp.expander.active_mode,
    };
    content = activeModule === "setup" ? setupMarkup(snapshotWithMeter)
      : activeModule === "noise" ? noiseMarkup(displayDsp, writable)
        : activeModule === "equalizer" ? equalizerMarkup(displayDsp, viewed.equalizer, writable)
          : activeModule === "compressor" ? compressorMarkup(displayDsp, viewed.compressor, writable)
            : activeModule === "expander" ? expanderMarkup(displayDsp, viewed.expander, writable)
              : activeModule === "enhancement" ? enhancementMarkup(displayDsp, writable)
                : headphoneMarkup(displayDsp, writable);
  }

  const captureLabel = dirty ? "Unsaved module changes" : matched ? "Captured state matches" : "Verified device change active";

  return `
    <main class="mic-chain-deck main-view">
      <header class="mic-chain-head">
        <div><span class="device-dot"></span><div><h1>Microphone chain</h1><p>BEACN Studio onboard DSP</p></div></div>
        <div class="capture-status ${matched && !dirty ? "is-matched" : ""}"><strong>${captureLabel}</strong><span>${writable ? `${title(writeModule)} gate armed` : "General hardware writes disabled"}</span></div>
        <div class="dsp-head-actions">
          ${writable ? `<button type="button" class="dsp-revert" data-action="dsp-revert" ${saving || (matched && !dirty) ? "disabled" : ""}>Revert captured</button><button type="button" class="dsp-apply" data-action="dsp-apply" ${saving || !dirty ? "disabled" : ""}>${saving ? "Verifying..." : "Apply + verify"}</button>` : ""}
          <button type="button" class="dsp-refresh" data-action="dsp-refresh" ${loading || saving ? "disabled" : ""}>Refresh snapshot</button>
        </div>
      </header>
      <div class="mic-chain-body">
        <nav class="dsp-module-nav" aria-label="Microphone processing modules">
          ${MODULES.map(([id, label]) => `<button type="button" data-action="dsp-module" data-dsp-module="${id}" class="${activeModule === id ? "is-active" : ""}"><span>${escapeHtml(label)}</span><small>${id === "setup" ? "XLR" : id === "headphones" ? "Playback" : "DSP"}</small></button>`).join("")}
        </nav>
        <section class="dsp-content">${content}</section>
      </div>
    </main>`;
}
