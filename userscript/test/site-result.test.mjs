import test from "node:test";
import assert from "node:assert/strict";
import { createSandbox, waitFor } from "./harness.mjs";

function solveRequest(requestId) {
  return {
    requestId,
    purpose: "solve",
    dataKey: "v1:w1",
    data: { dataVersion: "v1", weaponNumericProfilesById: {} },
    weaponId: "w1",
    mode: "operations",
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
  };
}

async function requestResult(solverResult) {
  const sb = createSandbox({ solverResult });
  const status = { isConnected: true, textContent: "", style: {} };
  sb.v.setStatusEl(status);

  const siteWorker = new sb.page.Worker("/assets/optimizer.worker-abc.js", {
    type: "module",
  });
  const responses = [];
  siteWorker.onmessage = (event) => responses.push(event.data);

  sb.hook.trigger("fast");
  siteWorker.postMessage(solveRequest(11));
  await waitFor(() => responses.length > 0);
  return { status, response: responses[0] };
}

test("a solver error is emitted as a site error instead of an infeasible result", async () => {
  const { status, response } = await requestResult({
    kind: "error",
    message: "boom",
    alternatives: [],
  });

  assert.equal(response.requestId, 11);
  assert.equal(response.result, undefined);
  assert.match(response.error, /求解失败/);
  assert.match(response.error, /boom/);
  assert.match(status.textContent, /求解失败/);
  assert.equal(status.style.color, "#f87171");
});

test("an incomplete result is emitted as a site error", async () => {
  const { response } = await requestResult({
    kind: "incomplete",
    optimal: false,
    truncated: false,
    incomplete: true,
    alternatives: [],
  });

  assert.equal(response.result, undefined);
  assert.match(response.error, /未证明无解/);
});

test("an infeasible result still produces a site result", async () => {
  const { response } = await requestResult({
    kind: "infeasible",
    optimal: false,
    truncated: false,
    alternatives: [],
  });

  assert.equal(response.error, undefined);
  assert.ok(response.result);
});
