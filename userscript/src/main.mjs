import {
  describeResultStatus,
  formatDuration,
  normalizeTimeLimitSeconds,
  resolveDeadlineMs,
  selectSiteSearchSeconds,
  siteOptimizerInputsKey,
  siteSearchSecondsFromRecord,
  SITE_SEARCH_INPUT_SELECTOR,
  SITE_SEARCH_SECONDS_MAX,
  SITE_SEARCH_SECONDS_MIN,
  SOLVE_DEADLINE_MAX_MS,
  SOLVE_DEADLINE_MIN_MS,
} from "../../solver/js/deadline.mjs";

const MINCOST_WASM_BASE64 = "__WASM_BASE64__";
const MINCOST_WORKER_SOURCE = "__WORKER_SOURCE__";
const MINCOST_SETTINGS_KEY = "moligod:gunsmith-mincost:v1";
const SITE_BIPOD_GROUP_SELECTOR = '[aria-label="脚架状态"]';
const SITE_DRAFT_KEY = "moligod:gunsmith-draft:v1";

const page = typeof unsafeWindow !== "undefined" ? unsafeWindow : window;
const RealPageWorker = page.Worker;

const defaultSettings = {
  mode: "full",
  priceSource: "current",
  missingPricePolicy: "reject",
  timeLimitSeconds: null,
};
const settings = loadSettings();

