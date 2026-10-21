const MINCOST_WASM_BASE64 = "__WASM_BASE64__";
const MINCOST_WORKER_SOURCE = "__WORKER_SOURCE__";
const MINCOST_SETTINGS_KEY = "moligod:gunsmith-mincost:v1";

const page = typeof unsafeWindow !== "undefined" ? unsafeWindow : window;
const RealPageWorker = page.Worker;

const defaultSettings = {
  mode: "full",
  priceSource: "current",
  missingPricePolicy: "reject",
};
const settings = loadSettings();

function loadSettings() {
  try {
    const raw = page.localStorage?.getItem(MINCOST_SETTINGS_KEY);
    const parsed = raw ? JSON.parse(raw) : null;
    return { ...defaultSettings, ...(parsed && typeof parsed === "object" ? parsed : {}) };
  } catch {
    return { ...defaultSettings };
  }
}

function saveSettings() {
  try {
    page.localStorage?.setItem(MINCOST_SETTINGS_KEY, JSON.stringify(settings));
  } catch {
    /* ignore */
  }
}

function dataKeyOf(context) {
  return `${context.dataVersion}:${context.weaponId}:${context.mode}`;
}

let backendPromise = null;
const payloadCache = new Map();

function getBackend() {
  if (backendPromise) return backendPromise;
  backendPromise = (async () => {
    const bytes = decodeBase64(MINCOST_WASM_BASE64);
    try {
      const worker = await createSolverWorkerBackend(bytes);
      return { worker };
    } catch (error) {
      console.warn("[gunsmith-mincost] solver worker unavailable, using main thread", error);
      const engine = await createMinCostEngine({ wasm: bytes });
      return { local: true, engine };
    }
  })();
  return backendPromise;
}

function createSolverWorkerBackend(wasmBytes) {
  return new Promise((resolve, reject) => {
    let blobUrl;
    let worker;
    try {
      blobUrl = URL.createObjectURL(
        new Blob([MINCOST_WORKER_SOURCE], { type: "text/javascript" }),
      );
      worker = new RealPageWorker(blobUrl);
    } catch (error) {
      if (blobUrl) URL.revokeObjectURL(blobUrl);
      reject(error);
      return;
    }

    const pending = new Map();
    let nextId = 1;
    let ready = false;
    const timer = setTimeout(() => {
      fail(new Error("solver worker init timeout"));
    }, 10_000);

    function fail(error) {
      clearTimeout(timer);
      worker.terminate();
      if (blobUrl) URL.revokeObjectURL(blobUrl);
      for (const entry of pending.values()) entry.reject(error);
      pending.clear();
      reject(error);
    }

    worker.onmessage = (event) => {
      const message = event.data;
      if (!message || typeof message !== "object") return;
      if (message.type === "ready") {
        ready = true;
        clearTimeout(timer);
        resolve({
          solve(context, options, key, data) {
            if (!ready) return Promise.reject(new Error("solver worker not ready"));
            const id = nextId++;
            return new Promise((resolveOne, rejectOne) => {
              pending.set(id, { resolve: resolveOne, reject: rejectOne });
              worker.postMessage({
                type: "solve",
                id,
                key,
                data,
                context,
                options,
              });
            });
          },
          terminate() {
            worker.terminate();
            if (blobUrl) URL.revokeObjectURL(blobUrl);
          },
        });
        return;
      }
      if (message.type === "init-error") {
        fail(new Error(message.message || "solver worker init failed"));
        return;
      }
      if (message.type === "result" || message.type === "error") {
        const entry = pending.get(message.id);
        if (!entry) return;
        pending.delete(message.id);
        if (message.type === "result") entry.resolve(message.result);
        else entry.reject(new Error(message.message || "solver failed"));
      }
    };

    worker.onerror = (event) => {
      fail(new Error((event && event.message) || "solver worker error"));
    };

    worker.postMessage({ type: "init", wasm: wasmBytes });
  });
}

