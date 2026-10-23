import test from "node:test";
import assert from "node:assert/strict";

import {
  UnsupportedConstraintError,
  UnsupportedFeatureError,
  SUPPORTED_CONSTRAINT_IDS,
  PRIMARY_ATTR_BY_GROUP,
  captureWorkerRequest,
  optionsFromSiteRequest,
  toSolveRequest,
  fromSolverOutput,
  toSiteResult,
} from "./site-adapter.mjs";

const WEAPON_DATA = {
  dataVersion: "7e60debf7b978ebec618a142",
  parts: {},
  slotsByHost: {},
  defaultPresetsByWeapon: {},
  virtualRulesByWeapon: {},
};

function deepFreeze(value) {
  if (value === null || typeof value !== "object" || Object.isFrozen(value)) {
    return value;
  }
  for (const key of Object.keys(value)) deepFreeze(value[key]);
  return Object.freeze(value);
}

function makeContext(overrides = {}) {
  return {
    dataVersion: "v1",
    weaponId: "18010000049",
    mode: "operations",
    laserEnabled: false,
    data: WEAPON_DATA,
    pricesByObjectId: {},
    currentSelections: {},
    tuningState: {},
    lockedPathKeys: [],
    lockedTuningKeys: [],
    excludedPartIds: [],
    ...overrides,
  };
}

test("exports supported ids and primary attribute mapping", () => {
  assert.ok(SUPPORTED_CONSTRAINT_IDS.has("recoil-continuous-horizontal"));
  assert.ok(SUPPORTED_CONSTRAINT_IDS.has("10001"));
  assert.ok(SUPPORTED_CONSTRAINT_IDS.has("10008"));
  assert.ok(SUPPORTED_CONSTRAINT_IDS.has("handling-aim-time"));
  assert.ok(SUPPORTED_CONSTRAINT_IDS.has("hipfire-maximum-spread"));
  assert.ok(!SUPPORTED_CONSTRAINT_IDS.has("handling-unknown"));
  assert.deepEqual(PRIMARY_ATTR_BY_GROUP, {
    recoil: "10005",
    handling: "10006",
    stability: "10007",
    hipfire: "10008",
  });
});

test("captureWorkerRequest caches payload by dataKey and throws when missing", () => {
  const cache = new Map();
  const message = {
    dataKey: "v1:18010000049",
    data: WEAPON_DATA,
    weaponId: "18010000049",
    mode: "warfare",
    laserEnabled: true,
    pricesByObjectId: { a: { currentPrice: 10, systemPrice: 5 } },
    currentSelections: { "": "18010000049" },
    tuningState: { k: 50 },
    lockedPathKeys: ["p1"],
    lockedTuningKeys: ["k"],
    excludedPartIds: ["x"],
  };

  const context = captureWorkerRequest(message, cache);
  assert.equal(cache.get("v1:18010000049"), WEAPON_DATA);
  assert.equal(context.data, WEAPON_DATA);
  assert.equal(context.dataVersion, "7e60debf7b978ebec618a142");
  assert.equal(context.mode, "warfare");
  assert.equal(context.laserEnabled, true);
  assert.deepEqual(context.lockedPathKeys, ["p1"]);
  assert.deepEqual(context.excludedPartIds, ["x"]);

  const cached = captureWorkerRequest({ dataKey: "v1:18010000049" }, cache);
  assert.equal(cached.data, WEAPON_DATA);
  assert.equal(cached.dataVersion, "7e60debf7b978ebec618a142");
  assert.equal(cached.mode, "operations");
  assert.equal(cached.laserEnabled, false);
  assert.deepEqual(cached.pricesByObjectId, {});
  assert.deepEqual(cached.lockedPathKeys, []);

  assert.throws(
    () => captureWorkerRequest({ dataKey: "other" }, cache),
    /missing weapon payload for dataKey other/,
  );
});

