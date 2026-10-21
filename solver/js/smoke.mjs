import fs from "node:fs";
import { loadSolver } from "./solver.mjs";

const [wasmPath, payloadPath, requestPath] = process.argv.slice(2);
if (!wasmPath || !payloadPath || !requestPath) {
  console.error("usage: node smoke.mjs <solver.wasm> <payload.json> <request.json>");
  process.exit(1);
}

const wasm = fs.readFileSync(wasmPath);
const solver = await loadSolver(wasm);
const payload = JSON.parse(fs.readFileSync(payloadPath, "utf8"));
const request = JSON.parse(fs.readFileSync(requestPath, "utf8"));

const started = performance.now();
const result = solver.solve(payload, request);
const elapsed = performance.now() - started;

console.log(
  JSON.stringify(
    {
      optimal: result.optimal,
      truncated: result.truncated,
      nodes: result.nodes,
      leaves: result.leaves,
      cost: result.best?.total_price ?? null,
      message: result.message,
      ms: Math.round(elapsed),
    },
    null,
    2,
  ),
);
