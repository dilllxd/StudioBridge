import test from "node:test";
import assert from "node:assert/strict";
import { hardwareControlsEnabled, isReadOnlyHardware, linkControlsEnabled } from "../src/runtime.js";

test("mock mode keeps simulated hardware controls interactive", () => {
  const runtime = { studio_mode: "mock", hardware_writes_enabled: false };
  assert.equal(hardwareControlsEnabled(runtime), true);
  assert.equal(linkControlsEnabled(runtime), true);
  assert.equal(isReadOnlyHardware(runtime), false);
});

test("real mode is read-only until hardware writes are explicitly enabled", () => {
  const runtime = { studio_mode: "beacn", hardware_writes_enabled: false };
  assert.equal(hardwareControlsEnabled(runtime), false);
  assert.equal(linkControlsEnabled(runtime), false);
  assert.equal(isReadOnlyHardware(runtime), true);

  runtime.hardware_writes_enabled = true;
  assert.equal(hardwareControlsEnabled(runtime), true);
  assert.equal(linkControlsEnabled(runtime), true);
  assert.equal(isReadOnlyHardware(runtime), false);
});

test("Link host control does not enable microphone hardware writes", () => {
  const runtime = {
    studio_mode: "beacn",
    hardware_writes_enabled: false,
    link_control_enabled: true,
  };
  assert.equal(hardwareControlsEnabled(runtime), false);
  assert.equal(linkControlsEnabled(runtime), true);
  assert.equal(isReadOnlyHardware(runtime), true);
});
