import test from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import {
  createMinCostEngine,
  decodeBase64,
  normalizeSolveOptions,
} from "./engine.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "../..");
const wasmPath = resolve(
  repo,
  "target/wasm32-unknown-unknown/release/gunsmith_mincost.wasm",
);

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
          base: {
            10001: 50,
            10005: 50,
            10006: 50,
            10007: 50,
            10008: 50,
          },
          display: {
            10001: 50,
            10005: 50,
            10006: 50,
            10007: 50,
            10008: 50,
          },
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

function context(overrides = {}) {
  return {
    dataVersion: "test",
    weaponId: "w1",
    mode: "operations",
    laserEnabled: true,
    data: payload,
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
    ...overrides,
  };
}

const constraint = [{ id: "recoil-continuous-horizontal", max: 0.4 }];

test("normalizeSolveOptions defaults to full mode", () => {
  const options = normalizeSolveOptions();
  assert.equal(options.mode, "full");
  assert.equal(options.deadlineMs, 5000);
  assert.equal(options.constraints.length, 0);
});

test("decodeBase64 round trips bytes", () => {
  const bytes = decodeBase64(Buffer.from([0, 1, 127, 255]).toString("base64"));
  assert.deepEqual([...bytes], [0, 1, 127, 255]);
});

test("engine solves in full mode", async (t) => {
  if (!existsSync(wasmPath)) {
    t.skip(`missing ${wasmPath}; build the wasm target first`);
    return;
  }
  const engine = await createMinCostEngine({ wasm: readFileSync(wasmPath) });
  const result = engine.solve(context(), { constraints: constraint });
  assert.equal(result.kind, "optimal");
  assert.equal(result.optimal, true);
  assert.equal(result.truncated, false);
  assert.equal(result.build.totalPrice, 1050);
  assert.equal(result.build.attributes["10005"], 50);
  assert.deepEqual(result.build.installedPartKeys, [
    "weapon:w1/slot:1:Muzzle_point\0p2",
  ]);
  assert.equal(result.build.selections["weapon:w1/slot:1:Muzzle_point"], "p2");
  assert.ok(result.elapsedMs >= 0);
});

test("engine fast mode skips the optimality claim", async (t) => {
  if (!existsSync(wasmPath)) {
    t.skip(`missing ${wasmPath}; build the wasm target first`);
    return;
  }
  const engine = await createMinCostEngine({ wasm: readFileSync(wasmPath) });
  const result = engine.solve(context(), {
    constraints: constraint,
    mode: "fast",
  });
  assert.equal(result.kind, "feasible");
  assert.equal(result.optimal, false);
  assert.equal(result.build.totalPrice, 1050);
});

test("engine returns infeasible when no build meets the constraint", async (t) => {
  if (!existsSync(wasmPath)) {
    t.skip(`missing ${wasmPath}; build the wasm target first`);
    return;
  }
  const engine = await createMinCostEngine({ wasm: readFileSync(wasmPath) });
  const result = engine.solve(context(), {
    constraints: [{ id: "recoil-continuous-horizontal", max: 0.1 }],
  });
  assert.equal(result.kind, "infeasible");
  assert.equal(result.build, undefined);
});

test("engine accepts a site request through the adapter", async (t) => {
  if (!existsSync(wasmPath)) {
    t.skip(`missing ${wasmPath}; build the wasm target first`);
    return;
  }
  const engine = await createMinCostEngine({ wasm: readFileSync(wasmPath) });
  const siteRequest = {
    targets: { budgetEnabled: false, preparednessEnabled: false },
    smartOptions: {
      preferenceConditions: {},
      advancedPreferenceTargets: {
        "recoil-continuous-horizontal": { operator: "lte", value: 0.4 },
      },
    },
  };
  const result = engine.solveSiteRequest(context(), siteRequest);
  assert.equal(result.kind, "optimal");
  assert.equal(result.build.totalPrice, 1050);
});
