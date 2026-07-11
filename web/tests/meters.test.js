import test from "node:test";
import assert from "node:assert/strict";
import { meterSocketUrl, parseMeterEvent } from "../src/meters.js";

test("builds the PipeWeaver meter endpoint on the current host", () => {
  assert.equal(
    meterSocketUrl({ protocol: "http:", hostname: "192.168.1.69" }),
    "ws://192.168.1.69:14565/api/websocket/meter",
  );
  assert.equal(
    meterSocketUrl({ protocol: "https:", hostname: "studio.local" }),
    "wss://studio.local:14565/api/websocket/meter",
  );
});

test("validates and clamps meter frames", () => {
  assert.deepEqual(parseMeterEvent('{"id":"source-id","percent":73}'), { id: "source-id", percent: 73 });
  assert.deepEqual(parseMeterEvent('{"id":"source-id","percent":140}'), { id: "source-id", percent: 100 });
  assert.equal(parseMeterEvent('{"percent":20}'), null);
});