test("preferenceConditions gte maps to min on the primary attribute", () => {
  const options = optionsFromSiteRequest({
    targets: {},
    smartOptions: {
      preferenceConditions: {
        recoil: { join: "and", operator: "gte", value: 48 },
        handling: { join: "and", operator: "gte", value: null },
      },
    },
  });

  assert.deepEqual(options.constraints, [{ id: "10005", min: 48 }]);
});

test("preferenceConditions lte and eq map to max and both", () => {
  const options = optionsFromSiteRequest({
    targets: {},
    smartOptions: {
      preferenceConditions: {
        hipfire: { operator: "lte", value: 24 },
        stability: { operator: "eq", value: 71 },
      },
    },
  });

  assert.deepEqual(options.constraints, [
    { id: "10007", min: 71, max: 71 },
    { id: "10008", max: 24 },
  ]);
});

test("advancedPreferenceTargets lte on a supported metric maps to max", () => {
  const options = optionsFromSiteRequest({
    targets: {},
    smartOptions: {
      advancedPreferenceTargets: {
        "recoil-continuous-vertical": { operator: "lte", value: 0.46 },
        "recoil-fire-stability": { operator: "gte", value: 0.8 },
      },
    },
  });

  assert.deepEqual(options.constraints, [
    { id: "recoil-continuous-vertical", max: 0.46 },
    { id: "recoil-fire-stability", min: 0.8 },
  ]);
});

test("unsupported advanced metric throws UnsupportedConstraintError", () => {
  const request = {
    targets: {},
    smartOptions: {
      advancedPreferenceTargets: {
        "handling-unknown": { operator: "lte", value: 300 },
        "recoil-continuous-vertical": { operator: "lte", value: 0.46 },
        "stability-unknown": { operator: "gte", value: 4 },
      },
    },
  };

  assert.throws(
    () => optionsFromSiteRequest(request),
    (error) => {
      assert.ok(error instanceof UnsupportedConstraintError);
      assert.deepEqual(error.constraints, ["handling-unknown", "stability-unknown"]);
      return true;
    },
  );

  const ignored = optionsFromSiteRequest(request, { ignoreUnsupported: true });
  assert.deepEqual(ignored.constraints, [
    { id: "recoil-continuous-vertical", max: 0.46 },
  ]);
  assert.equal(Object.hasOwn(ignored, "ignoreUnsupported"), false);
});

test("all site advanced metrics map to constraints", () => {
  const options = optionsFromSiteRequest({
    targets: {},
    smartOptions: {
      advancedPreferenceTargets: {
        "handling-aim-time": { operator: "lte", value: 300 },
        "stability-breath-duration": { operator: "gte", value: 4 },
        "hipfire-standing-spread": { operator: "lte", value: 2 },
      },
    },
  });
  assert.deepEqual(options.constraints, [
    { id: "handling-aim-time", max: 300 },
    { id: "stability-breath-duration", min: 4 },
    { id: "hipfire-standing-spread", max: 2 },
  ]);
});

test("legacy advancedPreferences shape is honored", () => {
  const options = optionsFromSiteRequest({
    targets: {},
    smartOptions: {
      advancedPreferences: {
        "recoil-single-horizontal": "custom",
        "handling-aim-time": "custom",
        "recoil-continuous-horizontal": "none",
      },
      advancedPreferenceValues: {
        "recoil-single-horizontal": 0.5,
        "handling-aim-time": 250,
        "recoil-continuous-horizontal": 0.9,
      },
      advancedPreferenceOperators: {
        "recoil-single-horizontal": "lte",
        "handling-aim-time": "lte",
      },
    },
  }, { ignoreUnsupported: true });

  assert.deepEqual(options.constraints, [
    { id: "recoil-single-horizontal", max: 0.5 },
    { id: "handling-aim-time", max: 250 },
  ]);
});

