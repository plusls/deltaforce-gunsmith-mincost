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

const MINCOST_FONT =
  "Bahnschrift, 'DIN Alternate', 'Microsoft YaHei', sans-serif";

function makeToggle(text, checked, title) {
  const label = document.createElement("label");
  label.style.cssText =
    "display:inline-flex;align-items:center;gap:5px;cursor:pointer;user-select:none;white-space:nowrap";
  label.title = title;

  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = checked;
  input.style.cssText =
    "margin:0;width:13px;height:13px;accent-color:#22c55e;cursor:pointer";

  const span = document.createElement("span");
  span.textContent = text;

  label.append(input, span);
  return { label, input, span };
}

function injectButton(trigger) {
  if (!document.body) return;
  const siteButton = document.querySelector(".gunsmith-optimizer-generate");
  if (!siteButton || document.querySelector(".gunsmith-mincost-bar")) return;

  const bar = document.createElement("div");
  bar.className = "gunsmith-mincost-bar";
  bar.style.cssText = "display:grid;gap:6px;margin-top:6px";

  const button = document.createElement("button");
  button.type = "button";
  button.className = "gunsmith-mincost-run";
  button.textContent = "最低开销";
  button.title = "在硬条件约束下搜索总价最低的配件方案";
  button.style.cssText = [
    "width:100%",
    "height:34px",
    "padding:0 12px",
    "border-radius:4px",
    "border:1px solid rgba(34,197,94,0.45)",
    "background:rgba(34,197,94,0.16)",
    "color:#86efac",
    `font:700 12px/1 ${MINCOST_FONT}`,
    "letter-spacing:0.04em",
    "cursor:pointer",
    "transition:background .15s,border-color .15s",
  ].join(";");
  button.addEventListener("mouseenter", () => {
    button.style.background = "rgba(34,197,94,0.26)";
    button.style.borderColor = "rgba(34,197,94,0.65)";
  });
  button.addEventListener("mouseleave", () => {
    button.style.background = "rgba(34,197,94,0.16)";
    button.style.borderColor = "rgba(34,197,94,0.45)";
  });

  const options = document.createElement("div");
  options.style.cssText = [
    "display:flex",
    "flex-wrap:wrap",
    "gap:10px",
    "align-items:center",
    `font:500 11px/16px ${MINCOST_FONT}`,
    "color:#a2a5ad",
  ].join(";");

  const mode = makeToggle(
    "完整模式",
    settings.mode !== "fast",
    "勾选: 完整搜索并证明最优; 取消: 快速模式, 只找第一个可行解",
  );
  const price = makeToggle(
    "市场价",
    settings.priceSource !== "system",
    "勾选: 按市场价最小化; 取消: 按系统价最小化",
  );
  const missing = makeToggle(
    "缺价排除",
    settings.missingPricePolicy !== "fallback-system",
    "市场价缺失时: 勾选排除该配件; 取消用系统价代替",
  );

  function syncMissing() {
    const disabled = !price.input.checked;
    missing.input.disabled = disabled;
    missing.label.style.opacity = disabled ? "0.45" : "1";
    missing.label.style.cursor = disabled ? "default" : "pointer";
  }
  price.input.addEventListener("change", syncMissing);
  syncMissing();

  options.append(mode.label, price.label, missing.label);
  bar.append(button, options);
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
    settings.mode = mode.input.checked ? "full" : "fast";
    settings.priceSource = price.input.checked ? "current" : "system";
    settings.missingPricePolicy = missing.input.checked ? "reject" : "fallback-system";
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
