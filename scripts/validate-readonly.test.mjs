import test from "node:test";
import assert from "node:assert/strict";
import { validateReadonly } from "./validate-readonly.mjs";

function validFixture() {
  return {
    health: {
      ok: true,
      studio_mode: "beacn",
      mixer_mode: "pipeweaver",
      hardware_writes_enabled: false,
      link_control_enabled: false,
    },
    state: {
      studio: {
        identity: { status: "connected", product: "BEACN Studio", serial: "private", firmware: "1.2.3", driverless_mode: false },
        microphone: { gain_db: 40, phantom_power: false },
        headphones: { volume: 65, mic_monitor: 40, channels_linked: true, output_mode: "line_level", mic_output_gain_tenths_db: 60 },
        linked_applications: [{ name: "Game", channel: "link1" }],
      },
      mixer: {
        status: "connected",
        channels: [{ id: "Game", meter_id: "source-id", name: "Game", source_kind: "physical", volumes_linked: false, personal_volume: 70, audience_volume: 60 }],
        targets: [
          { id: "Headphones", meter_id: "headphones-id", name: "Headphones", mix: "personal" },
          { id: "Audience Mix", meter_id: "audience-id", name: "Audience Mix", mix: "audience" },
        ],
        routes: [
          { source_id: "Game", target_id: "Headphones" },
          { source_id: "Game", target_id: "Audience Mix" },
        ],
      },
    },
  };
}

test("accepts an internally consistent read-only hardware baseline", () => {
  const fixture = validFixture();
  const result = validateReadonly(fixture.health, fixture.state);
  assert.deepEqual(result.failures, []);
  assert.equal(result.summary.sources, 1);
});

test("accepts the constrained Link-control phase after the baseline", () => {
  const fixture = validFixture();
  fixture.health.link_control_enabled = true;
  const result = validateReadonly(fixture.health, fixture.state, { expectLinkControl: true });
  assert.deepEqual(result.failures, []);
});

test("refuses a daemon with hardware writes enabled", () => {
  const fixture = validFixture();
  fixture.health.hardware_writes_enabled = true;
  const result = validateReadonly(fixture.health, fixture.state);
  assert(result.failures.some((failure) => failure.includes("hardware writes are enabled")));
});

test("rejects invalid read-only headphone and driverless states", () => {
  const fixture = validFixture();
  fixture.state.studio.identity.driverless_mode = "off";
  fixture.state.studio.headphones.mic_output_gain_tenths_db = 121;
  fixture.state.studio.headphones.output_mode = "unknown";
  const result = validateReadonly(fixture.health, fixture.state);
  assert(result.failures.some((failure) => failure.includes("driverless-mode")));
  assert(result.failures.some((failure) => failure.includes("output gain")));
  assert(result.failures.some((failure) => failure.includes("output mode")));
});

test("requires the narrow USB1 Link host control when requested", () => {
  const fixture = validFixture();
  const result = validateReadonly(fixture.health, fixture.state, { expectLinkControl: true });
  assert(result.failures.some((failure) => failure.includes("Link host control is not enabled")));
});

test("refuses Link host control during the initial baseline", () => {
  const fixture = validFixture();
  fixture.health.link_control_enabled = true;
  const result = validateReadonly(fixture.health, fixture.state);
  assert(result.failures.some((failure) => failure.includes("before the read-only baseline passed")));
});

test("requires routes to both Personal and Audience targets", () => {
  const fixture = validFixture();
  fixture.state.mixer.routes = fixture.state.mixer.routes.filter((route) => route.target_id !== "Audience Mix");
  const result = validateReadonly(fixture.health, fixture.state);
  assert(result.failures.some((failure) => failure.includes("no route to an Audience target")));
});

test("rejects routes that refer to unknown graph nodes", () => {
  const fixture = validFixture();
  fixture.state.mixer.routes.push({ source_id: "missing", target_id: "also-missing" });
  const result = validateReadonly(fixture.health, fixture.state);
  assert(result.failures.some((failure) => failure.includes("unknown source")));
  assert(result.failures.some((failure) => failure.includes("unknown target")));
});
