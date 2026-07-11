import "@fontsource/atkinson-hyperlegible/400.css";
import "@fontsource/atkinson-hyperlegible/700.css";
import "@fontsource/barlow-condensed/500.css";
import "@fontsource/barlow-condensed/600.css";
import { createIcons, Headphones, Link, Mic2, Route, Settings, SlidersVertical, Volume2, VolumeX } from "lucide";
import { channelSourceLabel } from "./channel-source.js";
import { escapeHtml } from "./escape.js";
import { connectPipeweaverMeters } from "./meters.js";
import { micChainMarkup } from "./mic-chain.js";
import { isMixMuted, toggleMixMute } from "./mute-state.js";
import { hardwareControlsEnabled, isReadOnlyHardware, linkControlsEnabled } from "./runtime.js";
import "./styles.css";

const API = import.meta.env.VITE_API_URL || window.location.origin;
const app = document.querySelector("#app");

let state = null;
let runtime = null;
let activeView = "mixer";
let hasRendered = false;
let meterConnection = null;
let dspState = null;
let dspCaptured = null;
let dspDraft = null;
let dspLoading = false;
let dspSaving = false;
let dspError = null;
let activeDspModule = "setup";
const dspViewModes = {};

const views = [
  ["mixer", "Mixer"],
  ["routing", "Routing"],
  ["applications", "Applications"],
  ["hardware", "Mic Chain"],
];

const icons = {
  Headphones,
  Link,
  Mic2,
  Route,
  Settings,
  SlidersVertical,
  Volume2,
  VolumeX,
};

async function request(path, options = {}) {
  const response = await fetch(`${API}${path}`, {
    headers: { "Content-Type": "application/json" },
    ...options,
  });
  if (!response.ok) {
    const body = await response.json().catch(() => ({}));
    throw new Error(body.error || `Request failed (${response.status})`);
  }
  return response.json();
}

async function loadMicrophoneDsp() {
  if (dspLoading) return;
  dspLoading = true;
  dspError = null;
  render();
  try {
    const next = await request("/api/studio/microphone-dsp");
    dspState = next;
    if (!dspCaptured) dspCaptured = structuredClone(next);
    dspDraft = structuredClone(next);
  } catch (error) {
    dspError = error instanceof Error ? error.message : String(error);
  } finally {
    dspLoading = false;
    render();
  }
}

const dspModuleNames = {
  noise: "noise_suppression",
  equalizer: "equalizer",
  compressor: "compressor",
  expander: "expander",
  enhancement: "enhancement_suite",
  headphones: "headphone_equalizer",
};

function setDspDraftValue(path, value) {
  if (!dspDraft || !path) return;
  const keys = path.split(".");
  const last = keys.pop();
  const target = keys.reduce((current, key) => current?.[key], dspDraft);
  if (target && last !== undefined) target[last] = value;
}

function formatDspValue(value) {
  return Number(value).toFixed(2).replace(/\.00$/, "").replace(/(\.\d)0$/, "$1");
}

function updateDspActionState() {
  const module = dspModuleNames[activeDspModule];
  if (!module || !dspState || !dspDraft || !dspCaptured) return;
  const dirty = JSON.stringify(dspState[module]) !== JSON.stringify(dspDraft[module]);
  const matched = JSON.stringify(dspState) === JSON.stringify(dspCaptured);
  const apply = document.querySelector('[data-action="dsp-apply"]');
  const revert = document.querySelector('[data-action="dsp-revert"]');
  const capture = document.querySelector(".capture-status");
  if (apply) apply.disabled = dspSaving || !dirty;
  if (revert) revert.disabled = dspSaving || (matched && !dirty);
  if (capture) {
    capture.classList.toggle("is-matched", matched && !dirty);
    const label = capture.querySelector("strong");
    if (label) label.textContent = dirty
      ? "Unsaved module changes"
      : matched
        ? "Captured state matches"
        : "Verified device change active";
  }
}

