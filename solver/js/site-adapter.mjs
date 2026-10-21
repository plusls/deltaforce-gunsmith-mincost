export class UnsupportedConstraintError extends Error {
  constructor(constraints) {
    const ids = Array.isArray(constraints) ? constraints : [String(constraints)];
    super(`unsupported constraints: ${ids.join(", ")}`);
    this.name = "UnsupportedConstraintError";
    this.constraints = ids;
  }
}

export class UnsupportedFeatureError extends Error {
  constructor(feature) {
    super(`unsupported feature: ${feature}`);
    this.name = "UnsupportedFeatureError";
    this.feature = feature;
  }
}

export const SUPPORTED_CONSTRAINT_IDS = new Set([
  "recoil-continuous-horizontal",
  "recoil-continuous-vertical",
  "recoil-single-horizontal",
  "recoil-single-vertical",
  "recoil-fire-stability",
  "handling-aim-time",
  "handling-sprint-fire-delay",
  "handling-aim-move-speed",
  "handling-reload-time",
  "handling-empty-reload-time",
  "handling-holster-time",
  "handling-switch-time",
  "stability-breath-range",
  "stability-breath-duration",
  "stability-movement-camera",
  "stability-fire-camera",
  "stability-hit-sway",
  "stability-hold-breath-recoil",
  "hipfire-standing-spread",
  "hipfire-maximum-spread",
  "10001",
  "10005",
  "10006",
  "10007",
  "10008",
]);

const DETAIL_METRIC_FIELDS = {
  "handling-aim-time": "aim_time",
  "handling-sprint-fire-delay": "sprint_fire_delay",
  "handling-aim-move-speed": "aim_move_speed",
  "handling-reload-time": "reload_time",
  "handling-empty-reload-time": "empty_reload_time",
  "handling-holster-time": "holster_time",
  "handling-switch-time": "switch_time",
  "stability-breath-range": "breath_range",
  "stability-breath-duration": "breath_duration",
  "stability-movement-camera": "movement_camera",
  "stability-fire-camera": "fire_camera",
  "stability-hit-sway": "hit_sway",
  "stability-hold-breath-recoil": "hold_breath_recoil",
  "hipfire-standing-spread": "hipfire_standing",
  "hipfire-maximum-spread": "hipfire_maximum",
};

function detailMetricMap(raw) {
  const detail = asObject(raw);
  const out = {};
  for (const [id, field] of Object.entries(DETAIL_METRIC_FIELDS)) {
    const value = detail[field];
    if (Number.isFinite(value)) out[id] = value;
  }
  return out;
}

export const PRIMARY_ATTR_BY_GROUP = {
  recoil: "10005",
  handling: "10006",
  stability: "10007",
  hipfire: "10008",
};

export const OPTION_DEFAULTS = Object.freeze({
  mode: "full",
  priceSource: "current",
  lockMode: "locked",
  missingPricePolicy: "reject",
  maxResults: 3,
  maxNodes: 2_000_000,
  maxLeaves: 1_000_000,
  deadlineMs: 5000,
  costCap: null,
  constraints: [],
});

const hasOwn = (object, key) =>
  Object.prototype.hasOwnProperty.call(object, key);

const asObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};

const asArray = (value) => (Array.isArray(value) ? value : []);

export function captureWorkerRequest(message, cache) {
  const msg = asObject(message);
  const store = cache instanceof Map ? cache : new Map();
  const dataKey = msg.dataKey;

  if (msg.data !== null && typeof msg.data === "object") {
    store.set(dataKey, msg.data);
  }

  const data =
    msg.data !== null && typeof msg.data === "object"
      ? msg.data
      : store.get(dataKey);

  if (data === null || typeof data !== "object") {
    throw new Error(`missing weapon payload for dataKey ${dataKey}`);
  }

  return {
    dataVersion: data.dataVersion ?? "",
    weaponId: msg.weaponId ?? "",
    mode: msg.mode ?? "operations",
    laserEnabled: msg.laserEnabled === true,
    data,
    pricesByObjectId: asObject(msg.pricesByObjectId),
    currentSelections: asObject(msg.currentSelections),
    tuningState: asObject(msg.tuningState),
    lockedPathKeys: asArray(msg.lockedPathKeys),
    lockedTuningKeys: asArray(msg.lockedTuningKeys),
    excludedPartIds: asArray(msg.excludedPartIds),
  };
}

function constraintFor(id, operator, value) {
  if (!Number.isFinite(value)) return null;
  if (operator === "gte") return { id, min: value };
  if (operator === "lte") return { id, max: value };
  if (operator === "eq") return { id, min: value, max: value };
  return null;
}

