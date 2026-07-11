import "@fontsource/atkinson-hyperlegible/400.css";
import "@fontsource/atkinson-hyperlegible/700.css";
import "@fontsource/barlow-condensed/500.css";
import "@fontsource/barlow-condensed/600.css";
import { createIcons, Headphones, Link, Mic2, Route, Settings, SlidersVertical, Volume2, VolumeX } from "lucide";
import { escapeHtml } from "./escape.js";
import { hardwareControlsEnabled, isReadOnlyHardware } from "./runtime.js";
import "./styles.css";

const API = import.meta.env.VITE_API_URL || "http://127.0.0.1:17840";
const app = document.querySelector("#app");

let state = null;
let runtime = null;
let activeMix = "personal";
let activeView = "mixer";

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

function navItem(icon, label, view, active = false) {
  return `
    <button class="nav-item ${active ? "is-active" : ""}" type="button" ${view ? `data-action="view" data-view="${view}"` : ""}>
      <i data-lucide="${icon}"></i><span>${label}</span>
    </button>`;
}

function routingMarkup(snapshot) {
  const mixerRoutes = snapshot.mixer.routes || [];
  const mixerTargets = snapshot.mixer.targets || [];
  const routes = new Set(mixerRoutes.map((route) => `${route.source_id}\u0000${route.target_id}`));
  return `
    <main class="routing-deck" aria-label="Signal routing">
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

function channelMarkup(channel, index) {
  const personalMeter = Math.max(14, Math.min(82, channel.personal_volume - index * 4));
  const audienceMeter = Math.max(10, Math.min(86, channel.audience_volume - index * 3));
  const muted = channel.mute_state !== "unmuted";
  const sources = channel.applications.length ? channel.applications.join(" · ") : "No source assigned";
  const safeId = escapeHtml(channel.id);
  const safeName = escapeHtml(channel.name);
  const safeSources = escapeHtml(sources);

  return `
    <section class="channel ${muted ? "is-muted" : ""}" data-channel="${safeId}">
      <header class="channel-head">
        <h2>${safeName}</h2>
        <p title="${safeSources}">${safeSources}</p>
      </header>
      <div class="mix-labels"><span>Personal</span><span>Audience</span></div>
      <div class="meters" aria-hidden="true">
        <div class="meter meter-personal"><b style="height:${personalMeter}%"></b></div>
        <div class="meter meter-audience"><b style="height:${audienceMeter}%"></b></div>
      </div>
      <div class="fader-pair">
        ${fader(channel, "personal", channel.personal_volume)}
        ${fader(channel, "audience", channel.audience_volume)}
      </div>
      <button class="mute-button ${muted ? "is-active" : ""}" type="button" data-action="mute" aria-pressed="${muted}">
        <i data-lucide="${muted ? "volume-x" : "volume-2"}"></i>
        <span>${muted ? "Muted" : "Mute"}</span>
      </button>
    </section>`;
}

function fader(channel, mix, value) {
  const safeName = escapeHtml(channel.name);
  return `
    <label class="fader fader-${mix}">
      <span class="sr-only">${safeName} ${mix} volume</span>
      <output>${value}</output>
      <input type="range" min="0" max="100" value="${value}" data-action="volume" data-mix="${mix}" />
      <small>−∞</small>
    </label>`;
}

function inspectorMarkup(snapshot, hardwareControlsEnabled) {
  const studio = snapshot.studio;
  const hardwareDisabled = hardwareControlsEnabled ? "" : "disabled";
  const grouped = ["link1", "link2", "link3", "link4"].map((channel) => {
    const assigned = studio.linked_applications.filter((item) => item.channel === channel);
    return {
      channel,
      label: channel.replace("link", "Link "),
      names: assigned.length ? assigned.map((item) => item.name).join(", ") : "Unassigned",
    };
  });

  return `
    <aside class="inspector">
      <header class="inspector-title">
        <div><span class="device-dot"></span><h2>BEACN Studio</h2></div>
        <small title="${escapeHtml(studio.identity.error || "")}">${studio.identity.error ? "USB unavailable" : escapeHtml(studio.identity.firmware || "Firmware unknown")}</small>
      </header>
      <section class="inspector-section ${hardwareControlsEnabled ? "" : "is-readonly"}">
        <h3>Microphone</h3>
        <label class="horizontal-control">
          <span>Mic gain</span><output data-output="gain">${studio.microphone.gain_db} dB</output>
          <input type="range" min="0" max="69" value="${studio.microphone.gain_db}" data-action="gain" ${hardwareDisabled} />
          <small><span>0 dB</span><span>69 dB</span></small>
        </label>
        <div class="toggle-row">
          <span>Phantom power <small>48V</small></span>
          <button class="power-toggle ${studio.microphone.phantom_power ? "is-active" : ""}" type="button" data-action="phantom" aria-pressed="${studio.microphone.phantom_power}" ${hardwareDisabled}>
            ${studio.microphone.phantom_power ? "On" : "Off"}
          </button>
        </div>
      </section>
      <section class="inspector-section">
        <h3>Headphones</h3>
        <label class="horizontal-control">
          <span>Headphone volume</span><output>${studio.headphones.volume}%</output>
          <input type="range" min="0" max="100" value="${studio.headphones.volume}" disabled />
          <small><span>0%</span><span>100%</span></small>
        </label>
      </section>
      <section class="inspector-section link-section">
        <h3>Link assignments <span>(Game PC)</span></h3>
        <div class="link-list">
          ${grouped.map((item) => `
            <div class="link-row">
              <strong>${item.label}</strong>
              <span>${escapeHtml(item.names)}</span>
            </div>`).join("")}
        </div>
        <details class="application-assignments">
          <summary>Assign individual applications</summary>
          ${studio.linked_applications.map((item) => `
            <label>
              <span>${escapeHtml(item.name)}</span>
              <select data-action="link" data-application="${escapeHtml(item.name)}" ${hardwareDisabled}>
                ${["system", "link1", "link2", "link3", "link4"].map((channel) => `<option value="${channel}" ${channel === item.channel ? "selected" : ""}>${channel === "system" ? "System" : channel.replace("link", "Link ")}</option>`).join("")}
              </select>
            </label>`).join("")}
        </details>
      </section>
    </aside>`;
}

function render() {
  if (!state) return;
  const connected = ["connected", "mock"].includes(state.studio.identity.status);
  const canWriteHardware = hardwareControlsEnabled(runtime);
  const readOnlyHardware = isReadOnlyHardware(runtime);
  app.innerHTML = `
    <div class="shell">
      <header class="topbar">
        <div class="wordmark">StudioBridge</div>
        <div class="connection"><i data-lucide="link"></i><span>BEACN Studio</span><b class="${connected ? "ok" : ""}">${connected ? "Connected" : "Disconnected"}</b>${readOnlyHardware ? '<em title="Gain, 48V, and Link assignment writes are disabled">READ-ONLY</em>' : ""}</div>
        <div class="mix-switch" aria-label="Active mix">
          <button class="personal ${activeMix === "personal" ? "is-active" : ""}" data-action="active-mix" data-mix="personal">Personal</button>
          <span></span>
          <button class="audience ${activeMix === "audience" ? "is-active" : ""}" data-action="active-mix" data-mix="audience">Audience</button>
        </div>
      </header>
      <nav class="sidebar" aria-label="Main navigation">
        <div>
          ${navItem("sliders-vertical", "Mixer", "mixer", activeView === "mixer")}
          ${navItem("route", "Routing", "routing", activeView === "routing")}
          ${navItem("mic-2", "Microphone", null)}
          ${navItem("link", "Link", null)}
          ${navItem("settings", "Settings", null)}
        </div>
        <footer><i data-lucide="headphones"></i><span>Dual-PC mode</span><b>Active</b></footer>
      </nav>
      ${activeView === "routing" ? routingMarkup(state) : `<main class="mixer-deck ${state.mixer.channels.length ? "" : "is-empty"}" aria-label="Audio mixer">${state.mixer.channels.length ? state.mixer.channels.map(channelMarkup).join("") : `<div class="backend-empty"><strong>PipeWeaver unavailable</strong><p>${escapeHtml(state.mixer.error || "Start the PipeWeaver daemon and refresh.")}</p></div>`}</main>`}
      ${inspectorMarkup(state, canWriteHardware)}
    </div>
    <div class="toast" role="status" aria-live="polite"></div>`;

  createIcons({ icons, attrs: { "stroke-width": 1.7 } });
  bindEvents();
}

function bindEvents() {
  document.querySelectorAll('[data-action="view"]').forEach((button) => {
    button.addEventListener("click", () => {
      activeView = button.dataset.view;
      render();
    });
  });

  document.querySelectorAll('[data-action="active-mix"]').forEach((button) => {
    button.addEventListener("click", () => {
      activeMix = button.dataset.mix;
      render();
    });
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

  document.querySelectorAll('[data-action="mute"]').forEach((button) => {
    button.addEventListener("click", async () => {
      const channel = state.mixer.channels.find((item) => item.id === button.closest(".channel").dataset.channel);
      await update("/api/mixer/mute", {
        channel_id: channel.id,
        state: channel.mute_state === "unmuted" ? "muted_all" : "unmuted",
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
      await update("/api/studio/link-assignment", {
        application: select.dataset.application,
        channel: select.value,
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