test("preparednessEnabled throws UnsupportedFeatureError", () => {
  assert.throws(
    () => optionsFromSiteRequest({ targets: { preparednessEnabled: true } }),
    (error) => {
      assert.ok(error instanceof UnsupportedFeatureError);
      assert.equal(error.feature, "preparedness");
      return true;
    },
  );
});

test("budget maps to costCap and override wins", () => {
  const request = {
    targets: { budgetEnabled: true, budgetPrice: 123456.7 },
    smartOptions: {},
  };

  assert.equal(optionsFromSiteRequest(request).costCap, 123457);
  assert.equal(optionsFromSiteRequest(request, { costCap: 5000 }).costCap, 5000);
  assert.equal(
    optionsFromSiteRequest({
      targets: { budgetEnabled: false, budgetPrice: 123456.7 },
    }).costCap,
    null,
  );
});

test("blackhawk budget mirrors the site worker's forced cap", () => {
  const request = (targets = {}) => ({
    mode: "blackhawk",
    weaponId: "18010000008",
    data: {
      blackhawkPrices: { "18010000008": 1000, "13140000035": 40 },
      blackhawkModifyLimit: 600,
    },
    targets,
    smartOptions: {},
  });

  assert.equal(optionsFromSiteRequest(request()).costCap, 1600);
  assert.equal(
    optionsFromSiteRequest(request({ budgetEnabled: false, budgetPrice: 100000 }))
      .costCap,
    1600,
  );
  assert.equal(
    optionsFromSiteRequest(request({ budgetEnabled: true, budgetPrice: 100000 }))
      .costCap,
    1600,
  );
  assert.equal(
    optionsFromSiteRequest(request({ budgetEnabled: true, budgetPrice: 1200 }))
      .costCap,
    1200,
  );
  assert.equal(
    optionsFromSiteRequest(request({ budgetEnabled: true, budgetPrice: 100000 }), {
      costCap: 5000,
    }).costCap,
    5000,
  );
});

test("blackhawk budget needs weapon data before it can cap the budget", () => {
  const options = optionsFromSiteRequest({
    mode: "blackhawk",
    weaponId: "18010000008",
    targets: { budgetEnabled: true, budgetPrice: 100000 },
    smartOptions: {},
  });
  assert.equal(options.costCap, 100000);
});

test("operations mode still ignores the blackhawk cap", () => {
  const options = optionsFromSiteRequest({
    mode: "operations",
    weaponId: "18010000008",
    data: {
      blackhawkPrices: { "18010000008": 1000 },
      blackhawkModifyLimit: 600,
    },
    targets: {},
    smartOptions: {},
  });
  assert.equal(options.costCap, null);
});

test("site timeBudgetMs seeds deadlineMs and overrides win", () => {
  const request = { targets: {}, smartOptions: {}, timeBudgetMs: 15_000 };

  assert.equal(optionsFromSiteRequest(request).deadlineMs, 15_000);
  assert.equal(
    optionsFromSiteRequest(request, { deadlineMs: 60_000 }).deadlineMs,
    60_000,
  );
  assert.equal(
    optionsFromSiteRequest({ targets: {}, timeBudgetMs: 0 }).deadlineMs,
    5_000,
  );
  assert.equal(
    optionsFromSiteRequest({ targets: {}, timeBudgetMs: "bad" }).deadlineMs,
    5_000,
  );
  assert.equal(
    optionsFromSiteRequest({ targets: {}, timeBudgetMs: 1_000_000_000 })
      .deadlineMs,
    300_000,
  );
  assert.equal(
    optionsFromSiteRequest({
      targets: {},
      timeBudgetMs: Number.POSITIVE_INFINITY,
    }).deadlineMs,
    5_000,
  );
});