function readLegacyAdvanced(smartOptions, targets) {
  const prefs = asObject(smartOptions.advancedPreferences);
  const values = asObject(smartOptions.advancedPreferenceValues);
  const operators = asObject(smartOptions.advancedPreferenceOperators);

  for (const [id, raw] of Object.entries(values)) {
    if (targets.has(id)) continue;
    let value = raw;
    let operator = operators[id];
    if (raw !== null && typeof raw === "object") {
      value = raw.value;
      operator = raw.operator ?? operator;
    }
    if (!Number.isFinite(value)) continue;
    if (prefs[id] === "none") continue;
    targets.set(id, { operator: operator ?? "gte", value });
  }

  for (const [id, raw] of Object.entries(prefs)) {
    if (targets.has(id)) continue;
    if (raw === null || typeof raw !== "object") continue;
    const value = raw.value;
    if (!Number.isFinite(value)) continue;
    const operator = raw.operator ?? operators[id] ?? "gte";
    targets.set(id, { operator, value });
  }
}

export function optionsFromSiteRequest(siteRequest, overrides = {}) {
  const request = asObject(siteRequest);
  const overrideValues = asObject(overrides);
  const targets = asObject(request.targets);
  const smartOptions = asObject(request.smartOptions);

  if (targets.preparednessEnabled === true) {
    throw new UnsupportedFeatureError("preparedness");
  }

  let costCap = null;
  const budgetPrice = targets.budgetPrice;
  if (targets.budgetEnabled === true && Number.isFinite(budgetPrice) && budgetPrice > 0) {
    costCap = Math.round(budgetPrice);
  }

  const constraints = [];
  const conditions = asObject(smartOptions.preferenceConditions);
  for (const [group, attributeId] of Object.entries(PRIMARY_ATTR_BY_GROUP)) {
    const condition = asObject(conditions[group]);
    const constraint = constraintFor(attributeId, condition.operator, condition.value);
    if (constraint) constraints.push(constraint);
  }

  const advanced = new Map();
  for (const [id, raw] of Object.entries(asObject(smartOptions.advancedPreferenceTargets))) {
    const condition = asObject(raw);
    if (!Number.isFinite(condition.value)) continue;
    advanced.set(id, { operator: condition.operator, value: condition.value });
  }
  readLegacyAdvanced(smartOptions, advanced);

  const unsupported = [];
  for (const [id, condition] of advanced) {
    if (!SUPPORTED_CONSTRAINT_IDS.has(id)) {
      unsupported.push(id);
      continue;
    }
    const constraint = constraintFor(id, condition.operator, condition.value);
    if (constraint) constraints.push(constraint);
  }

  if (unsupported.length > 0 && overrideValues.ignoreUnsupported !== true) {
    throw new UnsupportedConstraintError([...new Set(unsupported)].sort());
  }

  const result = {};
  for (const [key, fallback] of Object.entries(OPTION_DEFAULTS)) {
    result[key] = hasOwn(overrideValues, key) ? overrideValues[key] : fallback;
  }
  result.constraints = Array.isArray(overrideValues.constraints)
    ? [...overrideValues.constraints]
    : constraints;
  if (hasOwn(overrideValues, "costCap")) {
    result.costCap = overrideValues.costCap;
  } else {
    result.costCap = costCap;
  }

  return result;
}

function pathKeys(value) {
  if (Array.isArray(value)) return value;
  if (value !== null && typeof value === "object") return Object.keys(value);
  return [];
}

export function toSolveRequest(context, options) {
  const ctx = asObject(context);
  const opts = asObject(options);

  const prices = {};
  const excluded = new Set(asArray(ctx.excludedPartIds));
  const pricesByObjectId = asObject(ctx.pricesByObjectId);

  for (const [officialObjectId, rawPair] of Object.entries(pricesByObjectId)) {
    const pair = asObject(rawPair);
    const pickSystem = opts.priceSource === "system";
    const value = pickSystem ? pair.systemPrice : pair.currentPrice;

    if (Number.isFinite(value) && value > 0) {
      prices[officialObjectId] = Math.round(value);
      continue;
    }

    if (
      opts.missingPricePolicy === "fallback-system" &&
      Number.isFinite(pair.systemPrice) &&
      pair.systemPrice > 0
    ) {
      prices[officialObjectId] = Math.round(pair.systemPrice);
      continue;
    }

    excluded.add(officialObjectId);
  }

  const currentSelections = asObject(ctx.currentSelections);
  const lockedPathKeys = {};
  for (const path of pathKeys(ctx.lockedPathKeys)) {
    const partId = currentSelections[path];
    lockedPathKeys[path] = typeof partId === "string" ? partId : null;
  }
  if (opts.lockMode === "current") {
    for (const [path, partId] of Object.entries(currentSelections)) {
      if (typeof partId === "string" && partId !== "") {
        lockedPathKeys[path] = partId;
      }
    }
  }

  const tuningState = asObject(ctx.tuningState);
  const lockedTuningKeys = {};
  for (const key of pathKeys(ctx.lockedTuningKeys)) {
    const value = tuningState[key];
    if (Number.isFinite(value)) lockedTuningKeys[key] = value;
  }

  return {
    prices,
    weaponId: ctx.weaponId ?? "",
    mode: ctx.mode ?? "operations",
    laserEnabled: ctx.laserEnabled === true,
    bipodDeployed: false,
    lockedPathKeys,
    lockedTuningKeys,
    excludedPartIds: [...excluded],
    constraints: Array.isArray(opts.constraints) ? opts.constraints : [],
    maxResults: opts.maxResults ?? 3,
    maxEvaluations: opts.maxNodes ?? 2_000_000,
    maxLeaves: opts.maxLeaves ?? 1_000_000,
    costCap: opts.costCap ?? null,
    firstSolution: opts.mode === "fast",
    deadlineMs: opts.deadlineMs ?? null,
  };
}

