import vm from "node:vm";
import { buildBundle } from "../build.mjs";

const hostSetTimeout = setTimeout;
const hostClearTimeout = clearTimeout;

const TEST_MARKER = "const hook = installWorkerHook();";
const TEST_HOOK = `${TEST_MARKER}
globalThis.__mincostTest = {
  hook,
  solveContext,
  getBackend,
  installWorkerHook,
  payloadCache,
  setStatusEl: (el) => {
    statusEl = el;
  },
};`;

let cachedBundle = null;

export function testBundle() {
  if (!cachedBundle) {
    const { bundle } = buildBundle({ wasmBytes: Buffer.from("test-wasm") });
    if (!bundle.includes(TEST_MARKER)) {
      throw new Error("test hook marker missing from bundle");
    }
    cachedBundle = bundle.replace(TEST_MARKER, TEST_HOOK);
  }
  return cachedBundle;
}

function fakeDocument() {
  return {
    body: {},
    readyState: "complete",
    documentElement: {},
    querySelector: () => null,
    querySelectorAll: () => [],
    createElement: () => ({
      style: {},
      addEventListener() {},
      append() {},
      insertAdjacentElement() {},
    }),
    addEventListener() {},
  };
}

export function createSandbox({
  autoAnswer = true,
  failInit = false,
  solverResult = { kind: "ok" },
} = {}) {
  const workers = [];
  const warnings = [];
  const errors = [];
  const queuedTimers = new Map();
  const solveRequests = [];
  const state = { autoAnswer, failInit, solverResult };
  const counters = { postToTerminated: 0 };
  let nextTimerId = 0;

  function schedule(fn, ms, ...args) {
    nextTimerId += 1;
    if (ms >= 3_000) {
      queuedTimers.set(nextTimerId, () => fn(...args));
      return nextTimerId;
    }
    return hostSetTimeout(fn, ms, ...args);
  }

  function cancelTimer(id) {
    if (!queuedTimers.delete(id)) hostClearTimeout(id);
  }

  class FakeWorker {
    constructor(url, options) {
      this.url = String(url);
      this.options = options;
      this.onmessage = null;
      this.onerror = null;
      this.received = [];
      this.terminated = false;
      this.terminateCalls = 0;
    }

    addEventListener() {}

    removeEventListener() {}

    terminate() {
      this.terminated = true;
      this.terminateCalls += 1;
    }

    postMessage(message) {
      this.received.push(message);
      if (this.terminated) {
        counters.postToTerminated += 1;
        return;
      }
      if (!this.url.startsWith("blob:")) return;
      if (!message || typeof message !== "object") return;
      if (message.type === "init") {
        if (state.failInit) {
          schedule(() => this.fireError("cannot load worker"), 0);
        } else {
          schedule(() => this.deliver({ type: "ready" }), 0);
        }
      } else if (message.type === "solve") {
        solveRequests.push(message);
        if (state.autoAnswer) {
          schedule(
            () =>
              this.deliver({
                type: "result",
                id: message.id,
                result: { ...state.solverResult, tag: this.url },
              }),
            1,
          );
        }
      }
    }

    deliver(data) {
      if (!this.terminated && this.onmessage) this.onmessage({ data });
    }

    fireError(message) {
      if (this.onerror) this.onerror({ message });
    }
  }

  const storage = new Map();
  const page = {
    Worker: function (url, options) {
      const worker = new FakeWorker(url, options);
      workers.push(worker);
      return worker;
    },
    localStorage: {
      getItem: (key) => (storage.has(key) ? storage.get(key) : null),
      setItem: (key, value) => storage.set(key, String(value)),
    },
  };

  const sandbox = {
    unsafeWindow: page,
    document: fakeDocument(),
    MutationObserver: class {
      observe() {}
    },
    requestAnimationFrame: (callback) => hostSetTimeout(callback, 0),
    URL: {
      createObjectURL: () => "blob:test-solver",
      revokeObjectURL() {},
    },
    Blob: class {},
    console: {
      log() {},
      info() {},
      debug() {},
      warn: (...args) => warnings.push(args),
      error: (...args) => errors.push(args),
    },
    setTimeout: schedule,
    clearTimeout: cancelTimer,
    performance: { now: () => Date.now() },
    WebAssembly,
    TextEncoder,
    TextDecoder,
    Buffer,
    alert: () => {},
  };
  sandbox.window = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(testBundle(), sandbox, { filename: "gunsmith-mincost.user.js" });

  return {
    sandbox,
    page,
    workers,
    warnings,
    errors,
    queuedTimers,
    solveRequests,
    counters,
    state,
    v: sandbox.__mincostTest,
    hook: sandbox.__mincostTest.hook,
    solverWorker: () => workers.find((worker) => worker.url.startsWith("blob:")),
    solverWorkers: () => workers.filter((worker) => worker.url.startsWith("blob:")),
    runQueuedTimers() {
      const pending = [...queuedTimers.values()];
      queuedTimers.clear();
      for (const timer of pending) timer();
    },
  };
}

export function testContext(dataVersion = "v1") {
  return {
    dataVersion,
    weaponId: "w1",
    mode: "operations",
    data: { marker: dataVersion },
    pricesByObjectId: {},
    currentSelections: {},
    tuningState: {},
    lockedPathKeys: [],
    lockedTuningKeys: [],
    excludedPartIds: [],
  };
}

export const TEST_OPTIONS = {
  constraints: [{ id: "recoil-continuous-horizontal", max: 0.4 }],
  mode: "fast",
};

export async function settle(promise, ms = 1_000) {
  let timer;
  const timeout = new Promise((resolvePromise) => {
    timer = hostSetTimeout(() => resolvePromise({ state: "pending" }), ms);
  });
  try {
    return await Promise.race([
      promise.then(
        (value) => ({ state: "resolved", value }),
        (error) => ({ state: "rejected", error }),
      ),
      timeout,
    ]);
  } finally {
    hostClearTimeout(timer);
  }
}

export function tick(ms = 10) {
  return new Promise((resolvePromise) => hostSetTimeout(resolvePromise, ms));
}

export async function waitFor(predicate, ms = 2_000) {
  const deadline = Date.now() + ms;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for condition");
    await tick(5);
  }
}
