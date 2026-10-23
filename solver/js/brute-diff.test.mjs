import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { compare } from "./brute-diff.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const bruteDiff = path.join(here, "brute-diff.mjs");

function solveResult(price) {
  return {
    optimal: true,
    truncated: false,
    best: {
      total_price: price,
      constraints: [],
      recoil: {
        continuous_h: 0,
        continuous_v: 0,
        single_h: 0,
        single_v: 0,
        fire_stability: 0,
      },
    },
  };
}

function feasibleRecheck(price) {
  return {
    feasible: true,
    totalPrice: price,
    constraints: [],
    recoil: {
      continuousH: 0,
      continuousV: 0,
      singleH: 0,
      singleV: 0,
      fireStability: 0,
    },
  };
}

function bruteResult({ price = 1200, witnessLock, recheck = null, lockRejected = 0 } = {}) {
  return {
    brute: {
      minTotalPrice: price,
      assemblies: 1,
      tuningEvals: 1,
      feasibleCount: 1,
      assembliesTruncated: false,
      evalsTruncated: false,
      skippedTuningGroups: 0,
      lockRejectedAssemblies: lockRejected,
      witnessLock,
    },
    recheckSolution: recheck,
  };
}

test("compare rejects a brute witness whose locks were not installed", () => {
  const witnessLock = {
    valid: false,
    checked: [
      {
        path: "weapon:w1/slot:3:S3",
        expected: "p2",
        actual: null,
        slotFound: true,
        ok: false,
      },
    ],
  };
  const result = compare(
    solveResult(1200),
    bruteResult({ price: 1200, witnessLock, recheck: feasibleRecheck(1200) }),
  );
  assert.equal(result.verdict, "DIFF-brute-invalid-lock");
  assert.equal(result.witnessLockValid, false);
});

test("compare keeps OK for a valid witness lock", () => {
  const result = compare(
    solveResult(1200),
    bruteResult({
      price: 1200,
      witnessLock: { valid: true, checked: [] },
      recheck: feasibleRecheck(1200),
      lockRejected: 1,
    }),
  );
  assert.equal(result.verdict, "OK");
  assert.equal(result.witnessLockValid, true);
  assert.equal(result.lockRejectedAssemblies, 1);
});

test("compare keeps the old behaviour when witnessLock is absent", () => {
  const result = compare(
    solveResult(1200),
    bruteResult({ price: 1200, recheck: feasibleRecheck(1200) }),
  );
  assert.equal(result.verdict, "OK");
  assert.equal(result.witnessLockValid, null);
});

test("an invalid witness lock fails the run with a DIFF exit code", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "dws-brute-diff-test-"));
  try {
    const solveScript = path.join(dir, "solve");
    const bfScript = path.join(dir, "bf");
    const caseFile = path.join(dir, "case.json");
    fs.writeFileSync(
      solveScript,
      `#!/usr/bin/env node\nprocess.stdout.write(${JSON.stringify(JSON.stringify(solveResult(1200)))});\n`,
      { mode: 0o755 },
    );
    fs.writeFileSync(
      bfScript,
      `#!/usr/bin/env node\nprocess.stdout.write(${JSON.stringify(
        JSON.stringify(
          bruteResult({
            price: 1000,
            witnessLock: { valid: false, checked: [] },
            recheck: feasibleRecheck(1000),
          }),
        ),
      )});\n`,
      { mode: 0o755 },
    );
    fs.writeFileSync(
      caseFile,
      JSON.stringify({
        payload: { dataVersion: "test", parts: {}, slotsByHost: {} },
        request: {
          weaponId: "w1",
          lockedPathKeys: { "weapon:w1/slot:3:S3": "p2" },
        },
      }),
    );
    const proc = spawnSync(
      process.execPath,
      [bruteDiff, "--solve", solveScript, "--bf", bfScript, caseFile],
      { encoding: "utf8" },
    );
    assert.equal(proc.status, 1, proc.stderr);
    const result = JSON.parse(proc.stdout.trim());
    assert.equal(result.verdict, "DIFF-brute-invalid-lock");
    assert.equal(result.brutePrice, 1000);
    assert.equal(result.solvePrice, 1200);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