async function solveContext(context, options) {
  const backend = await getBackend();
  if (backend.local) return backend.engine.solve(context, options);

  const key = dataKeyOf(context);
  const data = payloadCache.has(key) ? undefined : context.data;
  payloadCache.set(key, context.data);
  return backend.worker.solve({ ...context, data: undefined }, options, key, data);
}

async function runMinCost(context, mode, options) {
  const merged = {
    ...options,
    mode,
    priceSource: settings.priceSource,
    missingPricePolicy: settings.missingPricePolicy,
    deadlineMs: mode === "fast" ? 2_000 : 5_000,
  };
  if (merged.constraints.length === 0 && merged.costCap == null) {
    throw new Error("请先在智能配装面板设置至少一个硬条件");
  }
  return solveContext(context, merged);
}

function installWorkerHook() {
  const sitePayloadCache = new Map();
  let pendingMinCost = null;
  let pendingToken = 0;

  function trigger(mode) {
    pendingToken += 1;
    const token = pendingToken;
    pendingMinCost = { mode };
    setTimeout(() => {
      if (token === pendingToken && pendingMinCost) pendingMinCost = null;
    }, 3_000);
  }

  function PatchedWorker(url, options) {
    const real = new RealPageWorker(url, options);
    if (!/optimizer\.worker/.test(String(url))) return real;

    let onmessage = null;
    let onerror = null;
    const listeners = new Set();
    const errorListeners = new Set();
    const queued = [];

    real.addEventListener("message", (event) => {
      if (onmessage) onmessage(event);
      for (const listener of listeners) listener(event);
    });
    real.addEventListener("error", (event) => {
      if (onerror) onerror(event);
      for (const listener of errorListeners) listener(event);
    });

    const proxy = {
      postMessage(message, ...rest) {
        if (!message || typeof message !== "object") {
          real.postMessage(message, ...rest);
          return;
        }

        if (message.data && message.dataKey) {
          sitePayloadCache.set(message.dataKey, message.data);
        }

        if (pendingMinCost && message.purpose === "solve") {
          const mode = pendingMinCost.mode;
          pendingMinCost = null;
          pendingToken += 1;

          const emit = (payload) => {
            const event = {
              data: {
                requestId: message.requestId,
                purpose: message.purpose,
                ...payload,
              },
            };
            if (onmessage || listeners.size > 0) {
              if (onmessage) onmessage(event);
              for (const listener of listeners) listener(event);
            } else {
              queued.push(event);
            }
          };

          let context;
          try {
            context = captureWorkerRequest(message, sitePayloadCache);
          } catch (error) {
            emit({ error: friendlyError(error) });
            return;
          }

          (async () => {
            try {
              const options = optionsFromSiteRequest(message, {});
              const result = await runMinCost(context, mode, options);
              page.__moligodMinCostLast = {
                requestId: message.requestId,
                mode,
                options: { ...options, mode },
                context: {
                  dataVersion: context.dataVersion,
                  weaponId: context.weaponId,
                  mode: context.mode,
                  laserEnabled: context.laserEnabled,
                  pricesByObjectId: context.pricesByObjectId,
                  lockedPathKeys: context.lockedPathKeys,
                  lockedTuningKeys: context.lockedTuningKeys,
                  excludedPartIds: context.excludedPartIds,
                  currentSelections: context.currentSelections,
                },
                result,
              };
              emit({ result: toSiteResult(context, result) });
            } catch (error) {
              emit({ error: friendlyError(error) });
            }
          })();
          return;
        }

        let outgoing = message;
        if (
          message.data == null &&
          message.dataKey != null &&
          sitePayloadCache.has(message.dataKey)
        ) {
          outgoing = { ...message, data: sitePayloadCache.get(message.dataKey) };
        }
        real.postMessage(outgoing, ...rest);
      },
      terminate() {
        real.terminate();
      },
      addEventListener(type, listener) {
        if (type === "message") listeners.add(listener);
        else if (type === "error") errorListeners.add(listener);
        else real.addEventListener(type, listener);
      },
      removeEventListener(type, listener) {
        if (type === "message") listeners.delete(listener);
        else if (type === "error") errorListeners.delete(listener);
        else real.removeEventListener(type, listener);
      },
      get onmessage() {
        return onmessage;
      },
      set onmessage(handler) {
        onmessage = handler;
        while (queued.length > 0 && onmessage) onmessage(queued.shift());
      },
      get onerror() {
        return onerror;
      },
      set onerror(handler) {
        onerror = handler;
      },
    };

    return proxy;
  }

  PatchedWorker.prototype = RealPageWorker.prototype;
  try {
    page.Worker = PatchedWorker;
  } catch (error) {
    console.warn("[gunsmith-mincost] failed to hook Worker", error);
  }

  return { trigger };
}

