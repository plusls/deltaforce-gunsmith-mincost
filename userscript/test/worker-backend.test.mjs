import test from "node:test";
import assert from "node:assert/strict";
import {
  createSandbox,
  settle,
  testContext,
  TEST_OPTIONS,
  tick,
} from "./harness.mjs";

test("worker error after ready invalidates backend and rebuilds on next solve", async () => {
  const sb = createSandbox();
  const first = await sb.v.solveContext(testContext(), TEST_OPTIONS);
  assert.equal(first.kind, "ok");

  const crashed = sb.solverWorkers().at(-1);
  crashed.fireError("boom after ready");
  assert.equal(crashed.terminated, true);

  const postsBefore = sb.counters.postToTerminated;
  const outcome = await settle(sb.v.solveContext(testContext("v2"), TEST_OPTIONS), 2_000);
  assert.equal(outcome.state, "resolved");
  assert.equal(outcome.value.kind, "ok");
  assert.equal(sb.solverWorkers().length, 2);
  assert.equal(sb.solverWorkers().at(-1).terminated, false);
  assert.equal(sb.counters.postToTerminated, postsBefore);
});

test("worker error rejects the in-flight solve and follows up with a rebuilt worker", async () => {
  const sb = createSandbox();
  await settle(sb.v.getBackend());
  sb.state.autoAnswer = false;

  const inflight = sb.v.solveContext(testContext("inflight"), TEST_OPTIONS);
  await tick(20);
  assert.equal(sb.solveRequests.length, 1);

  sb.solverWorker().fireError("boom in flight");
  const rejected = await settle(inflight, 1_000);
  assert.equal(rejected.state, "rejected");
  assert.match(String(rejected.error && rejected.error.message), /boom in flight/);

  sb.state.autoAnswer = true;
  const outcome = await settle(sb.v.solveContext(testContext("after"), TEST_OPTIONS), 2_000);
  assert.equal(outcome.state, "resolved");
  assert.equal(outcome.value.kind, "ok");
});

test("concurrent solves after a worker error all settle on one rebuilt worker", async () => {
  const sb = createSandbox();
  await sb.v.solveContext(testContext(), TEST_OPTIONS);
  sb.solverWorker().fireError("boom");

  const outcomes = await Promise.all([
    settle(sb.v.solveContext(testContext("a"), TEST_OPTIONS), 2_000),
    settle(sb.v.solveContext(testContext("b"), TEST_OPTIONS), 2_000),
  ]);
  for (const outcome of outcomes) {
    assert.equal(outcome.state, "resolved");
    assert.equal(outcome.value.kind, "ok");
  }
  assert.equal(sb.solverWorkers().length, 2);
  assert.equal(sb.solverWorkers().at(-1).received.filter((m) => m.type === "solve").length, 2);
});

test("a failed backend initialization is not cached across calls", async () => {
  const sb = createSandbox({ failInit: true });
  const first = await settle(sb.v.getBackend(), 2_000);
  assert.equal(first.state, "rejected");
  const second = await settle(sb.v.getBackend(), 2_000);
  assert.equal(second.state, "rejected");
  assert.equal(sb.solverWorkers().length, 2);
});
