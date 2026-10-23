export const SITE_SEARCH_INPUT_SELECTOR =
  'input[aria-label="搜索时长（秒）"]';

export const SITE_OPTIMIZER_INPUTS_KEY_PREFIX =
  "moligod:gunsmith-optimizer-inputs:v1";

export const SITE_SEARCH_SECONDS_DEFAULT = 15;
export const SITE_SEARCH_SECONDS_MIN = 2;
export const SITE_SEARCH_SECONDS_MAX = 60;
export const SITE_SEARCH_FINETUNE_MIN_SECONDS = 5;
export const SITE_MAX_EVALUATIONS_PER_SECOND = 15_000;

export const SOLVE_DEADLINE_FALLBACK_MS = Object.freeze({
  full: 5_000,
  fast: 2_000,
});

export const SOLVE_DEADLINE_MIN_MS = 1_000;
export const SOLVE_DEADLINE_MAX_MS = 300_000;

export const TIME_LIMIT_SOURCES = Object.freeze({
  user: "user",
  site: "site",
  request: "request",
  fallback: "fallback",
});

const asPlainObject = (value) =>
  value !== null && typeof value === "object" && !Array.isArray(value)
    ? value
    : {};

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

export function normalizeTimeLimitSeconds(raw) {
  if (raw === null || raw === undefined || raw === "") return null;
  const seconds = Number(raw);
  if (!Number.isFinite(seconds) || seconds <= 0) return null;
  return Math.round(
    clamp(
      seconds,
      SOLVE_DEADLINE_MIN_MS / 1000,
      SOLVE_DEADLINE_MAX_MS / 1000,
    ),
  );
}

export function normalizeSiteSearchSeconds(raw) {
  const seconds = Number(raw);
  if (!Number.isFinite(seconds) || seconds <= 0) return null;
  return Math.round(clamp(seconds, SITE_SEARCH_SECONDS_MIN, SITE_SEARCH_SECONDS_MAX));
}

export function siteOptimizerInputsKey(weaponId, mode, preparedness = false) {
  return `${SITE_OPTIMIZER_INPUTS_KEY_PREFIX}:${weaponId ?? ""}:${mode ?? "operations"}:${
    preparedness ? "preparedness" : "ordinary"
  }`;
}

export function siteSearchSecondsFromRecord(record) {
  const parsed = asPlainObject(record);
  return normalizeSiteSearchSeconds(parsed.smartSearchSeconds);
}

export function selectSiteSearchSeconds({ domSeconds = null, storedSeconds = null } = {}) {
  const fromDom = normalizeSiteSearchSeconds(domSeconds);
  if (fromDom !== null) return fromDom;
  return normalizeSiteSearchSeconds(storedSeconds);
}

export function resolveDeadlineMs({
  mode = "full",
  timeLimitSeconds = null,
  siteSeconds = null,
  requestDeadlineMs = null,
} = {}) {
  const userSeconds = normalizeTimeLimitSeconds(timeLimitSeconds);
  if (userSeconds !== null) {
    return {
      deadlineMs:
        Math.round(
          clamp(
            userSeconds,
            SOLVE_DEADLINE_MIN_MS / 1000,
            SOLVE_DEADLINE_MAX_MS / 1000,
          ),
        ) * 1000,
      source: TIME_LIMIT_SOURCES.user,
    };
  }

  const normalizedSite = normalizeSiteSearchSeconds(siteSeconds);
  if (normalizedSite !== null) {
    return {
      deadlineMs: normalizedSite * 1000,
      source: TIME_LIMIT_SOURCES.site,
    };
  }

  const requestMs = Number(requestDeadlineMs);
  if (Number.isFinite(requestMs) && requestMs > 0) {
    return {
      deadlineMs: Math.max(
        1,
        Math.min(Math.round(requestMs), SOLVE_DEADLINE_MAX_MS),
      ),
      source: TIME_LIMIT_SOURCES.request,
    };
  }

  const fallback =
    SOLVE_DEADLINE_FALLBACK_MS[mode] ?? SOLVE_DEADLINE_FALLBACK_MS.full;
  return { deadlineMs: fallback, source: TIME_LIMIT_SOURCES.fallback };
}

export function formatDuration(ms) {
  if (!Number.isFinite(ms) || ms <= 0) return "0 毫秒";
  if (ms < 1000) return `${Math.round(ms)} 毫秒`;
  const seconds = ms / 1000;
  return `${Number.isInteger(seconds) ? seconds : seconds.toFixed(1)} 秒`;
}

export function describeResultStatus(result, deadlineMs) {
  const mapped = asPlainObject(result);
  const elapsed = Number.isFinite(mapped.elapsedMs) ? mapped.elapsedMs : 0;
  const hasBuild = mapped.kind === "optimal" || mapped.kind === "feasible";

  if (mapped.truncated === true) {
    if (hasBuild) {
      return {
        tone: "warn",
        text: `时限 ${formatDuration(deadlineMs)} 用尽 · 当前最好方案, 未证明最优`,
      };
    }
    return {
      tone: "error",
      text: `时限 ${formatDuration(deadlineMs)} 用尽 · 未找到可行方案`,
    };
  }
  if (mapped.kind === "optimal") {
    return { tone: "ok", text: `最优解 · 用时 ${formatDuration(elapsed)}` };
  }
  if (mapped.kind === "feasible") {
    return {
      tone: "warn",
      text: `找到可行方案 · 用时 ${formatDuration(elapsed)} · 未证明最优`,
    };
  }
  if (mapped.kind === "incomplete") {
    return {
      tone: "error",
      text: "搜索因内部上限提前停止 · 未证明无解",
    };
  }
  if (mapped.kind === "error") {
    const reason = mapped.message ? `求解失败 · ${mapped.message}` : "求解失败";
    return { tone: "error", text: `${reason} · 未证明无解` };
  }
  return { tone: "error", text: `当前约束无解 · 用时 ${formatDuration(elapsed)}` };
}