async function writeDspModule(source) {
  const module = dspModuleNames[activeDspModule];
  if (!module || !source || dspSaving) return;
  dspSaving = true;
  dspError = null;
  render();
  try {
    const result = await request("/api/studio/microphone-dsp", {
      method: "POST",
      body: JSON.stringify({ module, state: source[module] }),
    });
    if (!result.verified) throw new Error(`${module} read-back was not verified`);
    dspState = result.snapshot;
    dspDraft = structuredClone(result.snapshot);
  } catch (error) {
    dspError = error instanceof Error ? error.message : String(error);
  } finally {
    dspSaving = false;
    render();
  }
}

function routingMarkup(snapshot) {
  const mixerRoutes = snapshot.mixer.routes || [];
  const mixerTargets = snapshot.mixer.targets || [];
  const routes = new Set(mixerRoutes.map((route) => `${route.source_id}\u0000${route.target_id}`));
  return `
    <main class="routing-deck main-view" aria-label="Signal routing">
      <header class="routing-head">
        <h1>Signal routing</h1>
        <p>Choose exactly where each source is heard. Personal and Audience levels remain independent.</p>
      </header>
      ${mixerTargets.length ? `
        <div class="route-matrix" style="--target-count:${mixerTargets.length}">
          <div class="route-corner">Source</div>
          ${mixerTargets.map((target) => `<div class="route-target"><strong>${escapeHtml(target.name)}</strong><small>${escapeHtml(target.mix)}</small></div>`).join("")}
          ${snapshot.mixer.channels.map((channel) => `
            <div class="route-source"><span style="--channel-colour:${escapeHtml(channel.colour)}"></span><strong>${escapeHtml(channel.name)}</strong></div>
            ${mixerTargets.map((target) => {
              const enabled = routes.has(`${channel.id}\u0000${target.id}`);
              return `<button class="route-cell ${enabled ? "is-active" : ""}" type="button" data-action="route" data-source="${escapeHtml(channel.id)}" data-target="${escapeHtml(target.id)}" data-enabled="${enabled}" aria-label="Route ${escapeHtml(channel.name)} to ${escapeHtml(target.name)}" aria-pressed="${enabled}"><span></span></button>`;
            }).join("")}`).join("")}
        </div>` : `<div class="backend-empty"><strong>No output targets</strong><p>Attach Personal and Audience targets in PipeWeaver first.</p></div>`}
    </main>`;
}

function channelMarkup(channel, studio) {
  const personalMuted = isMixMuted(channel.mute_state, "personal");
  const audienceMuted = isMixMuted(channel.mute_state, "audience");
  const sources = channelSourceLabel(channel, studio);
  const safeId = escapeHtml(channel.id);
  const safeName = escapeHtml(channel.name);
  const safeSources = escapeHtml(sources);

  return `
    <section class="channel ${personalMuted ? "is-personal-muted" : ""} ${audienceMuted ? "is-audience-muted" : ""}" data-channel="${safeId}" style="--channel-colour:${escapeHtml(channel.colour)}">
      <header class="channel-head">
        <h2>${safeName}</h2>
        <p title="${safeSources}">${safeSources}</p>
      </header>
      <div class="fader-pair">
        ${fader(channel, "personal", channel.personal_volume)}
        ${fader(channel, "audience", channel.audience_volume)}
        <button class="volume-link ${channel.volumes_linked ? "is-active" : ""}" type="button" data-action="volume-link" aria-label="${channel.volumes_linked ? "Unlink" : "Link"} Personal and Audience volume" aria-pressed="${channel.volumes_linked}" title="${channel.volumes_linked ? "Personal and Audience move together" : "Personal and Audience move independently"}"><i data-lucide="link"></i></button>
      </div>
      <div class="mute-pair">
        <button class="mute-button mute-personal ${personalMuted ? "is-active" : ""}" type="button" data-action="mute" data-mute-mix="personal" aria-pressed="${personalMuted}"><i data-lucide="${personalMuted ? "volume-x" : "volume-2"}"></i><span>Personal</span></button>
        <button class="mute-button mute-audience ${audienceMuted ? "is-active" : ""}" type="button" data-action="mute" data-mute-mix="audience" aria-pressed="${audienceMuted}"><i data-lucide="${audienceMuted ? "volume-x" : "volume-2"}"></i><span>Audience</span></button>
      </div>
    </section>`;
}