test("overrides replace mapped constraints and unknown overrides are dropped", () => {
  const options = optionsFromSiteRequest(
    {
      targets: {},
      smartOptions: {
        preferenceConditions: { recoil: { operator: "gte", value: 48 } },
      },
    },
    { constraints: [{ id: "10001", min: 55 }], mode: "fast", somethingElse: 1 },
  );

  assert.deepEqual(options.constraints, [{ id: "10001", min: 55 }]);
  assert.equal(options.mode, "fast");
  assert.equal(Object.hasOwn(options, "somethingElse"), false);
  assert.deepEqual(optionsFromSiteRequest({}).constraints, []);
});

test("price source selects current or system prices", () => {
  const context = makeContext({
    pricesByObjectId: {
      a: { currentPrice: 100.4, systemPrice: 50.6 },
      b: { currentPrice: 80, systemPrice: 20 },
    },
  });

  const current = toSolveRequest(context, {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  });
  assert.deepEqual(current.prices, { a: 100, b: 80 });

  const system = toSolveRequest(context, {
    priceSource: "system",
    lockMode: "locked",
    missingPricePolicy: "reject",
  });
  assert.deepEqual(system.prices, { a: 51, b: 20 });
});

test("blackhawk mode takes blackhawkPrices instead of market prices", () => {
  const context = makeContext({
    mode: "blackhawk",
    weaponId: "18060000009",
    data: {
      ...WEAPON_DATA,
      blackhawkPrices: {
        "18060000009": 1800,
        "13110000057": 40,
        "13020000443": 160.4,
      },
    },
    pricesByObjectId: {
      "18060000009": { currentPrice: 102000, systemPrice: 90000 },
      "13110000057": { currentPrice: 82309, systemPrice: 80000 },
      "13020000443": { currentPrice: 150664, systemPrice: 140000 },
    },
  });

  const request = toSolveRequest(context, {
    priceSource: "system",
    lockMode: "locked",
    missingPricePolicy: "fallback-system",
  });

  assert.equal(request.mode, "blackhawk");
  assert.deepEqual(request.prices, {
    "18060000009": 1800,
    "13110000057": 40,
    "13020000443": 160,
  });
});

test("blackhawk prices are keyed by officialObjectId when present", () => {
  const context = makeContext({
    mode: "blackhawk",
    data: {
      ...WEAPON_DATA,
      parts: { p1: { id: "p1", officialObjectId: "official-1" } },
      blackhawkPrices: { p1: 55 },
    },
  });

  const request = toSolveRequest(context, {
    lockMode: "locked",
    missingPricePolicy: "reject",
  });
  assert.deepEqual(request.prices, { "official-1": 55 });
});

test("blackhawk mode only excludes absent or non-numeric prices", () => {
  const context = makeContext({
    mode: "blackhawk",
    excludedPartIds: ["pre"],
    data: {
      ...WEAPON_DATA,
      blackhawkPrices: {
        good: 40,
        zero: 0,
        negative: -5,
        nullPrice: null,
        nanPrice: Number.NaN,
        infinitePrice: Number.POSITIVE_INFINITY,
        stringPrice: "40",
      },
    },
    pricesByObjectId: {
      good: { currentPrice: 100, systemPrice: 90 },
      zero: { currentPrice: 70, systemPrice: 60 },
    },
  });

  const request = toSolveRequest(context, {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "fallback-system",
  });

  // The site's `sn()` treats a stored 0 as a real price and only an absent key
  // as missing, so zero and negative entries must survive the conversion.
  assert.deepEqual(request.prices, { good: 40, zero: 0, negative: -5 });
  assert.deepEqual(request.excludedPartIds, [
    "pre",
    "nullPrice",
    "nanPrice",
    "infinitePrice",
    "stringPrice",
  ]);
});

test("blackhawk mode without usable weapon data throws", () => {
  const options = {
    lockMode: "locked",
    missingPricePolicy: "reject",
  };

  assert.throws(
    () => toSolveRequest(makeContext({ mode: "blackhawk", data: null }), options),
    /blackhawk mode requires context\.data/,
  );
  assert.throws(
    () => toSolveRequest(makeContext({ mode: "blackhawk", data: {} }), options),
    /blackhawk mode requires data\.blackhawkPrices/,
  );
});