function makeSelect(values, current, labels) {
  const select = document.createElement("select");
  select.style.cssText =
    "padding:5px 6px;border-radius:6px;border:1px solid #555;background:#1b1b1b;color:#ddd;font-size:12px";
  for (const value of values) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = labels[value] || value;
    if (value === current) option.selected = true;
    select.append(option);
  }
  return select;
}

function injectButton(trigger) {
  if (!document.body) return;
  const siteButton = document.querySelector(".gunsmith-optimizer-generate");
  if (!siteButton || document.querySelector(".gunsmith-mincost-bar")) return;

  const bar = document.createElement("div");
  bar.className = "gunsmith-mincost-bar";
  bar.style.cssText =
    "display:flex;flex-wrap:wrap;gap:6px;align-items:center;margin:8px 0 4px";

  const button = document.createElement("button");
  button.type = "button";
  button.className = "gunsmith-mincost-run";
  button.textContent = "最低开销";
  button.title = "在硬条件约束下搜索总价最低的配件方案";
  button.style.cssText =
    "padding:6px 12px;border-radius:6px;border:1px solid #42a36b;background:#1f8a55;color:#fff;font-weight:600;cursor:pointer";

  const mode = makeSelect(["full", "fast"], settings.mode, {
    full: "完整模式",
    fast: "快速模式",
  });
  mode.title = "完整模式证明最优, 快速模式只找可行解";

  const price = makeSelect(["current", "system"], settings.priceSource, {
    current: "市场价",
    system: "系统价",
  });
  price.title = "最小化的价格口径";

  const missing = makeSelect(["reject", "fallback-system"], settings.missingPricePolicy, {
    reject: "缺价排除",
    "fallback-system": "缺价用系统价",
  });
  missing.title = "市场价缺失时的处理方式";

  bar.append(button, mode, price, missing);
  siteButton.insertAdjacentElement("afterend", bar);

  button.addEventListener("click", () => {
    const target = document.querySelector(".gunsmith-optimizer-generate");
    if (!target) {
      alert("智能配装面板尚未就绪");
      return;
    }
    if (target.disabled === true || target.getAttribute("aria-disabled") === "true") {
      alert("智能配装面板当前不可用");
      return;
    }
    settings.mode = mode.value;
    settings.priceSource = price.value;
    settings.missingPricePolicy = missing.value;
    saveSettings();
    trigger(settings.mode);
    target.click();
  });
}

function friendlyError(error) {
  if (error && error.name === "UnsupportedConstraintError") {
    return `暂不支持这些约束: ${error.constraints.join(", ")}`;
  }
  if (error && error.name === "UnsupportedFeatureError") {
    if (error.feature === "preparedness") return "暂不支持战备下限约束";
    return `暂不支持: ${error.feature}`;
  }
  return String(error && error.message ? error.message : error);
}

const hook = installWorkerHook();
const boot = () => injectButton(hook.trigger);

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", boot, { once: true });
} else {
  boot();
}

let scheduled = false;
const observer = new MutationObserver(() => {
  if (scheduled) return;
  scheduled = true;
  requestAnimationFrame(() => {
    scheduled = false;
    boot();
  });
});
if (document.documentElement) {
  observer.observe(document.documentElement, { childList: true, subtree: true });
}
