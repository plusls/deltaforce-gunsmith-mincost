import test from "node:test";
import assert from "node:assert/strict";
import { createSandbox, waitFor } from "./harness.mjs";

const DATA_KEY = "dk-blackhawk";

function siteRequest(requestId, overrides = {}) {
  return {
    requestId,
    purpose: "solve",
    dataKey: DATA_KEY,
    weaponId: "w1",
    mode: "blackhawk",
    laserEnabled: false,
    currentSelections: {},
    tuningState: {},
    lockedPathKeys: [],
    lockedTuningKeys: [],
    excludedPartIds: [],
    pricesByObjectId: {},
    targets: { budgetEnabled: false },
    smartOptions: { preferenceConditions: { recoil: { operator: "gte", value: 50 } } },
    timeBudgetMs: 5_000,
    ...overrides,
  };
}

function payload(blackhawkPrices, blackhawkModifyLimit) {
  return {
    dataVersion: "v1",
    weaponNumericProfilesById: {},
    blackhawkPrices,
    blackhawkModifyLimit,
  };
}

function openSiteWorker(sb) {
  const siteWorker = new sb.page.Worker("/assets/optimizer.worker-abc.js", {
    type: "module",
  });
  const responses = [];
  siteWorker.onmessage = (event) => responses.push(event.data);
  return { siteWorker, responses };
}

test("blackhawk cap uses the cached payload for dataKey-only requests", async () => {
  const sb = createSandbox();
  const { siteWorker } = openSiteWorker(sb);

  siteWorker.postMessage({
    purpose: "search",
    dataKey: DATA_KEY,
    data: payload({ w1: 10_000 }, 500),
  });

  sb.hook.trigger("blackhawk");
  siteWorker.postMessage(siteRequest(1));

  await waitFor(() => sb.solveRequests.length > 0);
  const solve = sb.solveRequests[0];
  assert.equal(solve.options.costCap, 10_500);
  assert.equal(solve.options.mode, "blackhawk");
});

test("an explicit request payload still wins over the cached payload", async () => {
  const sb = createSandbox();
  const { siteWorker } = openSiteWorker(sb);

  siteWorker.postMessage({
    purpose: "search",
    dataKey: DATA_KEY,
    data: payload({ w1: 10_000 }, 500),
  });

  sb.hook.trigger("blackhawk");
  siteWorker.postMessage(
    siteRequest(2, { data: payload({ w1: 20_000 }, 0) }),
  );

  await waitFor(() => sb.solveRequests.length > 0);
  const solve = sb.solveRequests[0];
  assert.equal(solve.options.costCap, 20_000);
  assert.equal(solve.data.blackhawkPrices.w1, 20_000);
});

test("non-blackhawk requests without cached data still behave as before", async () => {
  const sb = createSandbox();
  const { siteWorker, responses } = openSiteWorker(sb);

  sb.hook.trigger("fast");
  siteWorker.postMessage(
    siteRequest(3, {
      dataKey: "missing-key",
      mode: "operations",
      data: undefined,
    }),
  );

  await waitFor(() => responses.length > 0);
  assert.match(responses[0].error, /missing weapon payload/);
});
