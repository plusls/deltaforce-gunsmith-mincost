import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..");
const defaultWasmPath = resolve(
  repo,
  "target/wasm32-unknown-unknown/release/gunsmith_mincost.wasm",
);

function stripModule(source) {
  return source
    .replace(/^import[\s\S]*?from\s+["'][^"']+["'];?[ \t]*$/gm, "")
    .replace(/^export\s+(?=(async\s+)?(function|class|const|let|var)\b)/gm, "")
    .replace(/^export\s+\{[^}]*\};?[ \t]*$/gm, "");
}

function readModule(repoDir, path) {
  return stripModule(readFileSync(resolve(repoDir, path), "utf8"));
}

export function buildBundle({
  repoDir = repo,
  wasmPath = defaultWasmPath,
  wasmBytes = null,
} = {}) {
  const core = [
    readModule(repoDir, "solver/js/core/util.mjs"),
    readModule(repoDir, "solver/js/core/limits.mjs"),
    readModule(repoDir, "solver/js/core/protocol.mjs"),
    readModule(repoDir, "solver/js/core/payload-cache.mjs"),
    readModule(repoDir, "solver/js/solver.mjs"),
    readModule(repoDir, "solver/js/deadline.mjs"),
    readModule(repoDir, "solver/js/site-adapter.mjs"),
    readModule(repoDir, "solver/js/engine.mjs"),
  ].join("\n");

  const workerSource = `${core}\n${readModule(repoDir, "userscript/src/worker-main.mjs")}\n`;
  const mainSource = `${readModule(repoDir, "userscript/src/status.mjs")}\n${readModule(
    repoDir,
    "userscript/src/constraint-sync.mjs",
  )}\n${readModule(repoDir, "userscript/src/sync-ui.mjs")}\n${readModule(
    repoDir,
    "userscript/src/main.mjs",
  )}`;
  const wasmBase64 = (wasmBytes ?? readFileSync(wasmPath)).toString("base64");

  let bundle = `${metadata}
(function () {
"use strict";
${core}
${mainSource}
})();
`;

  bundle = bundle
    .replace('"__WASM_BASE64__"', () => JSON.stringify(wasmBase64))
    .replace('"__WORKER_SOURCE__"', () => JSON.stringify(workerSource));

  if (bundle.includes("__WASM_BASE64__") || bundle.includes("__WORKER_SOURCE__")) {
    throw new Error("bundle placeholders were not replaced");
  }
  return { bundle, workerSource, mainSource, wasmBase64 };
}

const metadata = `// ==UserScript==
// @name         moligod 改枪最低开销
// @namespace    https://github.com/plusls/gunsmith-mincost
// @version      0.4.2
// @description  在硬条件约束下搜索总价最低的改枪方案
// @match        https://moligod.com/gunsmith/*
// @match        https://moligod.com/gunsmith
// @run-at       document-start
// @grant        unsafeWindow
// ==/UserScript==
`;

function main() {
  const outDir = resolve(here, "dist");
  const outPath = resolve(outDir, "gunsmith-mincost.user.js");
  const { bundle } = buildBundle();
  mkdirSync(outDir, { recursive: true });
  writeFileSync(outPath, bundle);
  console.log(
    `wrote ${outPath} (${(Buffer.byteLength(bundle) / 1024 / 1024).toFixed(2)} MiB)`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main();
}
