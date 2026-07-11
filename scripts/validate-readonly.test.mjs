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
    },
    state: {
      studio: {
        identity: { status: "connected", product: "BEACN Studio", serial: "private", firmware: "1.2.3" },
        microphone: { gain_db: 40, phantom_power: false },
        headphones: { volume: 65 },
        linked_applications: [{ name: "Game", channel: "link1" }],
      },
      mixer: {
        status: "connected",
        channels: [{ name: "Game", personal_volume: 70, audience_volume: 60 }],
        targets: [{ name: "Headphones" }],
        routes: [{ source_id: "Game", target_id: "Headphones" }],
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

test("refuses a daemon with hardware writes enabled", () => {
  const fixture = validFixture();
  fixture.health.hardware_writes_enabled = true;
  const result = validateReadonly(fixture.health, fixture.state);
  assert(result.failures.some((failure) => failure.includes("hardware writes are enabled")));
});
