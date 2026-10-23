export const SOLVE_DEADLINE_FALLBACK_MS = Object.freeze({
  full: 5_000,
  fast: 2_000,
});

export const SOLVE_DEADLINE_MIN_MS = 1_000;
export const SOLVE_DEADLINE_MAX_MS = 300_000;

export const WORKER_INIT_TIMEOUT_MS = 10_000;
export const SITE_SOLVE_CAPTURE_TIMEOUT_MS = 3_000;

export function clamp(value, min, max) {
  return Math.min(max, Math.max(min, value));
}

// Request budgets are clamped to [1ms, SOLVE_DEADLINE_MAX_MS]; invalid or
// non-positive budgets yield null so each caller can apply its own fallback.
export function clampRequestDeadlineMs(raw) {
  const ms = Number(raw);
  if (!Number.isFinite(ms) || ms <= 0) return null;
  return Math.max(1, Math.min(Math.round(ms), SOLVE_DEADLINE_MAX_MS));
}
