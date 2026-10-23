import test from "node:test";
import assert from "node:assert/strict";
import {
  BASIC_SYNC_GROUPS,
  LAB_SYNC_METRICS,
  PRESET_IDS,
} from "../src/constraint-sync.mjs";
import { installSyncUi } from "../src/sync-ui.mjs";

const SYNC_MARK = "data-dwc-sync";

function matchesClass(element, className) {
  const value = element.className || "";
  return value.split(/\s+/).includes(className);
}

function matchesSelector(element, selector) {
  if (selector.startsWith(".")) return matchesClass(element, selector.slice(1));
  if (selector.startsWith("[")) {
    const match = selector.match(/^\[([^=\]]+)(?:="([^"]*)")?\]$/);
    if (!match) return false;
    const value = element.getAttribute(match[1]);
    return value != null && (match[2] === undefined || value === match[2]);
  }
  return element.tagName.toLowerCase() === selector.toLowerCase();
}

function descendants(node) {
  const out = [];
  for (const child of node.children) {
    out.push(child, ...descendants(child));
  }
  return out;
}

class FakeElement {
  constructor(tagName) {
    this.tagName = tagName.toUpperCase();
    this.children = [];
    this.parentNode = null;
    this.style = {};
    this.attributes = new Map();
    this.listeners = new Map();
    this.className = "";
    this.textContent = "";
    this.type = "";
    this.title = "";
    this.disabled = false;
  }

  appendChild(child) {
    child.parentNode = this;
    this.children.push(child);
    return child;
  }

  append(...nodes) {
    for (const node of nodes) this.appendChild(node);
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  getAttribute(name) {
    return this.attributes.has(name) ? this.attributes.get(name) : null;
  }

  addEventListener(type, listener) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(listener);
  }

  async click() {
    for (const listener of this.listeners.get("click") || []) {
      await listener({ type: "click", preventDefault() {}, stopPropagation() {} });
    }
  }

  querySelectorAll(selector) {
    return descendants(this).filter((node) => matchesSelector(node, selector));
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }
}

class FakeDocument {
  constructor() {
    this.body = new FakeElement("body");
    this.documentElement = new FakeElement("html");
    this.documentElement.appendChild(this.body);
  }

  createElement(tagName) {
    return new FakeElement(tagName);
  }

  querySelectorAll(selector) {
    return this.documentElement.querySelectorAll(selector);
  }

  querySelector(selector) {
    return this.querySelectorAll(selector)[0] || null;
  }
}

function element(doc, tagName, className, textContent) {
  const node = doc.createElement(tagName);
  if (className) node.className = className;
  if (textContent != null) node.textContent = textContent;
  return node;
}

function labRow(doc, label, dual) {
  const row = element(doc, "div");
  row.appendChild(element(doc, "span", "", label));
  if (dual) {
    const value = element(doc, "span", "gunsmith-lab-dual-value");
    value.appendChild(element(doc, "strong", "", "0.57x"));
    row.appendChild(value);
  } else {
    row.appendChild(element(doc, "strong", "", "411ms"));
  }
  return row;
}

function statRow(doc, label, value) {
  const row = element(doc, "div", "gunsmith-stat-row");
  row.appendChild(element(doc, "span", "gunsmith-stat-icon", ""));
  const copy = element(doc, "div", "gunsmith-stat-copy");
  const line = element(doc, "div");
  line.appendChild(element(doc, "span", "", label));
  line.appendChild(element(doc, "strong", "is-positive", value));
  copy.appendChild(line);
  copy.appendChild(element(doc, "div", "gunsmith-stat-track"));
  row.appendChild(copy);
  return row;
}

function isSyncButton(node) {
  return node.getAttribute(SYNC_MARK) != null;
}

function syncButtons(row) {
  return row.children.filter(isSyncButton);
}

function buildPage({ labLabels, statLabels }) {
  const doc = new FakeDocument();

  const list = element(doc, "div", "gunsmith-lab-detail-list");
  const labRows = labLabels.map((label, index) => {
    const row = labRow(doc, label, index % 2 === 1);
    list.appendChild(row);
    return row;
  });
  doc.body.appendChild(list);

  const statRows = statLabels.map((label) => {
    const row = statRow(doc, label, "70(+22)");
    doc.body.appendChild(row);
    return row;
  });

  const bar = element(doc, "div", "gunsmith-mincost-bar");
  doc.body.appendChild(bar);

  return { doc, labRows, statRows, list, bar };
}

const noopSync = async () => ({ synced: [], failed: [] });