function loadSettings() {
  try {
    const raw = page.localStorage?.getItem(MINCOST_SETTINGS_KEY);
    const parsed = raw ? JSON.parse(raw) : null;
    const merged = {
      ...defaultSettings,
      ...(parsed && typeof parsed === "object" ? parsed : {}),
    };
    merged.timeLimitSeconds = normalizeTimeLimitSeconds(merged.timeLimitSeconds);
    return merged;
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

let statusEl = null;
const STATUS_COLORS = {
  info: "#a2a5ad",
  ok: "#86efac",
  warn: "#fbbf24",
  error: "#f87171",
};

function reportStatus(text, tone = "info") {
  if (!statusEl || !statusEl.isConnected) return;
  statusEl.textContent = text;
  statusEl.style.color = STATUS_COLORS[tone] ?? STATUS_COLORS.info;
}

function dataKeyOf(context) {
  return `${context.dataVersion}:${context.weaponId}:${context.mode}`;
}

let backendPromise = null;
let backendToken = 0;
const PAYLOAD_CACHE_LIMIT = 4;

class BoundedPayloadCache extends Map {
  constructor(limit) {
    super();
    this.limit = limit;
  }

  get(key) {
    if (!super.has(key)) return undefined;
    const value = super.get(key);
    super.delete(key);
    super.set(key, value);
    return value;
  }

  set(key, value) {
    if (super.has(key)) super.delete(key);
    super.set(key, value);
    while (this.size > this.limit) super.delete(super.keys().next().value);
    return this;
  }
}

const payloadCache = new BoundedPayloadCache(PAYLOAD_CACHE_LIMIT);

function getBackend() {
  if (backendPromise) return backendPromise;
  const token = ++backendToken;
  const attempt = (async () => {
    const bytes = decodeBase64(MINCOST_WASM_BASE64);
    try {
      const worker = await createSolverWorkerBackend(bytes, () => {
        if (backendToken === token) backendPromise = null;
      });
      return { worker };
    } catch (error) {
      console.warn("[gunsmith-mincost] solver worker unavailable, using main thread", error);
      const engine = await createMinCostEngine({ wasm: bytes });
      return { local: true, engine };
    }
  })();
  backendPromise = attempt;
  attempt.catch(() => {
    if (backendToken === token) backendPromise = null;
  });
  return attempt;
}

function createSolverWorkerBackend(wasmBytes, onFatal) {
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
    let failed = false;
    let settled = false;
    const timer = setTimeout(() => {
      fail(new Error("solver worker init timeout"));
    }, 10_000);

    function fail(error) {
      if (failed) return;
      failed = true;
      const wasReady = ready;
      ready = false;
      clearTimeout(timer);
      worker.terminate();
      if (blobUrl) URL.revokeObjectURL(blobUrl);
      for (const entry of pending.values()) entry.reject(error);
      pending.clear();
      if (!settled) {
        settled = true;
        reject(error);
        return;
      }
      console.warn("[gunsmith-mincost] solver worker failed, rebuilding on next solve", error);
      if (wasReady && onFatal) onFatal(error);
    }

    worker.onmessage = (event) => {
      const message = event.data;
      if (!message || typeof message !== "object") return;
      if (message.type === "ready") {
        if (failed) return;
        ready = true;
        settled = true;
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
            failed = true;
            ready = false;
            clearTimeout(timer);
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

function readSiteSearchSeconds(context) {
  let domSeconds = null;
  const inputs = document.querySelectorAll(SITE_SEARCH_INPUT_SELECTOR);
  const input = [...inputs].find((node) => node.disabled !== true) ?? inputs[0];
  if (input) {
    const value = Number(input.value);
    if (Number.isFinite(value) && value > 0) domSeconds = value;
  }

  let storedSeconds = null;
  try {
    const key = siteOptimizerInputsKey(context?.weaponId, context?.mode);
    const raw = page.localStorage?.getItem(key);
    storedSeconds = siteSearchSecondsFromRecord(raw ? JSON.parse(raw) : null);
  } catch {
    /* ignore */
  }

  return selectSiteSearchSeconds({ domSeconds, storedSeconds });
}

function readSiteBipodDeployed(context) {
  const group = document.querySelector(SITE_BIPOD_GROUP_SELECTOR);
  if (group) {
    const active = group.querySelector('button[aria-pressed="true"]');
    if (active) return /展开/.test(active.textContent || "");
  }

  try {
    const raw = page.localStorage?.getItem(SITE_DRAFT_KEY);
    const draft = raw ? JSON.parse(raw) : null;
    if (
      draft &&
      draft.weaponId === context?.weaponId &&
      draft.mode === context?.mode
    ) {
      return draft.bipodDeployed === true;
    }
  } catch {
    /* ignore */
  }

  return context?.bipodDeployed === true;
}

async function runMinCost(context, mode, options, requestDeadlineMs) {
  const { deadlineMs, source } = resolveDeadlineMs({
    mode,
    timeLimitSeconds: settings.timeLimitSeconds,
    siteSeconds: readSiteSearchSeconds(context),
    requestDeadlineMs,
  });
  const bipodDeployed =
    typeof options.bipodDeployed === "boolean"
      ? options.bipodDeployed
      : readSiteBipodDeployed(context);
  const merged = {
    ...options,
    mode,
    priceSource: settings.priceSource,
    missingPricePolicy: settings.missingPricePolicy,
    bipodDeployed,
    deadlineMs,
  };
  if (merged.constraints.length === 0 && merged.costCap == null) {
    throw new Error("请先在智能配装面板设置至少一个硬条件");
  }
  reportStatus(`求解中 · 时限 ${formatDuration(deadlineMs)}`, "info");
  const result = await solveContext(context, merged);
  return { ...result, deadlineMs, deadlineSource: source, bipodDeployed };
}

function requestWithCachedData(message, cache) {
  if (message.data != null) return message;
  if (message.dataKey == null || !cache.has(message.dataKey)) return message;
  return { ...message, data: cache.get(message.dataKey) };
}

function installWorkerHook() {
  const sitePayloadCache = new BoundedPayloadCache(PAYLOAD_CACHE_LIMIT);
  let pendingMinCost = null;
  let pendingToken = 0;
  let hookFailed = false;

  function trigger(mode) {
    pendingToken += 1;
    const token = pendingToken;
    pendingMinCost = { mode, warned: false };
    if (hookFailed) {
      pendingMinCost = null;
      pendingToken += 1;
      reportStatus("未接管 · 无法替换站点 Worker, 本次显示站点原方案", "warn");
      console.warn("[gunsmith-mincost] worker hook was not installed; site solver result used");
      return;
    }
    setTimeout(() => {
      if (token !== pendingToken || !pendingMinCost) return;
      pendingMinCost = null;
      reportStatus("未接管 · 3 秒内未收到站点求解请求, 本次显示站点原方案", "warn");
      console.warn(
        "[gunsmith-mincost] no site solve request captured within 3s; site solver result used",
      );
    }, 3_000);
  }

  function noteWorkerMismatch(url) {
    if (!pendingMinCost || pendingMinCost.warned) return;
    if (!/optimizer|solver/i.test(String(url))) return;
    pendingMinCost.warned = true;
    reportStatus("未接管 · 站点求解 Worker 地址不匹配, 本次显示站点原方案", "warn");
    console.warn("[gunsmith-mincost] site worker URL did not match:", String(url));
  }

  function PatchedWorker(url, options) {
    const real = new RealPageWorker(url, options);
    if (!/optimizer\.worker/.test(String(url))) {
      noteWorkerMismatch(url);
      return real;
    }

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

          const request = requestWithCachedData(message, sitePayloadCache);
          let context;
          try {
            context = captureWorkerRequest(request, sitePayloadCache);
          } catch (error) {
            reportStatus(friendlyError(error), "error");
            emit({ error: friendlyError(error) });
            return;
          }

          (async () => {
            try {
              const options = optionsFromSiteRequest(request, {});
              const result = await runMinCost(
                context,
                mode,
                options,
                message.timeBudgetMs,
              );
              page.__moligodMinCostLast = {
                requestId: message.requestId,
                mode,
                options: {
                  ...options,
                  mode,
                  deadlineMs: result.deadlineMs,
                  deadlineSource: result.deadlineSource,
                  bipodDeployed: result.bipodDeployed,
                },
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
              const status = describeResultStatus(result, result.deadlineMs);
              reportStatus(status.text, status.tone);
              if (result.kind === "error" || result.kind === "incomplete") {
                emit({ error: status.text });
              } else {
                emit({ result: toSiteResult(context, result) });
              }
            } catch (error) {
              reportStatus(friendlyError(error), "error");
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
    if (page.Worker !== PatchedWorker) {
      throw new Error("Worker assignment did not take effect");
    }
  } catch (error) {
    hookFailed = true;
    console.warn("[gunsmith-mincost] failed to hook Worker", error);
  }

  return { trigger, sitePayloadCache };
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

function makeNumberField(text, value, title, onChange) {
  const label = document.createElement("label");
  label.style.cssText =
    "display:inline-flex;align-items:center;gap:5px;white-space:nowrap";
  label.title = title;

  const span = document.createElement("span");
  span.textContent = text;

  const input = document.createElement("input");
  input.type = "number";
  input.min = String(SOLVE_DEADLINE_MIN_MS / 1000);
  input.max = String(SOLVE_DEADLINE_MAX_MS / 1000);
  input.step = "1";
  input.placeholder = "自动";
  if (value != null) input.value = String(value);
  input.style.cssText = [
    "width:46px",
    "height:18px",
    "padding:0 4px",
    "border-radius:3px",
    "border:1px solid rgba(255,255,255,0.18)",
    "background:rgba(0,0,0,0.25)",
    "color:#e5e7eb",
    `font:500 11px/16px ${MINCOST_FONT}`,
    "text-align:right",
  ].join(";");
  input.addEventListener("change", () => {
    const seconds = normalizeTimeLimitSeconds(input.value);
    input.value = seconds == null ? "" : String(seconds);
    onChange(seconds);
  });

  const unit = document.createElement("small");
  unit.textContent = "秒";
  unit.style.opacity = "0.7";

  label.append(span, input, unit);
  return { label, input };
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
  const limit = makeNumberField(
    "时限",
    settings.timeLimitSeconds,
    `留空: 跟随站点"搜索"时长 (${SITE_SEARCH_SECONDS_MIN}-${SITE_SEARCH_SECONDS_MAX} 秒, 取不到时用站点请求预算); 自定义 ${SOLVE_DEADLINE_MIN_MS / 1000}-${SOLVE_DEADLINE_MAX_MS / 1000} 秒. 到时限后返回当前最好方案; 求解在独立 Worker 中运行, 页面不会卡顿`,
    (seconds) => {
      settings.timeLimitSeconds = seconds;
      saveSettings();
    },
  );

  function syncMissing() {
    const disabled = !price.input.checked;
    missing.input.disabled = disabled;
    missing.label.style.opacity = disabled ? "0.45" : "1";
    missing.label.style.cursor = disabled ? "default" : "pointer";
  }
  price.input.addEventListener("change", syncMissing);
  syncMissing();

  const status = document.createElement("div");
  status.className = "gunsmith-mincost-status";
  status.style.cssText = `font:500 11px/16px ${MINCOST_FONT};color:${STATUS_COLORS.info};min-height:16px`;
  statusEl = status;

  options.append(mode.label, price.label, missing.label, limit.label);
  bar.append(button, options, status);
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
    reportStatus("准备中 · 等待站点请求", "info");
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