test("warfare mode keeps using current and system market prices", () => {
  const context = makeContext({
    mode: "warfare",
    data: {
      ...WEAPON_DATA,
      blackhawkPrices: { a: 1, b: 2 },
    },
    pricesByObjectId: {
      a: { currentPrice: 100.4, systemPrice: 50.6 },
      b: { currentPrice: 80, systemPrice: 20 },
    },
  });

  assert.deepEqual(
    toSolveRequest(context, {
      priceSource: "current",
      lockMode: "locked",
      missingPricePolicy: "reject",
    }).prices,
    { a: 100, b: 80 },
  );
  assert.deepEqual(
    toSolveRequest(context, {
      priceSource: "system",
      lockMode: "locked",
      missingPricePolicy: "reject",
    }).prices,
    { a: 51, b: 20 },
  );
});

test("missing current prices are excluded by default and fallback uses system price", () => {
  const context = makeContext({
    excludedPartIds: ["pre"],
    pricesByObjectId: {
      good: { currentPrice: 100, systemPrice: 90 },
      noCurrent: { currentPrice: null, systemPrice: 60 },
      zeroCurrent: { currentPrice: 0, systemPrice: 70 },
      noPrice: { currentPrice: 0, systemPrice: 0 },
      negative: { currentPrice: -5, systemPrice: -1 },
    },
  });

  const strict = toSolveRequest(context, {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  });
  assert.deepEqual(strict.prices, { good: 100 });
  assert.deepEqual(strict.excludedPartIds, [
    "pre",
    "noCurrent",
    "zeroCurrent",
    "noPrice",
    "negative",
  ]);

  const fallback = toSolveRequest(context, {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "fallback-system",
  });
  assert.deepEqual(fallback.prices, {
    good: 100,
    noCurrent: 60,
    zeroCurrent: 70,
  });
  assert.deepEqual(fallback.excludedPartIds, ["pre", "noPrice", "negative"]);

  const system = toSolveRequest(context, {
    priceSource: "system",
    lockMode: "locked",
    missingPricePolicy: "reject",
  });
  assert.deepEqual(system.prices, {
    good: 90,
    noCurrent: 60,
    zeroCurrent: 70,
  });
  assert.deepEqual(system.excludedPartIds, ["pre", "noPrice", "negative"]);
});

test("missing prices exclude the part id, not the official object id", () => {
  const context = makeContext({
    data: {
      ...WEAPON_DATA,
      parts: {
        p1: { id: "p1", officialObjectId: "official-1" },
        p2: { id: "p2", officialObjectId: "official-2" },
        p3: { id: "p3" },
      },
    },
    pricesByObjectId: {
      "official-1": { currentPrice: 100, systemPrice: 90 },
      "official-2": { currentPrice: 0, systemPrice: 0 },
      p3: { currentPrice: null, systemPrice: 0 },
      stray: { currentPrice: 0, systemPrice: 0 },
    },
  });

  const request = toSolveRequest(context, {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  });

  assert.deepEqual(request.prices, { "official-1": 100 });
  assert.deepEqual(request.excludedPartIds, ["p2", "p3", "stray"]);
});

test("missing prices never leave an unpriced selectable part", () => {
  const parts = {
    p1: { id: "p1", officialObjectId: "official-1" },
    p2: { id: "p2", officialObjectId: "official-2" },
  };
  const context = makeContext({
    data: { ...WEAPON_DATA, parts },
    pricesByObjectId: {
      "official-1": { currentPrice: 0, systemPrice: 0 },
      "official-2": { currentPrice: 10, systemPrice: 10 },
    },
  });

  const request = toSolveRequest(context, {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  });

  const pricedPartIds = new Set(request.excludedPartIds);
  for (const part of Object.values(parts)) {
    const priceId = part.officialObjectId ?? part.id;
    if (priceId in request.prices) continue;
    assert.ok(
      pricedPartIds.has(part.id),
      `part ${part.id} has no price and is not excluded`,
    );
  }
});

