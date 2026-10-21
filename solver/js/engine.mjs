import { loadSolver } from "./solver.mjs";
import {
  optionsFromSiteRequest,
  fromSolverOutput,
  toSolveRequest,
} from "./site-adapter.mjs";

export const DEFAULT_SOLVE_OPTIONS = Object.freeze({
  mode: "full",
  priceSource: "current",
  lockMode: "locked",
  missingPricePolicy: "reject",
  constraints: [],
  costCap: null,
  maxResults: 3,
  maxNodes: 2_000_000,
  maxLeaves: 1_000_000,
  deadlineMs: 5_000,
});

export function normalizeSolveOptions(options = {}) {
  return { ...DEFAULT_SOLVE_OPTIONS, ...options };
}

function now() {
  return typeof performance !== "undefined" && performance.now
    ? performance.now()
    : Date.now();
}

export function decodeBase64(text) {
  const binary =
    typeof atob === "function"
      ? atob(text)
      : Buffer.from(text, "base64").toString("binary");
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

async function resolveWasm(options) {
  if (options.wasmBase64 != null) return decodeBase64(options.wasmBase64);
  if (options.wasm != null) return options.wasm;
  throw new TypeError("createMinCostEngine needs wasm or wasmBase64");
}

export async function createMinCostEngine(options) {
  const solver = await loadSolver(await resolveWasm(options));

  function solve(context, solveOptions = {}) {
    if (!context || !context.data) {
      throw new TypeError("solve needs a GunsmithContext with data");
    }
    const normalized = normalizeSolveOptions(solveOptions);
    const request = toSolveRequest(context, normalized);
    const started = now();
    const raw = solver.solve(context.data, request);
    const result = fromSolverOutput(raw);
    result.elapsedMs = Math.round(now() - started);
    return result;
  }

  function solveSiteRequest(context, siteRequest, overrides = {}) {
    return solve(context, optionsFromSiteRequest(siteRequest, overrides));
  }

  return { solve, solveSiteRequest };
}
