import test from "node:test";
import assert from "node:assert/strict";
import { isMixMuted, toggleMixMute } from "../src/mute-state.js";

test("toggles Personal without changing Audience", () => {
  assert.equal(toggleMixMute("unmuted", "personal"), "muted_personal");
  assert.equal(toggleMixMute("muted_audience", "personal"), "muted_all");
  assert.equal(toggleMixMute("muted_all", "personal"), "muted_audience");
});

test("toggles Audience without changing Personal", () => {
  assert.equal(toggleMixMute("unmuted", "audience"), "muted_audience");
  assert.equal(toggleMixMute("muted_personal", "audience"), "muted_all");
  assert.equal(toggleMixMute("muted_all", "audience"), "muted_personal");
  assert.equal(isMixMuted("muted_all", "audience"), true);
});