test("locked paths and tuning map to solver objects", () => {
  const context = makeContext({
    currentSelections: { s1: "p1", s2: "p2", s3: "" },
    lockedPathKeys: ["s1", "s4"],
    tuningState: { k1: 10, k2: 20, k3: "bad" },
    lockedTuningKeys: ["k1", "k2", "k3"],
  });
  const options = {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  };

  const locked = toSolveRequest(context, options);
  assert.deepEqual(locked.lockedPathKeys, { s1: "p1", s4: null });
  assert.deepEqual(locked.lockedTuningKeys, { k1: 10, k2: 20 });

  const current = toSolveRequest(context, { ...options, lockMode: "current" });
  assert.deepEqual(current.lockedPathKeys, { s1: "p1", s4: null, s2: "p2" });
});

test("mode maps to firstSolution", () => {
  const context = makeContext();
  const base = {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  };

  assert.equal(toSolveRequest(context, { ...base, mode: "fast" }).firstSolution, true);
  assert.equal(toSolveRequest(context, { ...base, mode: "full" }).firstSolution, false);

  const request = toSolveRequest(context, {
    ...base,
    mode: "fast",
    maxResults: 5,
    maxNodes: 10,
    maxLeaves: 20,
    deadlineMs: 100,
    costCap: 999,
  });
  assert.deepEqual(request, {
    prices: {},
    weaponId: "18010000049",
    mode: "operations",
    laserEnabled: false,
    bipodDeployed: false,
    lockedPathKeys: {},
    lockedTuningKeys: {},
    excludedPartIds: [],
    constraints: [],
    maxResults: 5,
    maxEvaluations: 10,
    maxLeaves: 20,
    costCap: 999,
    firstSolution: true,
    deadlineMs: 100,
  });
});

test("bipodDeployed flows through capture, options, and the solve request", () => {
  const cache = new Map();
  const context = captureWorkerRequest(
    { dataKey: "v1:18010000049", data: WEAPON_DATA, bipodDeployed: true },
    cache,
  );
  assert.equal(context.bipodDeployed, true);
  assert.equal(
    captureWorkerRequest({ dataKey: "v1:18010000049" }, cache).bipodDeployed,
    false,
  );

  const base = {
    priceSource: "current",
    lockMode: "locked",
    missingPricePolicy: "reject",
  };
  assert.equal(toSolveRequest(context, base).bipodDeployed, true);
  assert.equal(
    toSolveRequest(context, { ...base, bipodDeployed: false }).bipodDeployed,
    false,
  );

  const options = optionsFromSiteRequest(
    { targets: {}, bipodDeployed: true },
    {},
  );
  assert.equal(options.bipodDeployed, true);
  assert.equal(
    toSolveRequest(makeContext({ bipodDeployed: false }), options).bipodDeployed,
    true,
  );
  assert.equal(
    optionsFromSiteRequest(
      { targets: {}, bipodDeployed: true },
      { bipodDeployed: false },
    ).bipodDeployed,
    false,
  );
  assert.equal(
    Object.hasOwn(optionsFromSiteRequest({ targets: {} }), "bipodDeployed"),
    false,
  );
});