function targetMarkup(target) {
  return `
    <section class="target-card">
      <header><h2>${escapeHtml(target.name)}</h2><span>${escapeHtml(target.mix)}</span></header>
      <div class="target-meter meter-${target.mix}" data-meter-id="${escapeHtml(target.meter_id)}" data-volume="${target.volume}"><b></b></div>
      <strong>${target.volume}%</strong>
      <input type="range" min="0" max="100" value="${target.volume}" data-action="target-volume" data-target="${escapeHtml(target.id)}" aria-label="${escapeHtml(target.name)} output volume" />
      <small>${target.muted ? "Muted" : `${target.mix} output`}</small>
    </section>`;
}

function mixerMarkup(snapshot) {
  return `
    <main class="mixer-workspace main-view">
      <section class="source-bank">
        <header class="bank-title"><h1>Sources</h1><span>Live PipeWeaver meters</span></header>
        <div class="mixer-deck ${snapshot.mixer.channels.length ? "" : "is-empty"}">
          ${snapshot.mixer.channels.length
            ? snapshot.mixer.channels.map((channel) => channelMarkup(channel, snapshot.studio)).join("")
            : `<div class="backend-empty"><strong>PipeWeaver unavailable</strong><p>${escapeHtml(snapshot.mixer.error || "Start the PipeWeaver daemon and refresh.")}</p></div>`}
        </div>
      </section>
      <section class="target-bank">
        <header class="bank-title"><h1>Targets</h1></header>
        <div class="target-list">${snapshot.mixer.targets.map(targetMarkup).join("")}</div>
      </section>
    </main>`;
}

function fader(channel, mix, value) {
  const safeName = escapeHtml(channel.name);
  return `
    <label class="fader fader-${mix}">
      <span class="sr-only">${safeName} ${mix} volume</span>
      <span class="fader-label">${mix}</span>
      <output>${value}</output>
      <span class="fader-meter" data-meter-id="${escapeHtml(channel.meter_id)}" data-volume="${value}" aria-hidden="true"><b></b></span>
      <input type="range" min="0" max="100" value="${value}" data-action="volume" data-mix="${mix}" />
      <small>−∞</small>
    </label>`;
}

