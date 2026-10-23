import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const defaults = {
  solve: "target/release/solve",
  bf: "target/release/bf",
};

function usage() {
  console.error(
    "usage: node solver/js/brute-diff.mjs [--solve path] [--bf path] <case.json>...",
  );
  console.error("case JSON must contain payload and request");
}

function parseArgs(argv) {
  const options = { ...defaults, cases: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--solve") {
      options.solve = argv[++i];
    } else if (arg === "--bf") {
      options.bf = argv[++i];
    } else if (arg === "-h" || arg === "--help") {
      usage();
      process.exit(0);
    } else {
      options.cases.push(arg);
    }
  }
  return options;
}

function run(command, args) {
  const proc = spawnSync(command, args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  if (proc.error) throw proc.error;
  if (proc.status !== 0) {
    throw new Error(`${command} exited ${proc.status}: ${proc.stderr.trim()}`);
  }
  return JSON.parse(proc.stdout);
}

function metricDiffs(solve, recheck) {
  const diffs = [];
  if (solve.best.total_price !== recheck.totalPrice) {
    diffs.push(["total_price", solve.best.total_price, recheck.totalPrice]);
  }
  const reported = new Map((solve.best.constraints ?? []).map((c) => [c.id, c.value]));
  for (const constraint of recheck.constraints ?? []) {
    if (!reported.has(constraint.id)) continue;
    const value = reported.get(constraint.id);
    if (Math.abs(value - constraint.value) > 1e-9) {
      diffs.push([constraint.id, value, constraint.value]);
    }
  }
  const pairs = [
    ["continuousH", "continuous_h"],
    ["continuousV", "continuous_v"],
    ["singleH", "single_h"],
    ["singleV", "single_v"],
    ["fireStability", "fire_stability"],
  ];
  for (const [bruteKey, solveKey] of pairs) {
    if (Math.abs(recheck.recoil[bruteKey] - solve.best.recoil[solveKey]) > 1e-9) {
      diffs.push([bruteKey, solve.best.recoil[solveKey], recheck.recoil[bruteKey]]);
    }
  }
  return diffs;
}

export function compare(solve, brute) {
  const best = solve.best ?? null;
  const brutePrice = brute.brute.minTotalPrice;
  const recheck = brute.recheckSolution ?? null;
  const witnessLock = brute.brute.witnessLock ?? null;
  const truncated =
    brute.brute.assembliesTruncated ||
    brute.brute.evalsTruncated ||
    brute.brute.skippedTuningGroups > 0;

  const result = {
    solvePrice: best?.total_price ?? null,
    brutePrice,
    optimal: solve.optimal,
    solveTruncated: solve.truncated,
    bruteTruncated: truncated,
    assemblies: brute.brute.assemblies,
    tuningEvals: brute.brute.tuningEvals,
    feasible: brute.brute.feasibleCount,
    lockRejectedAssemblies: brute.brute.lockRejectedAssemblies ?? null,
    witnessLockValid: witnessLock?.valid ?? null,
    recheckFeasible: recheck?.feasible ?? null,
    diffs: [],
  };

  if (witnessLock?.valid === false) {
    result.verdict = "DIFF-brute-invalid-lock";
    return result;
  }
  if (truncated) {
    result.verdict = "BRUTE-TRUNCATED";
    return result;
  }
  if (best === null && brutePrice === null) {
    result.verdict = "OK-no-solution";
    return result;
  }
  if (best === null) {
    result.verdict = "DIFF-missed-solution";
    return result;
  }
  if (brutePrice === null) {
    result.verdict = "DIFF-brute-none";
    return result;
  }
  if (recheck) {
    result.diffs = metricDiffs(solve, recheck);
  }
  if (!recheck?.feasible) {
    result.verdict = "DIFF-not-feasible";
  } else if (result.diffs.length > 0) {
    result.verdict = "DIFF-values";
  } else if (best.total_price === brutePrice) {
    result.verdict = "OK";
  } else if (best.total_price > brutePrice) {
    result.verdict = "DIFF-suboptimal";
  } else {
    result.verdict = "DIFF-cheaper-than-brute";
  }
  return result;
}

function runCase(file, options, tempDir) {
  const spec = JSON.parse(fs.readFileSync(file, "utf8"));
  if (!spec.payload || !spec.request) {
    throw new Error(`${file}: case JSON needs payload and request`);
  }
  const name = path.basename(file, ".json");
  const payloadPath = path.join(tempDir, `${name}.payload.json`);
  const requestPath = path.join(tempDir, `${name}.request.json`);
  fs.writeFileSync(payloadPath, JSON.stringify(spec.payload));
  fs.writeFileSync(requestPath, JSON.stringify(spec.request));

  const solve = run(options.solve, ["--payload", payloadPath, "--request", requestPath]);
  const brute = run(options.bf, [file]);
  return {
    case: name,
    ...compare(solve, brute),
    tuning: solve.best?.tuning ?? null,
    selections: solve.best?.selections ?? null,
  };
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.cases.length === 0) {
    usage();
    process.exit(2);
  }
  const tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "dws-brute-diff-"));
  let failed = 0;
  try {
    for (const file of options.cases) {
      let result;
      try {
        result = runCase(file, options, tempDir);
      } catch (error) {
        result = { case: path.basename(file, ".json"), verdict: "ERROR", error: String(error) };
      }
      if (result.verdict.startsWith("DIFF") || result.verdict === "ERROR") {
        failed += 1;
      }
      console.log(JSON.stringify(result));
    }
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
  if (failed > 0) {
    process.exitCode = 1;
  }
}

const invokedDirectly =
  process.argv[1] !== undefined &&
  path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (invokedDirectly) {
  main();
}
