import test from "node:test";
import assert from "node:assert/strict";
import {
  describeResultStatus,
  formatDuration,
  normalizeSiteSearchSeconds,
  normalizeTimeLimitSeconds,
  resolveDeadlineMs,
  selectSiteSearchSeconds,
  siteOptimizerInputsKey,
  siteSearchSecondsFromRecord,
  SITE_SEARCH_SECONDS_MAX,
  SITE_SEARCH_SECONDS_MIN,
  SOLVE_DEADLINE_FALLBACK_MS,
  SOLVE_DEADLINE_MAX_MS,
  SOLVE_DEADLINE_MIN_MS,
  TIME_LIMIT_SOURCES,
} from "./deadline.mjs";

test("normalizeTimeLimitSeconds accepts numbers and numeric strings", () => {
  assert.equal(normalizeTimeLimitSeconds(15), 15);
  assert.equal(normalizeTimeLimitSeconds("15"), 15);
  assert.equal(normalizeTimeLimitSeconds(15.4), 15);
  assert.equal(normalizeTimeLimitSeconds("15.6"), 16);
  assert.equal(normalizeTimeLimitSeconds(null), null);
  assert.equal(normalizeTimeLimitSeconds(undefined), null);
  assert.equal(normalizeTimeLimitSeconds(""), null);
  assert.equal(normalizeTimeLimitSeconds("abc"), null);
  assert.equal(normalizeTimeLimitSeconds(0), null);
  assert.equal(normalizeTimeLimitSeconds(-5), null);
  assert.equal(normalizeTimeLimitSeconds(Infinity), null);
  assert.equal(normalizeTimeLimitSeconds(0.4), 1);
  assert.equal(normalizeTimeLimitSeconds(10_000), 300);
});

test("normalizeSiteSearchSeconds clamps to the site range", () => {
  assert.equal(normalizeSiteSearchSeconds(15), 15);
  assert.equal(normalizeSiteSearchSeconds("60"), 60);
  assert.equal(normalizeSiteSearchSeconds(1), SITE_SEARCH_SECONDS_MIN);
  assert.equal(normalizeSiteSearchSeconds(10_000), SITE_SEARCH_SECONDS_MAX);
  assert.equal(normalizeSiteSearchSeconds("bad"), null);
  assert.equal(normalizeSiteSearchSeconds(null), null);
});

test("site record and DOM readings feed the site seconds selection", () => {
  assert.equal(
    siteSearchSecondsFromRecord({ smartSearchSeconds: 30 }),
    30,
  );
  assert.equal(siteSearchSecondsFromRecord({}), null);
  assert.equal(siteSearchSecondsFromRecord(null), null);

  assert.equal(
    selectSiteSearchSeconds({ domSeconds: 45, storedSeconds: 30 }),
    45,
  );
  assert.equal(
    selectSiteSearchSeconds({ domSeconds: null, storedSeconds: 30 }),
    30,
  );
  assert.equal(selectSiteSearchSeconds({}), null);
  assert.equal(
    siteOptimizerInputsKey("18010000049", "operations"),
    "moligod:gunsmith-optimizer-inputs:v1:18010000049:operations:ordinary",
  );
  assert.match(
    siteOptimizerInputsKey("w", "warfare", true),
    /warfare:preparedness$/,
  );
});

test("resolveDeadlineMs precedence is user, site, request, then per-mode fallback", () => {
  assert.deepEqual(
    resolveDeadlineMs({ mode: "full", timeLimitSeconds: 15, siteSeconds: 60 }),
    { deadlineMs: 15_000, source: TIME_LIMIT_SOURCES.user },
  );
  assert.deepEqual(
    resolveDeadlineMs({ mode: "full", timeLimitSeconds: null, siteSeconds: 60 }),
    { deadlineMs: 60_000, source: TIME_LIMIT_SOURCES.site },
  );
  assert.deepEqual(
    resolveDeadlineMs({
      mode: "full",
      timeLimitSeconds: null,
      siteSeconds: null,
      requestDeadlineMs: 15_000,
    }),
    { deadlineMs: 15_000, source: TIME_LIMIT_SOURCES.request },
  );
  assert.deepEqual(
    resolveDeadlineMs({
      mode: "full",
      timeLimitSeconds: 20,
      siteSeconds: 30,
      requestDeadlineMs: 15_000,
    }),
    { deadlineMs: 20_000, source: TIME_LIMIT_SOURCES.user },
  );
  assert.deepEqual(
    resolveDeadlineMs({ mode: "full", siteSeconds: 30, requestDeadlineMs: 15_000 }),
    { deadlineMs: 30_000, source: TIME_LIMIT_SOURCES.site },
  );
  assert.deepEqual(
    resolveDeadlineMs({ mode: "fast", siteSeconds: null }),
    {
      deadlineMs: SOLVE_DEADLINE_FALLBACK_MS.fast,
      source: TIME_LIMIT_SOURCES.fallback,
    },
  );
  assert.deepEqual(
    resolveDeadlineMs({ mode: "full" }),
    {
      deadlineMs: SOLVE_DEADLINE_FALLBACK_MS.full,
      source: TIME_LIMIT_SOURCES.fallback,
    },
  );
  assert.deepEqual(
    resolveDeadlineMs({ mode: "unknown" }),
    {
      deadlineMs: SOLVE_DEADLINE_FALLBACK_MS.full,
      source: TIME_LIMIT_SOURCES.fallback,
    },
  );
});