test("fromSolverOutput maps optimal and feasible results", () => {
  const raw = {
    optimal: true,
    truncated: false,
    nodes: 100,
    leaves: 200,
    message: "done",
    best: {
      selections: { s1: "p1", s2: null, s3: 7, s4: "p4" },
      tuning: { "s1\0p1\0t": 50 },
      installed_parts: [
        { slot_path: "s2", part_id: "p2" },
        { slot_path: "s1", part_id: "p1" },
      ],
      panel: { 10005: 71 },
      detail: { aim_time: 320.5, hipfire_standing: 2.25, unknown: 1 },
      recoil: { score: 0.5 },
      total_price: 1234,
      attachment_price: 1000,
    },
    alternatives: [
      {
        selections: { s1: "alt" },
        tuning: {},
        installed_part_keys: ["s1\0alt"],
        panel: { 10005: 60 },
        recoil: {},
        total_price: 2000,
        attachment_price: 1800,
      },
    ],
  };

  const mapped = fromSolverOutput(raw);
  assert.equal(mapped.kind, "optimal");
  assert.equal(mapped.optimal, true);
  assert.equal(mapped.truncated, false);
  assert.equal(mapped.nodes, 100);
  assert.equal(mapped.leaves, 200);
  assert.equal(mapped.elapsedMs, 0);
  assert.equal(mapped.message, "done");
  assert.deepEqual(mapped.build.selections, { s1: "p1", s4: "p4" });
  assert.deepEqual(mapped.build.tuning, { "s1\0p1\0t": 50 });
  assert.deepEqual(mapped.build.installedPartKeys, ["s1\0p1", "s2\0p2"]);
  assert.deepEqual(mapped.build.attributes, { 10005: 71 });
  assert.deepEqual(mapped.build.detail, {
    aim_time: 320.5,
    hipfire_standing: 2.25,
    unknown: 1,
  });
  assert.deepEqual(mapped.build.recoil, { score: 0.5 });
  assert.equal(mapped.build.totalPrice, 1234);
  assert.equal(mapped.build.attachmentPrice, 1000);
  assert.equal(mapped.alternatives.length, 1);
  assert.deepEqual(mapped.alternatives[0].installedPartKeys, ["s1\0alt"]);

  const feasible = fromSolverOutput({ ...raw, optimal: false });
  assert.equal(feasible.kind, "feasible");
});

test("fromSolverOutput maps infeasible and truncated results", () => {
  const infeasible = fromSolverOutput({
    optimal: false,
    truncated: false,
    nodes: 5,
    leaves: 6,
    message: "no solution",
    best: null,
    alternatives: [],
  });
  assert.equal(infeasible.kind, "infeasible");
  assert.equal(infeasible.build, undefined);
  assert.equal(infeasible.message, "no solution");
  assert.deepEqual(infeasible.alternatives, []);

  const truncated = fromSolverOutput({
    optimal: false,
    truncated: true,
    nodes: 9,
    leaves: 10,
    message: "deadline",
    best: null,
  });
  assert.equal(truncated.kind, "error");
  assert.equal(
    truncated.message,
    "time budget exhausted before a feasible build was found",
  );
});

test("toSiteResult produces a recommendation with a candidate", () => {
  const context = makeContext({
    currentSelections: { s1: "old", s2: null },
  });
  const result = {
    kind: "optimal",
    optimal: true,
    truncated: false,
    nodes: 42,
    leaves: 84,
    elapsedMs: 123,
    message: "done",
    build: {
      selections: { s1: "p1", s2: null, s3: "p3" },
      tuning: { "s1\0p1\0t": 30 },
      installedPartKeys: ["s1\0p1", "s3\0p3"],
      attributes: { 10005: 71, 10006: 35 },
      detail: {
        aim_time: 320.5,
        sprint_fire_delay: null,
        stability_score: 42,
      },
      recoil: {},
      totalPrice: 1000,
      attachmentPrice: 900,
    },
  };

  const site = toSiteResult(context, result);
  assert.equal(site.smartRecommendation.kind, "recommendation");
  assert.equal(site.allTargetsSatisfied, true);
  assert.equal(site.truncated, false);
  assert.equal(site.evaluatedStates, 42);
  assert.equal(site.generatedStates, 84);
  assert.equal(site.elapsedMs, 123);
  assert.deepEqual(site.selections, { s1: "old" });

  const candidate = site.smartRecommendation.candidate;
  assert.deepEqual(candidate.selections, { s1: "p1", s3: "p3" });
  assert.deepEqual(candidate.tuningState, { "s1\0p1\0t": 30 });
  assert.deepEqual(candidate.installedPartKeys, ["s1\0p1", "s3\0p3"]);
  assert.deepEqual(candidate.metrics.attributes, { 10005: 71, 10006: 35 });
  assert.deepEqual(candidate.metrics.detailMetrics, {
    "handling-aim-time": 320.5,
  });
  assert.deepEqual(site.candidates, [candidate]);
  assert.deepEqual(site.smartRecommendation.failures, []);
});

