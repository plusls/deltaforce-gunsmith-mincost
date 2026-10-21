import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import vm from "node:vm";
import { buildBundle } from "../build.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const wasmPath = resolve(
  repo,
  "target/wasm32-unknown-unknown/release/delta_weapon_solver.wasm",
);
const skip = !existsSync(wasmPath) ? `missing ${wasmPath}` : false;

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

function testContext() {
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

function createScope() {
  const outbox = [];
  const waiters = [];
  const scope = {
    onmessage: null,
    postMessage(message) {
      outbox.push(message);
      for (const wake of waiters.splice(0)) wake();
    },
  };

  async function next(predicate = () => true, timeoutMs = 20_000) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const index = outbox.findIndex(predicate);
      if (index >= 0) return outbox.splice(index, 1)[0];
      const remaining = deadline - Date.now();
      if (remaining <= 0) throw new Error("timed out waiting for worker message");
      await new Promise((resolvePromise, rejectPromise) => {
        const timer = setTimeout(
          () => rejectPromise(new Error("timed out waiting for worker message")),
          remaining,
        );
        waiters.push(() => {
          clearTimeout(timer);
          resolvePromise();
        });
      });
    }
  }

  return { scope, next };
}

test("bundle replaces placeholders and parses as a userscript", { skip }, () => {
  const { bundle, workerSource, mainSource } = buildBundle();
  assert.match(bundle, /^\/\/ ==UserScript==/);
  assert.ok(!bundle.includes("__WASM_BASE64__"));
  assert.ok(!bundle.includes("__WORKER_SOURCE__"));
  assert.ok(bundle.length > 100_000);
  assert.doesNotThrow(() => new vm.Script(bundle));
  assert.doesNotThrow(() => new vm.Script(workerSource));
  assert.ok(!/^\s*import\s/m.test(workerSource));
  assert.ok(!/^\s*export\s/m.test(workerSource));
  assert.match(mainSource, /installWorkerHook/);
});

test("worker source initializes, caches payloads, and solves", { skip, timeout: 30_000 }, async () => {
  const { workerSource } = buildBundle();
  const { scope, next } = createScope();
  const sandbox = {
    self: scope,
    console,
    WebAssembly,
    TextEncoder,
    TextDecoder,
    setTimeout,
    clearTimeout,
  };
  vm.createContext(sandbox);
  vm.runInContext(workerSource, sandbox);

  await scope.onmessage({
    data: { type: "init", wasm: readFileSync(wasmPath) },
  });
  assert.equal((await next()).type, "ready");

  const options = {
    constraints: [{ id: "recoil-continuous-horizontal", max: 0.4 }],
  };
  await scope.onmessage({
    data: {
      type: "solve",
      id: 1,
      key: "test:w1:operations",
      data: payload,
      context: testContext(),
      options,
    },
  });
  const first = await next((message) => message.id === 1);
  assert.equal(first.type, "result");
  assert.equal(first.result.kind, "optimal");
  assert.equal(first.result.build.totalPrice, 1050);

  const cachedContext = testContext();
  await scope.onmessage({
    data: { type: "solve", id: 2, key: "test:w1:operations", context: cachedContext, options },
  });
  const second = await next((message) => message.id === 2);
  assert.equal(second.type, "result");
  assert.equal(second.result.build.totalPrice, 1050);
});

test("worker reports a missing payload", { skip, timeout: 30_000 }, async () => {
  const { workerSource } = buildBundle();
  const { scope, next } = createScope();
  const sandbox = {
    self: scope,
    console,
    WebAssembly,
    TextEncoder,
    TextDecoder,
    setTimeout,
    clearTimeout,
  };
  vm.createContext(sandbox);
  vm.runInContext(workerSource, sandbox);

  await scope.onmessage({
    data: { type: "init", wasm: readFileSync(wasmPath) },
  });
  assert.equal((await next()).type, "ready");

  await scope.onmessage({
    data: {
      type: "solve",
      id: 3,
      key: "missing",
      context: testContext(),
      options: {},
    },
  });
  const message = await next((entry) => entry.id === 3);
  assert.equal(message.type, "error");
  assert.match(message.message, /missing payload/);
});