function applicationsMarkup(snapshot, linkControlsEnabled) {
  const studio = snapshot.studio;
  const mixerApplications = snapshot.mixer.applications || [];
  const linkDisabled = linkControlsEnabled ? "" : "disabled";
  const grouped = ["link1", "link2", "link3", "link4"].map((channel) => {
    const assigned = studio.linked_applications.filter((item) => item.channel === channel);
    return {
      channel,
      label: channel.replace("link", "Link "),
      applications: assigned,
    };
  });

  return `
    <main class="applications-deck main-view">
      <section class="assignment-panel link-section ${linkControlsEnabled ? "" : "is-readonly"}">
        <header><div><h1>Windows apps</h1><span>BEACN Link · Game PC</span></div><small>Drag an app to any Link channel</small></header>
        <p class="link-instruction">Drag any app onto a Link channel. Channels can contain multiple apps.</p>
        <div class="link-app-pool" aria-label="Windows applications">
          ${studio.linked_applications.length ? studio.linked_applications.map((application) => `<span class="link-app-chip" draggable="true" data-drag-link-app="${escapeHtml(application.name)}" title="Drag ${escapeHtml(application.name)} to a Link channel">${escapeHtml(application.name)}</span>`).join("") : `<span class="link-empty">Start a Windows audio application to assign it.</span>`}
        </div>
        <div class="link-list">
          ${grouped.map((item) => `
            <div class="link-row" data-drop-link="${item.channel}" aria-label="Drop Windows applications on ${item.label}">
              <strong>${item.label}</strong>
              <div class="link-apps">
                ${item.applications.length ? item.applications.map((application) => `<span class="link-app-chip" draggable="true" data-drag-link-app="${escapeHtml(application.name)}" title="Drag ${escapeHtml(application.name)} to another Link channel">${escapeHtml(application.name)}</span>`).join("") : `<span class="link-empty">Drop apps here</span>`}
              </div>
            </div>`).join("")}
        </div>
        <details class="application-assignments">
          <summary>Keyboard and touch assignment</summary>
          ${studio.linked_applications.map((item) => `
            <label>
              <span>${escapeHtml(item.name)}</span>
              <select data-action="link" data-application="${escapeHtml(item.name)}" ${linkDisabled}>
                ${["system", "link1", "link2", "link3", "link4"].map((channel) => `<option value="${channel}" ${channel === item.channel ? "selected" : ""}>${channel === "system" ? "System" : channel.replace("link", "Link ")}</option>`).join("")}
              </select>
            </label>`).join("")}
        </details>
      </section>
      <section class="assignment-panel">
        <header><div><h1>Linux apps</h1><span>PipeWire · Stream PC</span></div><small>Persistent PipeWeaver assignment</small></header>
        <details class="application-assignments" open>
          <summary>Assign PipeWire applications</summary>
          ${mixerApplications.length ? mixerApplications.map((item) => `
            <label>
              <span title="${escapeHtml(item.process)}">${escapeHtml(item.title || item.name)}</span>
              <select data-action="mixer-application" data-process="${escapeHtml(item.process)}" data-application="${escapeHtml(item.name)}">
                <option value="" ${item.channel_id ? "" : "selected"}>Unassigned</option>
                ${snapshot.mixer.channels.map((channel) => `<option value="${escapeHtml(channel.id)}" ${channel.id === item.channel_id ? "selected" : ""}>${escapeHtml(channel.name)}</option>`).join("")}
              </select>
            </label>`).join("") : `<p class="assignment-empty">Start a Linux audio application to assign it.</p>`}
        </details>
      </section>
    </main>`;
}

function hardwareMarkup(snapshot, hardwareControlsEnabled) {
  const studio = snapshot.studio;
  const disabled = hardwareControlsEnabled ? "" : "disabled";
  return `
    <main class="hardware-deck main-view ${hardwareControlsEnabled ? "" : "is-readonly"}">
      <section class="hardware-card">
        <header><h1>Microphone</h1><span>BEACN Studio</span></header>
        <label class="horizontal-control">
          <span>Mic gain</span><output data-output="gain">${studio.microphone.gain_db} dB</output>
          <input type="range" min="0" max="69" value="${studio.microphone.gain_db}" data-action="gain" ${disabled} />
          <small><span>0 dB</span><span>69 dB</span></small>
        </label>
        <div class="toggle-row">
          <span>Phantom power <small>48V</small></span>
          <button class="power-toggle ${studio.microphone.phantom_power ? "is-active" : ""}" type="button" data-action="phantom" aria-pressed="${studio.microphone.phantom_power}" ${disabled}>${studio.microphone.phantom_power ? "On" : "Off"}</button>
        </div>
      </section>
      <section class="hardware-card">
        <header><h1>Headphones</h1><span>Read-only hardware state</span></header>
        <label class="horizontal-control">
          <span>Headphone volume</span><output>${studio.headphones.volume}%</output>
          <input type="range" min="0" max="100" value="${studio.headphones.volume}" disabled />
          <small><span>0%</span><span>100%</span></small>
        </label>
      </section>
      <aside class="safety-card"><strong>Hardware safe</strong><p>Gain, 48V, firmware, reset, and storage writes are disabled. Link heartbeat and application assignment are isolated.</p></aside>
    </main>`;
}

