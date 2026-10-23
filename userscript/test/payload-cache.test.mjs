import test from "node:test";
import assert from "node:assert/strict";
import {
  createSandbox,
  loadSolverWorker,
  loadWorkerArtifacts,
} from "./harness.mjs";

const payload = {
  dataVersion: "test",
  parts: {
    w1: { id: "w1", name: "Test Gun" },
    p1: {
      id: "p1",
      name: "P1",
      rawRecoilMultipliersByMode: {
        operations: { horizontal: -0.2, vertical: -0.2 },
      },
    },
    p2: {
      id: "p2",
      name: "P2",
      mainAttributeModifiersByMode: {
        operations: [
          { attributeId: "10005", modifierType: "Addend", delta: 50 },
        ],
      },
      rawRecoilMultipliersByMode: {
        operations: { horizontal: -0.5, vertical: -0.5 },
      },
    },
  },
  slotsByHost: {
    w1: [{ id: "1", name: "Muzzle_point", candidateIds: ["p1", "p2"] }],
  },
  defaultPresetsByWeapon: {
    w1: { nodes: [{ isRoot: true, partId: "w1" }] },
  },
  virtualRulesByWeapon: { w1: [] },
  candidateOverridesByWeapon: {},
  weaponStatsById: {
    w1: {
      byMode: {
        operations: {
          base: { 10001: 50, 10005: 50, 10006: 50, 10007: 50, 10008: 50 },
          display: { 10001: 50, 10005: 50, 10006: 50, 10007: 50, 10008: 50 },
        },
      },
    },
  },
  weaponNumericProfilesById: {
    w1: {
      linkedTables: {
        aimRecoil: {},
        recoilCurves: {
          recoilControl: {
            keys: [
              { Time: 0, Value: 1.0 },
              { Time: 100, Value: 0.5 },
            ],
          },
          gunkick: {
            keys: [
              { Time: 0, Value: 1.0 },
              { Time: 100, Value: 0.5 },
            ],
          },
        },
      },
    },
  },
};

function workerContext() {
  return {
    dataVersion: "test",
    weaponId: "w1",
    mode: "operations",
    laserEnabled: true,
    pricesByObjectId: {
      w1: { currentPrice: 1000, systemPrice: 1000 },
      p1: { currentPrice: 100, systemPrice: 100 },
      p2: { currentPrice: 50, systemPrice: 50 },
    },
    currentSelections: {},
    tuningState: {},
    lockedPathKeys: [],
    lockedTuningKeys: [],
    excludedPartIds: [],
  };
}

const options = { constraints: [{ id: "recoil-continuous-horizontal", max: 0.4 }] };

function assertBoundedLru(cache, MapClass) {
  assert.ok(cache instanceof MapClass);
  for (let index = 1; index <= 4; index += 1) cache.set(`k${index}`, { index });
  assert.equal(cache.size, 4);

  cache.get("k1");
  cache.set("k5", { index: 5 });

  assert.equal(cache.size, 4);
  assert.equal(cache.get("k1").index, 1);
  assert.equal(cache.has("k2"), false);
  assert.equal(cache.get("k5").index, 5);
  assert.equal(cache.size, 4);
}

test("main-thread payload cache is bounded and keeps recent keys", () => {
  const sb = createSandbox();
  assertBoundedLru(sb.v.payloadCache, sb.vmMap);
});

test("site payload cache is bounded and keeps recent keys", () => {
  const sb = createSandbox();
  assertBoundedLru(sb.hook.sitePayloadCache, sb.vmMap);
});

test("missing site payload is only reused while it is still cached", () => {
  const sb = createSandbox();
  const siteWorker = new sb.page.Worker("/assets/optimizer.worker-abc.js");
  const real = sb.workers.find((worker) => worker.url.includes("optimizer.worker"));

  siteWorker.postMessage({ purpose: "search", dataKey: "dk1", data: { marker: 1 } });
  siteWorker.postMessage({ purpose: "search", dataKey: "dk1" });
  assert.deepEqual(real.received.at(-1).data, { marker: 1 });

  for (let index = 0; index < 4; index += 1) {
    siteWorker.postMessage({
      purpose: "search",
      dataKey: `dk-extra-${index}`,
      data: { marker: index },
    });
  }
  siteWorker.postMessage({ purpose: "search", dataKey: "dk1" });
  assert.equal(real.received.at(-1).data, undefined);
});

const artifacts = loadWorkerArtifacts();
const skip = !artifacts ? "missing wasm artifact" : false;

test("worker payload cache evicts the least recently used payload", { skip, timeout: 30_000 }, async () => {
  const { scope, next } = loadSolverWorker(artifacts.workerSource);
  await scope.onmessage({ data: { type: "init", wasm: artifacts.wasm } });
  assert.equal((await next()).type, "ready");

  for (let index = 1; index <= 5; index += 1) {
    await scope.onmessage({
      data: {
        type: "solve",
        id: index,
        key: `k${index}`,
        data: payload,
        context: workerContext(),
        options,
      },
    });
    const message = await next((entry) => entry.id === index);
    assert.equal(message.type, "result");
  }

  await scope.onmessage({
    data: { type: "solve", id: 6, key: "k5", context: workerContext(), options },
  });
  assert.equal((await next((entry) => entry.id === 6)).type, "result");

  await scope.onmessage({
    data: { type: "solve", id: 7, key: "k1", context: workerContext(), options },
  });
  const evicted = await next((entry) => entry.id === 7);
  assert.equal(evicted.type, "error");
  assert.match(evicted.message, /missing payload/);
});