test("injection is idempotent and limited to whitelisted rows", () => {
  const labLabels = [LAB_SYNC_METRICS[0].label, "行走速度"];
  const statLabels = [BASIC_SYNC_GROUPS[0].label, "基础伤害"];
  const { doc, labRows, statRows, bar } = buildPage({ labLabels, statLabels });
  const options = { doc, onStatus: () => {}, syncOne: noopSync, syncAll: noopSync };

  installSyncUi(options);
  installSyncUi(options);

  assert.equal(syncButtons(labRows[0]).length, 1);
  assert.equal(syncButtons(labRows[1]).length, 0);
  assert.equal(syncButtons(statRows[0]).length, 1);
  assert.equal(syncButtons(statRows[1]).length, 0);
  assert.equal(syncButtons(bar).length, 1);
  assert.equal(syncButtons(bar)[0].getAttribute(SYNC_MARK), "preset");

  const button = syncButtons(labRows[0])[0];
  assert.equal(button.textContent, "同步");
  assert.equal(button.type, "button");
  assert.equal(button.getAttribute(SYNC_MARK), LAB_SYNC_METRICS[0].id);
  assert.match(button.title, new RegExp(LAB_SYNC_METRICS[0].label));
  assert.match(button.title, new RegExp(LAB_SYNC_METRICS[0].id));
  assert.equal(labRows[0].style.position, "relative");
  assert.ok(labRows[0].style.paddingRight);
});

test("re-injection after rows are rebuilt adds one button per row", () => {
  const { doc, list, labRows } = buildPage({
    labLabels: [LAB_SYNC_METRICS[1].label],
    statLabels: [],
  });
  const options = { doc, onStatus: () => {}, syncOne: noopSync, syncAll: noopSync };
  installSyncUi(options);
  assert.equal(syncButtons(labRows[0]).length, 1);

  list.children = [];
  const rebuilt = labRow(doc, LAB_SYNC_METRICS[1].label, false);
  list.appendChild(rebuilt);
  installSyncUi(options);
  installSyncUi(options);

  assert.equal(syncButtons(rebuilt).length, 1);
});

test("single sync button calls syncMetric and reports the value", async () => {
  const metric = LAB_SYNC_METRICS[0];
  const { doc, labRows } = buildPage({ labLabels: [metric.label], statLabels: [] });
  const calls = [];
  const statuses = [];
  installSyncUi({
    doc,
    onStatus: (text, tone) => statuses.push([text, tone]),
    syncOne: async (targetDoc, id) => {
      calls.push([targetDoc, id]);
      return { synced: [{ id, value: 0.575 }], failed: [] };
    },
    syncAll: noopSync,
  });

  const button = syncButtons(labRows[0])[0];
  await button.click();

  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], doc);
  assert.equal(calls[0][1], metric.id);
  assert.deepEqual(statuses[0], [`已同步 ${metric.label} = 0.575`, "ok"]);
  assert.equal(button.disabled, false);
  assert.equal(button.textContent, "同步");
});

test("single sync failure and exception report a reason", async () => {
  const metric = LAB_SYNC_METRICS[0];
  const { doc, labRows } = buildPage({ labLabels: [metric.label], statLabels: [] });
  const statuses = [];
  let mode = "failed";
  installSyncUi({
    doc,
    onStatus: (text, tone) => statuses.push([text, tone]),
    syncOne: async () => {
      if (mode === "throw") throw new Error("页面未就绪");
      return { synced: [], failed: [{ id: metric.id, reason: "找不到目标行" }] };
    },
    syncAll: noopSync,
  });

  const button = syncButtons(labRows[0])[0];
  await button.click();
  assert.deepEqual(statuses[0], ["同步失败 · 找不到目标行", "error"]);

  mode = "throw";
  await button.click();
  assert.deepEqual(statuses[1], ["同步失败 · 页面未就绪", "error"]);
  assert.equal(button.disabled, false);
});

test("preset button calls syncMetrics with PRESET_IDS and reports counts", async () => {
  const { doc, bar } = buildPage({ labLabels: [], statLabels: [] });
  const calls = [];
  const statuses = [];
  let outcome = { synced: [{ id: "a" }, { id: "b" }], failed: [] };
  installSyncUi({
    doc,
    onStatus: (text, tone) => statuses.push([text, tone]),
    syncOne: noopSync,
    syncAll: async (targetDoc, ids) => {
      calls.push([targetDoc, ids]);
      return outcome;
    },
  });

  const button = syncButtons(bar)[0];
  await button.click();

  assert.equal(calls.length, 1);
  assert.equal(calls[0][0], doc);
  assert.deepEqual(calls[0][1], PRESET_IDS);
  assert.deepEqual(statuses[0], ["已同步 2 项约束，点击最低开销开始搜索", "ok"]);
  assert.equal(button.disabled, false);
  assert.equal(button.textContent, "一键同步");

  outcome = { synced: [{ id: "a" }], failed: [{ id: "b", reason: "不可计算" }] };
  await button.click();
  assert.deepEqual(statuses[1], [
    "已同步 1 项约束，失败 1 项 · 不可计算",
    "warn",
  ]);
});

test("a missing mincost bar does not break installation", () => {
  const doc = new FakeDocument();
  assert.doesNotThrow(() =>
    installSyncUi({ doc, onStatus: () => {}, syncOne: noopSync, syncAll: noopSync }),
  );
});