function render() {
  if (!state) return;
  const uiState = hasRendered ? captureUiState() : null;
  const connected = ["connected", "mock"].includes(state.studio.identity.status);
  const canWriteHardware = hardwareControlsEnabled(runtime);
  const canControlLink = linkControlsEnabled(runtime);
  const readOnlyHardware = isReadOnlyHardware(runtime);
  const mainContent = activeView === "routing"
    ? routingMarkup(state)
    : activeView === "applications"
      ? applicationsMarkup(state, canControlLink)
      : activeView === "hardware"
        ? micChainMarkup({
            snapshot: state,
            dsp: dspState,
            captured: dspCaptured,
            draft: dspDraft,
            loading: dspLoading,
            saving: dspSaving,
            error: dspError,
            activeModule: activeDspModule,
            viewModes: dspViewModes,
            writeModules: runtime?.dsp_write_modules || [],
          })
        : mixerMarkup(state);
  app.innerHTML = `
    <div class="shell ${hasRendered ? "no-intro" : ""}">
      <header class="topbar">
        <div class="wordmark">StudioBridge</div>
        <nav class="view-tabs" aria-label="Main views">
          ${views.map(([id, label]) => `<button type="button" data-action="view" data-view="${id}" class="${activeView === id ? "is-active" : ""}">${label}</button>`).join("")}
        </nav>
        <div class="connection"><i data-lucide="link"></i><span>BEACN Studio</span><b class="${connected ? "ok" : ""}">${connected ? "Connected" : "Disconnected"}</b>${readOnlyHardware ? `<em title="Gain and 48V writes are disabled${canControlLink ? "; Link host control is isolated" : ""}">HARDWARE SAFE</em>` : ""}</div>
      </header>
      ${mainContent}
    </div>
    <div class="toast" role="status" aria-live="polite"></div>`;

  createIcons({ icons, attrs: { "stroke-width": 1.7 } });
  bindEvents();
  if (uiState) restoreUiState(uiState);
  if (!meterConnection && runtime?.mixer_mode === "pipeweaver") {
    meterConnection = connectPipeweaverMeters(updateMeterLevel);
  }
  hasRendered = true;
}

function captureUiState() {
  const focused = document.activeElement;
  return {
    mainScroll: document.querySelector(".main-view")?.scrollTop || 0,
    deckScroll: document.querySelector(".mixer-deck")?.scrollLeft || 0,
    openDetails: [...document.querySelectorAll(".main-view details")].map((details) => details.open),
    focus: focused?.dataset?.action
      ? {
          action: focused.dataset.action,
          application: focused.dataset.application,
          process: focused.dataset.process,
          mix: focused.dataset.mix,
          view: focused.dataset.view,
          dspModule: focused.dataset.dspModule,
          dspMode: focused.dataset.dspMode,
          dspPath: focused.dataset.dspPath,
          channel: focused.closest?.(".channel")?.dataset.channel,
        }
      : null,
  };
}

function restoreUiState(uiState) {
  const mainView = document.querySelector(".main-view");
  const deck = document.querySelector(".mixer-deck");
  if (mainView) mainView.scrollTop = uiState.mainScroll;
  if (deck) deck.scrollLeft = uiState.deckScroll;
  document.querySelectorAll(".main-view details").forEach((details, index) => {
    details.open = uiState.openDetails[index] ?? details.open;
  });
  if (!uiState.focus) return;
  const candidate = [...document.querySelectorAll(`[data-action="${uiState.focus.action}"]`)].find((element) =>
    ["application", "process", "mix", "view", "dspModule", "dspMode", "dspPath"].every((key) =>
      uiState.focus[key] === undefined || element.dataset[key] === uiState.focus[key],
    ) && (uiState.focus.channel === undefined || element.closest(".channel")?.dataset.channel === uiState.focus.channel),
  );
  candidate?.focus({ preventScroll: true });
}