test("site request deadline is validated and clamped", () => {
  assert.equal(
    resolveDeadlineMs({ requestDeadlineMs: 1_000_000_000 }).deadlineMs,
    SOLVE_DEADLINE_MAX_MS,
  );
  assert.equal(
    resolveDeadlineMs({ requestDeadlineMs: 0 }).deadlineMs,
    SOLVE_DEADLINE_FALLBACK_MS.full,
  );
  assert.equal(
    resolveDeadlineMs({ requestDeadlineMs: Number.NaN }).deadlineMs,
    SOLVE_DEADLINE_FALLBACK_MS.full,
  );
  assert.equal(
    resolveDeadlineMs({ requestDeadlineMs: "1250.4" }).deadlineMs,
    1_250,
  );
});

test("user time limit is clamped to the userscript range", () => {
  assert.equal(
    resolveDeadlineMs({ timeLimitSeconds: 0.5, siteSeconds: 60 }).deadlineMs,
    SOLVE_DEADLINE_MIN_MS,
  );
  assert.equal(
    resolveDeadlineMs({ timeLimitSeconds: 10_000 }).deadlineMs,
    SOLVE_DEADLINE_MAX_MS,
  );
  assert.equal(
    resolveDeadlineMs({ timeLimitSeconds: 300 }).deadlineMs,
    SOLVE_DEADLINE_MAX_MS,
  );
});

test("formatDuration renders milliseconds and seconds", () => {
  assert.equal(formatDuration(15_000), "15 秒");
  assert.equal(formatDuration(1_500), "1.5 秒");
  assert.equal(formatDuration(400), "400 毫秒");
  assert.equal(formatDuration(0), "0 毫秒");
  assert.equal(formatDuration(Number.NaN), "0 毫秒");
});

test("system-price budgets are disclosed instead of claimed as market-price optimal", () => {
  const base = {
    kind: "optimal",
    optimal: true,
    truncated: false,
    elapsedMs: 1200,
    priceSource: "system",
    costCap: 100000,
  };

  const system = describeResultStatus(base, 5000);
  assert.equal(system.tone, "warn");
  assert.match(system.text, /最优解/);
  assert.match(system.text, /预算按系统价约束, 市场价可能超预算/);

  const current = describeResultStatus(
    { ...base, priceSource: "current" },
    5000,
  );
  assert.equal(current.tone, "ok");
  assert.doesNotMatch(current.text, /系统价/);

  const noCap = describeResultStatus(
    { ...base, costCap: null },
    5000,
  );
  assert.equal(noCap.tone, "ok");
  assert.doesNotMatch(noCap.text, /系统价/);

  const infeasible = describeResultStatus(
    { kind: "infeasible", truncated: false, priceSource: "system", costCap: 100 },
    5000,
  );
  assert.equal(infeasible.tone, "error");
  assert.match(infeasible.text, /市场价可能超预算/);
});

test("result status never claims optimal for a truncated run", () => {
  assert.deepEqual(describeResultStatus({ kind: "optimal", elapsedMs: 1200 }, 5000), {
    tone: "ok",
    text: "最优解 · 用时 1.2 秒",
  });

  const feasible = describeResultStatus(
    { kind: "feasible", optimal: false, truncated: true, elapsedMs: 15_000 },
    15_000,
  );
  assert.equal(feasible.tone, "warn");
  assert.match(feasible.text, /时限 15 秒 用尽/);
  assert.match(feasible.text, /未证明最优/);
  assert.doesNotMatch(feasible.text, /最优解/);

  const error = describeResultStatus({ kind: "error", truncated: true }, 60_000);
  assert.equal(error.tone, "error");
  assert.match(error.text, /时限 60 秒 用尽/);

  const truncatedInfeasible = describeResultStatus(
    { kind: "infeasible", truncated: true },
    5000,
  );
  assert.match(truncatedInfeasible.text, /时限 5 秒 用尽/);

  const provenInfeasible = describeResultStatus(
    { kind: "infeasible", truncated: false, elapsedMs: 900 },
    5000,
  );
  assert.equal(provenInfeasible.tone, "error");
  assert.match(provenInfeasible.text, /当前约束无解/);
});

test("internal-limit stop is reported as incomplete, not proven infeasible", () => {
  const status = describeResultStatus(
    { kind: "incomplete", optimal: false, truncated: false, elapsedMs: 900 },
    5000,
  );
  assert.equal(status.tone, "error");
  assert.match(status.text, /内部上限提前停止/);
  assert.match(status.text, /未证明无解/);
  assert.doesNotMatch(status.text, /当前约束无解/);
});

test("solver failures never claim the constraints were proven infeasible", () => {
  const status = describeResultStatus(
    { kind: "error", truncated: false, message: "boom" },
    5000,
  );
  assert.equal(status.tone, "error");
  assert.match(status.text, /求解失败/);
  assert.match(status.text, /未证明无解/);
});

test("fast-mode result found before the deadline does not claim the limit was hit", () => {
  const fast = describeResultStatus(
    { kind: "feasible", optimal: false, truncated: false, elapsedMs: 70 },
    2000,
  );
  assert.equal(fast.tone, "warn");
  assert.match(fast.text, /找到可行方案/);
  assert.match(fast.text, /70 毫秒/);
  assert.match(fast.text, /未证明最优/);
  assert.doesNotMatch(fast.text, /用尽/);
  assert.doesNotMatch(fast.text, /最优解/);
});

test("an inconsistent optimal-and-truncated result is reported as truncated", () => {
  const status = describeResultStatus(
    { kind: "optimal", optimal: true, truncated: true, elapsedMs: 5000 },
    5000,
  );
  assert.equal(status.tone, "warn");
  assert.match(status.text, /未证明最优/);
  assert.doesNotMatch(status.text, /^最优解/);
});
