import {
  BASIC_SYNC_GROUPS,
  LAB_SYNC_METRICS,
  PRESET_IDS,
  syncMetric,
  syncMetrics,
} from "./constraint-sync.mjs";
import { friendlyError } from "./status.mjs";

const SYNC_MARK = "data-dwc-sync";
const SYNC_FONT = "Bahnschrift, 'DIN Alternate', 'Microsoft YaHei', sans-serif";

const SYNC_ROW_BUTTON_STYLE = [
  "position:absolute",
  "right:6px",
  "top:50%",
  "transform:translateY(-50%)",
  "height:18px",
  "padding:0 6px",
  "border-radius:3px",
  "border:1px solid rgba(148,163,184,0.35)",
  "background:rgba(148,163,184,0.12)",
  "color:#cbd5e1",
  `font:600 10px/16px ${SYNC_FONT}`,
  "letter-spacing:0.02em",
  "cursor:pointer",
  "white-space:nowrap",
  "z-index:1",
].join(";");

const SYNC_PRESET_BUTTON_STYLE = [
  "width:100%",
  "height:26px",
  "padding:0 12px",
  "border-radius:4px",
  "border:1px solid rgba(148,163,184,0.35)",
  "background:rgba(148,163,184,0.08)",
  "color:#cbd5e1",
  `font:600 11px/1 ${SYNC_FONT}`,
  "letter-spacing:0.03em",
  "cursor:pointer",
  "transition:background .15s,border-color .15s",
].join(";");

function hasClass(node, name) {
  const className = node && typeof node.className === "string" ? node.className : "";
  return className.split(/\s+/).includes(name);
}

function tagOf(node) {
  return String((node && node.tagName) || "").toLowerCase();
}

function directChildren(node) {
  return Array.from((node && node.children) || []);
}

function directSpanText(row) {
  for (const child of directChildren(row)) {
    if (tagOf(child) === "span") return String(child.textContent || "").trim();
  }
  return null;
}

function statRowLabel(row) {
  for (const copy of directChildren(row)) {
    if (!hasClass(copy, "gunsmith-stat-copy")) continue;
    for (const div of directChildren(copy)) {
      if (tagOf(div) !== "div") continue;
      for (const span of directChildren(div)) {
        if (tagOf(span) === "span") return String(span.textContent || "").trim();
      }
    }
  }
  return null;
}

function markedChild(node, mark) {
  for (const child of directChildren(node)) {
    if (typeof child.getAttribute !== "function") continue;
    const value = child.getAttribute(SYNC_MARK);
    if (value != null && (mark == null || value === mark)) return child;
  }
  return null;
}

function formatValue(value) {
  return value == null ? "-" : String(value);
}

