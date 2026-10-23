import test from "node:test";
import assert from "node:assert/strict";
import { createSandbox, waitFor } from "./harness.mjs";

function siteRequest(requestId, overrides = {}) {
  return {
    requestId,
    purpose: "solve",
    dataKey: "dk-bipod",
    data: { dataVersion: "v1", weaponNumericProfilesById: {} },
    weaponId: "w1",
    mode: "operations",
    laserEnabled: true,
    bipodDeployed: true,
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

test("solve always uses the optimizer baseline bipodDeployed=false", async () => {
  const sb = createSandbox();
  sb.page.localStorage.setItem(
    "moligod:gunsmith-draft:v1",
    JSON.stringify({ weaponId: "w1", mode: "operations", bipodDeployed: true }),
  );

  const siteWorker = new sb.page.Worker("/assets/optimizer.worker-abc.js", {
    type: "module",
  });
  sb.hook.trigger("full");
  siteWorker.postMessage(siteRequest(1));

  await waitFor(() => sb.solveRequests.length > 0);
  const solve = sb.solveRequests[0];
  assert.equal(solve.options.bipodDeployed, false);
  assert.equal(solve.context.bipodDeployed, true);

  await waitFor(() => sb.page.__moligodMinCostLast != null);
  assert.equal(sb.page.__moligodMinCostLast.options.bipodDeployed, false);
  assert.equal(sb.page.__moligodMinCostLast.options.mode, "full");
  assert.equal(solve.context.laserEnabled, true);
});

test("blackhawk solves also ignore the site bipod state", async () => {
  const sb = createSandbox();
  const siteWorker = new sb.page.Worker("/assets/optimizer.worker-abc.js", {
    type: "module",
  });
  sb.hook.trigger("blackhawk");
  siteWorker.postMessage(
    siteRequest(2, {
      mode: "blackhawk",
      data: {
        dataVersion: "v1",
        weaponNumericProfilesById: {},
        blackhawkPrices: { w1: 5_000 },
        blackhawkModifyLimit: 0,
      },
    }),
  );

  await waitFor(() => sb.solveRequests.length > 0);
  assert.equal(sb.solveRequests[0].options.bipodDeployed, false);
  assert.equal(sb.solveRequests[0].options.mode, "blackhawk");
});
