import test from "node:test";
import assert from "node:assert/strict";
import { createSandbox, waitFor } from "./harness.mjs";

function statusElement() {
  return { isConnected: true, textContent: "", style: {} };
}

function siteSolveRequest(requestId, overrides = {}) {
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
    ...overrides,
  };
}

test("trigger expiry reports that the site solver was not taken over", () => {
  const sb = createSandbox();
  const status = statusElement();
  sb.v.setStatusEl(status);

  sb.hook.trigger("full");
  assert.equal(sb.queuedTimers.size, 1);
  sb.runQueuedTimers();

  assert.match(status.textContent, /未接管/);
  assert.match(status.textContent, /站点原方案/);
  assert.doesNotMatch(status.textContent, /求解中/);
  assert.equal(status.style.color, "#fbbf24");
  assert.ok(
    sb.warnings.some((args) => /no site solve request captured/.test(args.join(" "))),
    "expected a console warning for the uncaptured request",
  );
});

test("a mismatched site solver worker URL reports the fallback instead of silence", () => {
  const sb = createSandbox();
  const status = statusElement();
  sb.v.setStatusEl(status);

  sb.hook.trigger("fast");
  const worker = new sb.page.Worker("/assets/solver.worker-abc.js", { type: "module" });

  assert.ok(Array.isArray(worker.received), "mismatched URL must yield the raw page worker");
  assert.match(status.textContent, /未接管/);
  assert.match(status.textContent, /地址不匹配/);
  assert.equal(status.style.color, "#fbbf24");
  assert.ok(
    sb.warnings.some((args) => /did not match/.test(args.join(" "))),
    "expected a console warning for the mismatched worker URL",
  );
});

test("unrelated workers created while waiting do not raise a false warning", () => {
  const sb = createSandbox();
  const status = statusElement();
  sb.v.setStatusEl(status);

  sb.hook.trigger("full");
  new sb.page.Worker("/assets/telemetry.js");

  assert.equal(status.textContent, "");
  assert.equal(sb.warnings.length, 0);
});

test("a failed Worker replacement is reported when the user triggers a solve", () => {
  const sb = createSandbox();
  const originalWorker = sb.page.Worker;
  Object.defineProperty(sb.page, "Worker", {
    configurable: true,
    get: () => originalWorker,
    set() {
      throw new Error("Worker is read-only");
    },
  });

  const failedHook = sb.v.installWorkerHook();
  const status = statusElement();
  sb.v.setStatusEl(status);

  failedHook.trigger("full");

  assert.match(status.textContent, /未接管/);
  assert.match(status.textContent, /无法替换站点 Worker/);
  assert.equal(status.style.color, "#fbbf24");
  assert.equal(sb.queuedTimers.size, 0);
  assert.ok(
    sb.warnings.some((args) => /worker hook was not installed/.test(args.join(" "))),
    "expected a console warning for the failed hook installation",
  );
});

test("a captured request suppresses the expiry warning", async () => {
  const sb = createSandbox({
    solverResult: { kind: "infeasible", optimal: false, truncated: false, alternatives: [] },
  });
  const status = statusElement();
  sb.v.setStatusEl(status);

  const siteWorker = new sb.page.Worker("/assets/optimizer.worker-abc.js", {
    type: "module",
  });
  const responses = [];
  siteWorker.onmessage = (event) => responses.push(event.data);

  sb.hook.trigger("fast");
  siteWorker.postMessage(siteSolveRequest(7));

  await waitFor(() => responses.length > 0);
  assert.equal(responses[0].requestId, 7);
  assert.ok(responses[0].result || responses[0].error);

  const captured = status.textContent;
  assert.doesNotMatch(captured, /未接管/);
  sb.runQueuedTimers();
  assert.equal(status.textContent, captured);
  assert.ok(!sb.warnings.some((args) => /no site solve request captured/.test(args.join(" "))));
});