export function installSyncUi({
  doc,
  onStatus,
  syncOne = syncMetric,
  syncAll = syncMetrics,
} = {}) {
  if (!doc || typeof doc.createElement !== "function") return;

  function report(text, tone) {
    if (typeof onStatus === "function") onStatus(text, tone);
    else if (typeof console !== "undefined") console.log(`[gunsmith-mincost] ${text}`);
  }

  function errorText(error) {
    try {
      return friendlyError(error);
    } catch {
      return String(error && error.message ? error.message : error);
    }
  }

  function makeRowButton(metric) {
    const button = doc.createElement("button");
    button.type = "button";
    button.className = "gunsmith-mincost-sync";
    button.textContent = "同步";
    button.title = `同步当前${metric.label}到约束 ${metric.id}`;
    if (typeof button.setAttribute === "function") {
      button.setAttribute(SYNC_MARK, metric.id);
    }
    if (button.style) button.style.cssText = SYNC_ROW_BUTTON_STYLE;

    button.addEventListener("click", async (event) => {
      if (event && typeof event.preventDefault === "function") event.preventDefault();
      if (event && typeof event.stopPropagation === "function") event.stopPropagation();
      if (button.disabled) return;
      button.disabled = true;
      button.textContent = "同步中";
      try {
        const outcome = await syncOne(doc, metric.id);
        const synced = Array.isArray(outcome && outcome.synced) ? outcome.synced : [];
        const failed = Array.isArray(outcome && outcome.failed) ? outcome.failed : [];
        if (synced.length > 0) {
          report(`已同步 ${metric.label} = ${formatValue(synced[0].value)}`, "ok");
        } else if (failed.length > 0) {
          report(`同步失败 · ${failed[0].reason || "未知原因"}`, "error");
        } else {
          report(`同步失败 · ${metric.label} 没有可写入的值`, "error");
        }
      } catch (error) {
        report(`同步失败 · ${errorText(error)}`, "error");
      } finally {
        button.disabled = false;
        button.textContent = "同步";
      }
    });

    return button;
  }

  function injectRowButton(row, metric) {
    if (!row || typeof row.appendChild !== "function") return;
    if (markedChild(row, metric.id)) return;
    if (row.style) {
      if (!row.style.position) row.style.position = "relative";
      if (!row.style.paddingRight) row.style.paddingRight = "52px";
    }
    row.appendChild(makeRowButton(metric));
  }

  function injectLabButtons() {
    const byLabel = new Map(LAB_SYNC_METRICS.map((metric) => [metric.label, metric]));
    for (const list of Array.from(doc.querySelectorAll(".gunsmith-lab-detail-list"))) {
      for (const row of directChildren(list)) {
        const label = directSpanText(row);
        const metric = label ? byLabel.get(label) : null;
        if (metric) injectRowButton(row, metric);
      }
    }
  }

  function injectBasicButtons() {
    const byLabel = new Map(BASIC_SYNC_GROUPS.map((group) => [group.label, group]));
    for (const row of Array.from(doc.querySelectorAll(".gunsmith-stat-row"))) {
      const label = statRowLabel(row);
      const group = label ? byLabel.get(label) : null;
      if (group) injectRowButton(row, group);
    }
  }

  function injectPresetButton() {
    const bar = doc.querySelector(".gunsmith-mincost-bar");
    if (!bar || typeof bar.appendChild !== "function" || markedChild(bar, "preset")) return;

    const button = doc.createElement("button");
    button.type = "button";
    button.className = "gunsmith-mincost-sync-all";
    button.textContent = "一键同步";
    button.title = "把当前配装的指标值同步为硬条件, 再点击最低开销搜索";
    if (typeof button.setAttribute === "function") {
      button.setAttribute(SYNC_MARK, "preset");
    }
    if (button.style) button.style.cssText = SYNC_PRESET_BUTTON_STYLE;
    button.addEventListener("mouseenter", () => {
      if (!button.style) return;
      button.style.background = "rgba(148,163,184,0.16)";
      button.style.borderColor = "rgba(148,163,184,0.55)";
    });
    button.addEventListener("mouseleave", () => {
      if (!button.style) return;
      button.style.background = "rgba(148,163,184,0.08)";
      button.style.borderColor = "rgba(148,163,184,0.35)";
    });

    button.addEventListener("click", async (event) => {
      if (event && typeof event.preventDefault === "function") event.preventDefault();
      if (button.disabled) return;
      button.disabled = true;
      button.textContent = "同步中";
      try {
        const outcome = await syncAll(doc, PRESET_IDS);
        const synced = Array.isArray(outcome && outcome.synced) ? outcome.synced : [];
        const failed = Array.isArray(outcome && outcome.failed) ? outcome.failed : [];
        if (failed.length === 0) {
          report(`已同步 ${synced.length} 项约束，点击最低开销开始搜索`, "ok");
        } else {
          const reason = failed[0] && failed[0].reason ? ` · ${failed[0].reason}` : "";
          report(
            `已同步 ${synced.length} 项约束，失败 ${failed.length} 项${reason}`,
            synced.length > 0 ? "warn" : "error",
          );
        }
      } catch (error) {
        report(`同步失败 · ${errorText(error)}`, "error");
      } finally {
        button.disabled = false;
        button.textContent = "一键同步";
      }
    });

    bar.appendChild(button);
  }

  injectLabButtons();
  injectBasicButtons();
  injectPresetButton();
}