function updateMeterLevel(id, percent) {
  document.querySelectorAll("[data-meter-id]").forEach((meter) => {
    if (meter.dataset.meterId !== id) return;
    const volume = Number(meter.dataset.volume || 100);
    const visibleLevel = Math.max(0, Math.min(100, percent * volume / 100));
    meter.style.setProperty("--meter-level", `${visibleLevel}%`);
  });
}

function bindEvents() {
  document.querySelectorAll('[data-action="view"]').forEach((button) => {
    button.addEventListener("click", () => {
      activeView = button.dataset.view;
      render();
      if (activeView === "hardware" && !dspState) void loadMicrophoneDsp();
    });
  });

  document.querySelector('[data-action="dsp-refresh"]')?.addEventListener("click", () => {
    void loadMicrophoneDsp();
  });
  document.querySelectorAll('[data-action="dsp-module"]').forEach((button) => {
    button.addEventListener("click", () => {
      activeDspModule = button.dataset.dspModule;
      render();
    });
  });
  document.querySelectorAll('[data-action="dsp-mode-view"]').forEach((button) => {
    button.addEventListener("click", () => {
      dspViewModes[button.dataset.dspModule] = button.dataset.dspMode;
      render();
    });
  });
  document.querySelectorAll('[data-action="dsp-control"]').forEach((input) => {
    input.addEventListener("input", () => {
      setDspDraftValue(input.dataset.dspPath, Number(input.value));
      const output = input.closest(".dsp-control")?.querySelector("output");
      if (output) output.textContent = `${formatDspValue(input.value)}${input.dataset.unit || ""}`;
      updateDspActionState();
    });
  });
  document.querySelectorAll('[data-action="dsp-select"]').forEach((select) => {
    select.addEventListener("change", () => {
      const value = select.dataset.valueType === "number" ? Number(select.value) : select.value;
      setDspDraftValue(select.dataset.dspPath, value);
      render();
    });
  });
  document.querySelectorAll('[data-action="dsp-toggle"]').forEach((button) => {
    button.addEventListener("click", () => {
      setDspDraftValue(button.dataset.dspPath, button.dataset.enabled !== "true");
      render();
    });
  });
  document.querySelectorAll('[data-action="dsp-set-active-mode"]').forEach((button) => {
    button.addEventListener("click", () => {
      setDspDraftValue(`${button.dataset.dspModule}.active_mode`, button.dataset.dspMode);
      render();
    });
  });
  document.querySelector('[data-action="dsp-apply"]')?.addEventListener("click", () => {
    void writeDspModule(dspDraft);
  });
  document.querySelector('[data-action="dsp-revert"]')?.addEventListener("click", () => {
    void writeDspModule(dspCaptured);
  });

  document.querySelectorAll('[data-action="volume"]').forEach((input) => {
    input.addEventListener("input", () => {
      input.closest(".fader").querySelector("output").textContent = input.value;
    });
    input.addEventListener("change", async () => {
      const channelId = input.closest(".channel").dataset.channel;
      await update("/api/mixer/volume", {
        channel_id: channelId,
        mix: input.dataset.mix,
        volume: Number(input.value),
      });
    });
  });

  document.querySelectorAll('[data-action="volume-link"]').forEach((button) => {
    button.addEventListener("click", async () => {
      const channel = state.mixer.channels.find((item) => item.id === button.closest(".channel").dataset.channel);
      await update("/api/mixer/volume-link", {
        channel_id: channel.id,
        linked: !channel.volumes_linked,
      });
    });
  });

  document.querySelectorAll('[data-action="mute"]').forEach((button) => {
    button.addEventListener("click", async () => {
      const channel = state.mixer.channels.find((item) => item.id === button.closest(".channel").dataset.channel);
      await update("/api/mixer/mute", {
        channel_id: channel.id,
        state: toggleMixMute(channel.mute_state, button.dataset.muteMix),
      });
    });
  });

  document.querySelector('[data-action="gain"]')?.addEventListener("input", (event) => {
    document.querySelector('[data-output="gain"]').textContent = `${event.target.value} dB`;
  });
  document.querySelector('[data-action="gain"]')?.addEventListener("change", async (event) => {
    await update("/api/studio/microphone", { gain_db: Number(event.target.value) });
  });
  document.querySelector('[data-action="phantom"]')?.addEventListener("click", async () => {
    await update("/api/studio/microphone", { phantom_power: !state.studio.microphone.phantom_power });
  });
  document.querySelectorAll('[data-action="link"]').forEach((select) => {
    select.addEventListener("change", async () => {
      await assignLinkApplication(select.dataset.application, select.value);
    });
  });
  document.querySelectorAll("[data-drag-link-app]").forEach((chip) => {
    chip.addEventListener("dragstart", (event) => {
      event.dataTransfer.effectAllowed = "move";
      event.dataTransfer.setData("text/plain", chip.dataset.dragLinkApp);
      chip.classList.add("is-dragging");
    });
    chip.addEventListener("dragend", () => {
      chip.classList.remove("is-dragging");
      document.querySelectorAll(".link-row.is-dragover").forEach((row) => row.classList.remove("is-dragover"));
    });
  });
  document.querySelectorAll("[data-drop-link]").forEach((row) => {
    row.addEventListener("dragover", (event) => {
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      row.classList.add("is-dragover");
    });
    row.addEventListener("dragleave", () => row.classList.remove("is-dragover"));
    row.addEventListener("drop", async (event) => {
      event.preventDefault();
      row.classList.remove("is-dragover");
      const application = event.dataTransfer.getData("text/plain");
      if (application) await assignLinkApplication(application, row.dataset.dropLink);
    });
  });
  document.querySelectorAll('[data-action="mixer-application"]').forEach((select) => {
    select.addEventListener("change", async () => {
      await update("/api/mixer/application", {
        process: select.dataset.process,
        name: select.dataset.application,
        channel_id: select.value || null,
      });
    });
  });
  document.querySelectorAll('[data-action="route"]').forEach((button) => {
    button.addEventListener("click", async () => {
      await update("/api/mixer/route", {
        source_id: button.dataset.source,
        target_id: button.dataset.target,
        enabled: button.dataset.enabled !== "true",
      });
    });
  });
  document.querySelectorAll('[data-action="target-volume"]').forEach((input) => {
    input.addEventListener("change", async () => {
      await update("/api/mixer/target-volume", {
        target_id: input.dataset.target,
        volume: Number(input.value),
      });
    });
  });
}

async function assignLinkApplication(application, channel) {
  await update("/api/studio/link-assignment", { application, channel });
}

async function update(path, payload) {
  try {
    await request(path, { method: "POST", body: JSON.stringify(payload) });
    state = await request("/api/state");
    render();
  } catch (error) {
    showToast(error.message);
  }
}

function showToast(message) {
  const toast = document.querySelector(".toast");
  if (!toast) return;
  toast.textContent = message;
  toast.classList.add("is-visible");
  window.setTimeout(() => toast.classList.remove("is-visible"), 3200);
}

async function start() {
  app.innerHTML = `<div class="loading"><span></span><strong>StudioBridge</strong><p>Connecting to the control daemon…</p></div>`;
  try {
    [runtime, state] = await Promise.all([request("/api/health"), request("/api/state")]);
    render();
  } catch (error) {
    app.innerHTML = `<div class="error-screen"><strong>StudioBridge</strong><h1>Control daemon unavailable</h1><p>${escapeHtml(error.message)}</p><code>cargo run -p studiobridge-daemon</code><button type="button">Try again</button></div>`;
    document.querySelector(".error-screen button").addEventListener("click", start);
  }
}

start();