function mapBuild(raw) {
  const solution = asObject(raw);
  const selections = {};
  for (const [path, partId] of Object.entries(asObject(solution.selections))) {
    if (typeof partId === "string") selections[path] = partId;
  }

  let installedPartKeys = solution.installed_part_keys;
  if (!Array.isArray(installedPartKeys)) {
    installedPartKeys = asArray(solution.installed_parts)
      .filter(
        (part) =>
          part !== null &&
          typeof part === "object" &&
          part.slot_path != null &&
          part.part_id != null,
      )
      .map((part) => `${part.slot_path}\0${part.part_id}`)
      .sort();
  }

  return {
    selections,
    tuning: asObject(solution.tuning),
    installedPartKeys: [...installedPartKeys],
    attributes: asObject(solution.panel),
    detail: asObject(solution.detail),
    recoil: asObject(solution.recoil),
    totalPrice: solution.total_price,
    attachmentPrice: solution.attachment_price,
  };
}

const TRUNCATED_MESSAGE =
  "time budget exhausted before a feasible build was found";

export function fromSolverOutput(raw) {
  const output = asObject(raw);
  const optimal = output.optimal === true;
  const truncated = output.truncated === true;

  let kind;
  if (output.best) {
    kind = optimal ? "optimal" : "feasible";
  } else if (truncated) {
    kind = "error";
  } else {
    kind = "infeasible";
  }

  const result = {
    kind,
    optimal,
    truncated,
    nodes: output.nodes ?? 0,
    leaves: output.leaves ?? 0,
    elapsedMs: 0,
    message: kind === "error" ? TRUNCATED_MESSAGE : (output.message ?? ""),
  };

  if (output.best) {
    result.build = mapBuild(output.best);
  }

  result.alternatives = asArray(output.alternatives)
    .filter((solution) => solution !== null && typeof solution === "object")
    .map(mapBuild);

  return result;
}

function sanitizeSelections(selections) {
  const clean = {};
  for (const [path, partId] of Object.entries(asObject(selections))) {
    if (typeof partId === "string") clean[path] = partId;
  }
  return clean;
}

export function toSiteResult(context, result) {
  const ctx = asObject(context);
  const mapped = asObject(result);
  const kind = mapped.kind;
  const feasible = kind === "optimal" || kind === "feasible";
  const currentSelections = sanitizeSelections(ctx.currentSelections);
  const elapsedMs = mapped.elapsedMs ?? 0;
  const evaluatedStates = mapped.nodes ?? 0;
  const generatedStates = mapped.leaves ?? 0;

  if (!feasible || !mapped.build) {
    return {
      selections: currentSelections,
      metrics: { attributes: {} },
      ranges: {},
      rangeWitnesses: {},
      unmetItems: [],
      allTargetsSatisfied: false,
      elapsedMs,
      evaluatedStates,
      generatedStates,
      truncated: mapped.optimal === true ? false : true,
      smartRecommendation: {
        kind: "infeasible",
        currentMetrics: { attributes: {} },
        candidate: undefined,
        changedPartCount: 0,
        changedTuningCount: 0,
        score: 0,
        variants: [],
        failures: [{ kind: "solver", message: mapped.message ?? "" }],
      },
      candidates: [],
    };
  }

  const build = asObject(mapped.build);
  const candidate = {
    selections: sanitizeSelections(build.selections),
    tuningState: asObject(build.tuning),
    installedPartKeys: asArray(build.installedPartKeys),
    metrics: {
      attributes: asObject(build.attributes),
      detailMetrics: detailMetricMap(build.detail),
    },
  };

  return {
    selections: currentSelections,
    metrics: { attributes: {} },
    ranges: {},
    rangeWitnesses: {},
    unmetItems: [],
    allTargetsSatisfied: true,
    elapsedMs,
    evaluatedStates,
    generatedStates,
    truncated: mapped.optimal !== true,
    smartRecommendation: {
      kind: "recommendation",
      currentMetrics: { attributes: {} },
      candidate,
      changedPartCount: 0,
      changedTuningCount: 0,
      score: 0,
      variants: [],
      failures: [],
    },
    candidates: [candidate],
  };
}
