import test from "node:test";
import assert from "node:assert/strict";
import {
  clampRequestDeadlineMs,
  SOLVE_DEADLINE_MAX_MS,
} from "../../solver/js/core/limits.mjs";
import {
  SITE_MESSAGE_PURPOSE,
  WORKER_MESSAGE_FIELDS,
  WORKER_MESSAGE_TYPES,
} from "../../solver/js/core/protocol.mjs";
import {
  BoundedPayloadCache,
  fillCachedPayload,
  PAYLOAD_CACHE_LIMIT,
} from "../../solver/js/core/payload-cache.mjs";

test("request deadline clamp keeps the existing 1ms and max bounds", () => {
  assert.equal(clampRequestDeadlineMs(null), null);
  assert.equal(clampRequestDeadlineMs(undefined), null);
  assert.equal(clampRequestDeadlineMs(Number.NaN), null);
  assert.equal(clampRequestDeadlineMs(0), null);
  assert.equal(clampRequestDeadlineMs(-5), null);
  assert.equal(clampRequestDeadlineMs(0.4), 1);
  assert.equal(clampRequestDeadlineMs(1_000.6), 1_001);
  assert.equal(clampRequestDeadlineMs(SOLVE_DEADLINE_MAX_MS + 1), SOLVE_DEADLINE_MAX_MS);
});

test("protocol constants keep the worker and site vocabulary stable", () => {
  assert.deepEqual(
    { ...WORKER_MESSAGE_TYPES },
    {
      INIT: "init",
      READY: "ready",
      INIT_ERROR: "init-error",
      SOLVE: "solve",
      RESULT: "result",
      ERROR: "error",
    },
  );
  assert.equal(WORKER_MESSAGE_FIELDS.TYPE, "type");
  assert.equal(WORKER_MESSAGE_FIELDS.INIT_ERROR, undefined);
  assert.equal(SITE_MESSAGE_PURPOSE.SOLVE, "solve");
  assert.equal(SITE_MESSAGE_PURPOSE.SEARCH, "search");
});

test("fillCachedPayload only fills dataKey-only messages that are cached", () => {
  const cache = new BoundedPayloadCache(PAYLOAD_CACHE_LIMIT);
  cache.set("dk1", { marker: 1 });

  assert.deepEqual(fillCachedPayload({ dataKey: "dk1" }, cache), {
    dataKey: "dk1",
    data: { marker: 1 },
  });
  assert.deepEqual(fillCachedPayload({ dataKey: "dk1", data: null }, cache), {
    dataKey: "dk1",
    data: { marker: 1 },
  });
  assert.deepEqual(
    fillCachedPayload({ dataKey: "dk1", data: { explicit: true } }, cache),
    { dataKey: "dk1", data: { explicit: true } },
  );
  assert.deepEqual(fillCachedPayload({ dataKey: "missing" }, cache), {
    dataKey: "missing",
  });
  assert.deepEqual(fillCachedPayload({ data: { marker: 2 } }, cache), {
    data: { marker: 2 },
  });
  assert.equal(fillCachedPayload(null, cache), null);
});
