import assert from "node:assert/strict";
import test from "node:test";
import { escapeHtml } from "../src/escape.js";

test("escapes text and attribute delimiters from application names", () => {
  assert.equal(
    escapeHtml('<img src=x onerror="boom"> & \'quoted\''),
    "&lt;img src=x onerror=&quot;boom&quot;&gt; &amp; &#39;quoted&#39;",
  );
});

test("normalizes missing values", () => {
  assert.equal(escapeHtml(null), "");
  assert.equal(escapeHtml(undefined), "");
});