test("toSiteResult produces an infeasible result with failures", () => {
  const context = makeContext({ currentSelections: { s1: "old" } });
  const site = toSiteResult(context, {
    kind: "infeasible",
    optimal: false,
    truncated: false,
    nodes: 3,
    leaves: 4,
    elapsedMs: 0,
    message: "no solution",
  });

  assert.equal(site.smartRecommendation.kind, "infeasible");
  assert.equal(site.smartRecommendation.candidate, undefined);
  assert.deepEqual(site.smartRecommendation.failures, [
    { kind: "solver", message: "no solution" },
  ]);
  assert.equal(site.allTargetsSatisfied, false);
  assert.deepEqual(site.candidates, []);
  assert.deepEqual(site.selections, { s1: "old" });
  assert.deepEqual(site.metrics, { attributes: {} });
});

test("inputs are not mutated", () => {
  const siteRequest = deepFreeze({
    targets: {
      budgetEnabled: true,
      budgetPrice: 100.4,
      preparednessEnabled: false,
    },
    smartOptions: {
      preferenceConditions: { recoil: { operator: "gte", value: 48 } },
      advancedPreferenceTargets: {
        "recoil-continuous-vertical": { operator: "lte", value: 0.46 },
      },
      advancedPreferences: { "recoil-single-horizontal": "custom" },
      advancedPreferenceValues: { "recoil-single-horizontal": 0.5 },
      advancedPreferenceOperators: { "recoil-single-horizontal": "lte" },
    },
  });
  const overrides = deepFreeze({ lockMode: "current" });
  const options = optionsFromSiteRequest(siteRequest, overrides);

  const context = deepFreeze({
    dataVersion: "v1",
    weaponId: "w",
    mode: "operations",
    laserEnabled: true,
    data: WEAPON_DATA,
    pricesByObjectId: {
      a: { currentPrice: 10, systemPrice: 5 },
      b: { currentPrice: null, systemPrice: 6 },
    },
    currentSelections: { s1: "p1" },
    tuningState: { k: 5 },
    lockedPathKeys: ["s1"],
    lockedTuningKeys: ["k"],
    excludedPartIds: ["x"],
  });
  const frozenOptions = deepFreeze(options);
  const request = toSolveRequest(context, frozenOptions);
  assert.deepEqual(request.prices, { a: 10 });
  assert.deepEqual(request.lockedPathKeys, { s1: "p1" });
  assert.deepEqual(request.excludedPartIds, ["x", "b"]);

  const raw = deepFreeze({
    optimal: true,
    truncated: false,
    nodes: 1,
    leaves: 2,
    message: "ok",
    best: {
      selections: { s1: "p1" },
      tuning: {},
      installed_part_keys: ["s1\0p1"],
      panel: { 10005: 50 },
      recoil: {},
      total_price: 10,
      attachment_price: 8,
    },
    alternatives: [],
  });
  const mapped = fromSolverOutput(raw);
  const site = toSiteResult(context, deepFreeze(mapped));
  assert.equal(site.smartRecommendation.kind, "recommendation");
  assert.deepEqual(mapped.build.selections, { s1: "p1" });
  assert.deepEqual(raw.best.selections, { s1: "p1" });
});
