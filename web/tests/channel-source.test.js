import test from "node:test";
import assert from "node:assert/strict";
import { channelSourceLabel } from "../src/channel-source.js";

const studio = {
  linked_applications: [
    { name: "Game", channel: "link1" },
    { name: "Steam", channel: "link1" },
    { name: "Browser", channel: "link2" },
  ],
};

test("correlates multiple Windows applications with a physical Link input", () => {
  const label = channelSourceLabel(
    { id: "Link 1", name: "Link 1", source_kind: "physical", applications: [] },
    studio,
  );
  assert.equal(label, "Windows · Game · Steam");
});

test("describes physical and virtual sources without claiming they are unassigned", () => {
  assert.equal(
    channelSourceLabel(
      { id: "Microphone", name: "Microphone", source_kind: "physical", applications: [] },
      studio,
    ),
    "Physical input · BEACN Studio microphone",
  );
  assert.equal(
    channelSourceLabel(
      { id: "Communication", name: "Communication", source_kind: "virtual", applications: [] },
      studio,
    ),
    "Waiting for a Linux application",
  );
});
