import test from "node:test";
import assert from "node:assert/strict";
import {
  BASIC_SYNC_GROUPS,
  LAB_SYNC_METRICS,
  PRESET_IDS,
  computeConstraint,
  parseLeadingNumber,
  syncMetric,
  syncMetrics,
} from "../src/constraint-sync.mjs";

const EXPECTED_LAB_IDS = [
  "recoil-continuous-vertical",
  "recoil-continuous-horizontal",
  "recoil-single-vertical",
  "recoil-single-horizontal",
  "recoil-fire-stability",
  "handling-aim-time",
  "handling-sprint-fire-delay",
  "handling-aim-move-speed",
  "handling-reload-time",
  "handling-empty-reload-time",
  "handling-holster-time",
  "handling-switch-time",
  "stability-breath-range",
  "stability-breath-duration",
  "stability-hit-sway",
  "stability-movement-camera",
  "stability-fire-camera",
  "stability-hold-breath-recoil",
  "hipfire-standing-spread",
  "hipfire-maximum-spread",
];

const LAB_LTE = { id: "lab-lte", source: "lab", operator: "lte" };
const LAB_GTE = { id: "lab-gte", source: "lab", operator: "gte" };
const BASIC_GTE = { id: "basic-gte", source: "basic", operator: "gte" };
const BASIC_LTE = { id: "basic-lte", source: "basic", operator: "lte" };

function fakeElement({
  tagName = "DIV",
  textContent = "",
  value = "",
  disabled = false,
  attrs = {},
  classes = [],
  children = [],
  childNodes = [],
  nodes = {},
} = {}) {
  const classNames = new Set(classes);
  const element = {
    tagName,
    textContent,
    value,
    disabled,
    children,
    childNodes,
    events: [],
    clicks: 0,
    classList: {
      add(name) {
        classNames.add(name);
      },
      remove(name) {
        classNames.delete(name);
      },
      contains(name) {
        return classNames.has(name);
      },
    },
    getAttribute(name) {
      return Object.prototype.hasOwnProperty.call(attrs, name) ? attrs[name] : null;
    },
    click() {
      element.clicks += 1;
    },
    dispatchEvent(event) {
      element.events.push(event);
      return true;
    },
    querySelector(selector) {
      const found = nodes[selector];
      return Array.isArray(found) ? null : found ?? null;
    },
    querySelectorAll(selector) {
      const found = nodes[selector];
      if (Array.isArray(found)) return found;
      return found ? [found] : [];
    },
  };
  return element;
}

function fakeDoc({ ids = {}, nodes = {} } = {}) {
  return {
    defaultView: undefined,
    getElementById(id) {
      return ids[id] ?? null;
    },
    querySelector(selector) {
      const found = nodes[selector];
      return Array.isArray(found) ? null : found ?? null;
    },
    querySelectorAll(selector) {
      const found = nodes[selector];
      if (Array.isArray(found)) return found;
      return found ? [found] : [];
    },
  };
}

function makeTabGroup(labels, activeLabel) {
  const buttons = labels.map((label) =>
    fakeElement({
      tagName: "BUTTON",
      textContent: label,
      classes: label === activeLabel ? ["is-active"] : [],
    }),
  );
  for (const button of buttons) {
    button.click = () => {
      button.clicks += 1;
      for (const other of buttons) other.classList.remove("is-active");
      button.classList.add("is-active");
    };
  }
  return buttons;
}

function makeOptimizerTabs(active) {
  const basic = fakeElement({
    tagName: "BUTTON",
    classes: active === "basic" ? ["is-active"] : [],
  });
  const advanced = fakeElement({
    tagName: "BUTTON",
    classes: active === "advanced" ? ["is-active"] : [],
  });
  basic.click = () => {
    basic.clicks += 1;
    basic.classList.add("is-active");
    advanced.classList.remove("is-active");
  };
  advanced.click = () => {
    advanced.clicks += 1;
    advanced.classList.add("is-active");
    basic.classList.remove("is-active");
  };
  return { basic, advanced };
}

test("parseLeadingNumber reads only the leading number", () => {
  assert.equal(parseLeadingNumber("70(+22)"), 70);
  assert.equal(parseLeadingNumber("44(-13)"), 44);
  assert.equal(parseLeadingNumber("54(0)"), 54);
  assert.equal(parseLeadingNumber("0.57x"), 0.57);
  assert.equal(parseLeadingNumber("411ms"), 411);
  assert.equal(parseLeadingNumber("2.282s"), 2.282);
  assert.equal(parseLeadingNumber("1.811m/s"), 1.811);
  assert.equal(parseLeadingNumber("14.5%"), 14.5);
  assert.equal(parseLeadingNumber("  0.3° "), 0.3);
  assert.equal(parseLeadingNumber(""), null);
  assert.equal(parseLeadingNumber("abc"), null);
  assert.equal(parseLeadingNumber(null), null);
});

test("computeConstraint follows displayed decimals for lab rows", () => {
  assert.deepEqual(computeConstraint(LAB_LTE, "0.57x"), { operator: "lte", value: 0.575 });
  assert.deepEqual(computeConstraint(LAB_LTE, "411ms"), { operator: "lte", value: 411.5 });
  assert.deepEqual(computeConstraint(LAB_LTE, "266.3ms"), {
    operator: "lte",
    value: 266.35,
  });
  assert.deepEqual(computeConstraint(LAB_LTE, "2.282s"), {
    operator: "lte",
    value: 2.2825,
  });
  assert.deepEqual(computeConstraint(LAB_LTE, "0.3°"), { operator: "lte", value: 0.35 });
  assert.deepEqual(computeConstraint(LAB_GTE, "1.811m/s"), {
    operator: "gte",
    value: 1.8105,
  });
  assert.deepEqual(computeConstraint(LAB_GTE, "21.05s"), {
    operator: "gte",
    value: 21.045,
  });
});

test("computeConstraint treats basic rows as two-decimal displays", () => {
  assert.deepEqual(computeConstraint(BASIC_GTE, "70(+22)"), {
    operator: "gte",
    value: 69.995,
  });
  assert.deepEqual(computeConstraint(BASIC_LTE, "70(+22)"), {
    operator: "lte",
    value: 70.005,
  });
  assert.deepEqual(computeConstraint(BASIC_GTE, "44(-13)"), {
    operator: "gte",
    value: 43.995,
  });
  assert.deepEqual(computeConstraint(BASIC_GTE, "54(0)"), {
    operator: "gte",
    value: 53.995,
  });
});

test("computeConstraint rejects unusable input", () => {
  assert.equal(computeConstraint(LAB_LTE, ""), null);
  assert.equal(computeConstraint(LAB_LTE, "abc"), null);
  assert.equal(computeConstraint(LAB_LTE, null), null);
  assert.equal(computeConstraint({ id: "x", source: "other", operator: "lte" }, "1"), null);
  assert.equal(computeConstraint(null, "1"), null);
});

test("mapping tables cover the required metrics", () => {
  assert.equal(BASIC_SYNC_GROUPS.length, 4);
  assert.equal(LAB_SYNC_METRICS.length, 20);
  assert.deepEqual(
    BASIC_SYNC_GROUPS.map((definition) => definition.id),
    ["10005", "10006", "10007", "10008"],
  );
  assert.deepEqual(
    new Set(LAB_SYNC_METRICS.map((definition) => definition.id)),
    new Set(EXPECTED_LAB_IDS),
  );

  const definitions = [...BASIC_SYNC_GROUPS, ...LAB_SYNC_METRICS];
  assert.equal(new Set(definitions.map((definition) => definition.id)).size, 24);
  for (const definition of definitions) {
    assert.equal(typeof definition.label, "string");
    assert.equal(typeof definition.panelLabel, "string");
    assert.ok(definition.operator === "gte" || definition.operator === "lte");
    assert.ok(definition.source === "basic" || definition.source === "lab");
    assert.ok("groupTab" in definition);
  }

  const byId = new Map(definitions.map((definition) => [definition.id, definition]));
  assert.equal(byId.get("handling-aim-move-speed").operator, "gte");
  assert.equal(byId.get("stability-breath-duration").operator, "gte");
  assert.equal(byId.get("recoil-continuous-vertical").operator, "lte");
  assert.equal(byId.get("stability-hold-breath-recoil").label, "屏息后坐力相对裸枪修正");
  assert.equal(byId.get("stability-hold-breath-recoil").panelLabel, "屏息后坐力修正");

  const groupCounts = {};
  for (const definition of LAB_SYNC_METRICS) {
    groupCounts[definition.groupTab] = (groupCounts[definition.groupTab] ?? 0) + 1;
  }
  assert.deepEqual(groupCounts, { 后坐: 5, 操控: 7, 据枪: 6, 腰射: 2 });

  assert.equal(PRESET_IDS.length, 7);
  assert.equal(new Set(PRESET_IDS).size, 7);
  for (const id of PRESET_IDS) {
    assert.ok(byId.has(id), `preset id ${id} is missing from the mapping tables`);
  }
});

test("syncMetric writes a lab metric through the advanced panel", async () => {
  let waits = 0;
  const waitFrame = async () => {
    waits += 1;
  };

  const labStrong = fakeElement({ tagName: "STRONG", textContent: "0.57x" });
  const labLabel = fakeElement({ tagName: "SPAN", textContent: "全自动垂直后坐力" });
  const labRow = fakeElement({
    nodes: { ":scope > span": labLabel, ":scope > strong": labStrong },
  });
  const labList = fakeElement({ children: [labRow] });
  const labTabs = makeTabGroup(
    ["后坐力控制", "操控速度", "据枪稳定性", "腰际射击精度"],
    "腰际射击精度",
  );
  const groupButtons = makeTabGroup(["后坐", "操控", "据枪", "腰射"], "操控");

  const optimizer = makeOptimizerTabs("advanced");
  const priority = fakeElement({ tagName: "SELECT", value: "none" });
  const condition = fakeElement({ tagName: "SELECT", value: "gte" });
  const customValue = fakeElement({ tagName: "INPUT" });
  const advancedLabel = fakeElement({
    childNodes: [{ textContent: "全自动垂直后坐力" }],
  });
  const advancedRow = fakeElement({
    nodes: {
      ".gunsmith-smart-advanced-preference-label": advancedLabel,
      select: priority,
      'select[aria-label="全自动垂直后坐力比较条件"]': condition,
      'input[aria-label="全自动垂直后坐力自定义值"]': customValue,
    },
  });

  const doc = fakeDoc({
    ids: {
      "gunsmith-basic-attributes-tab": optimizer.basic,
      "gunsmith-advanced-attributes-tab": optimizer.advanced,
    },
    nodes: {
      ".gunsmith-lab-detail-list": labList,
      ".gunsmith-lab-tabs > button": labTabs,
      ".gunsmith-smart-advanced-groups button": groupButtons,
      ".gunsmith-smart-advanced-preference": [advancedRow],
    },
  });

  const result = await syncMetric(doc, "recoil-continuous-vertical", { waitFrame });

  assert.deepEqual(result.failed, []);
  assert.deepEqual(result.synced, [
    { id: "recoil-continuous-vertical", value: 0.575 },
  ]);
  assert.equal(priority.value, "custom");
  assert.equal(condition.value, "lte");
  assert.equal(customValue.value, "0.575");
  assert.deepEqual(
    priority.events.map((event) => event.type),
    ["input", "change"],
  );
  assert.equal(condition.events[0].bubbles, true);
  assert.equal(customValue.events[1].type, "change");
  assert.ok(waits >= 2);
  assert.equal(labTabs[0].clicks, 1);
  assert.equal(labTabs[3].clicks, 1);
  assert.ok(labTabs[3].classList.contains("is-active"));
  assert.equal(groupButtons[0].clicks, 1);
  assert.equal(groupButtons[1].clicks, 1);
  assert.ok(groupButtons[1].classList.contains("is-active"));
  assert.ok(optimizer.advanced.classList.contains("is-active"));
});

test("syncMetrics writes a basic metric and restores the original panel", async () => {
  const waitFrame = async () => {};
  const optimizer = makeOptimizerTabs("advanced");

  const statLabel = fakeElement({ tagName: "SPAN", textContent: "腰射精度" });
  const statValue = fakeElement({ tagName: "STRONG", textContent: "54(0)" });
  const statRow = fakeElement({
    nodes: {
      ".gunsmith-stat-copy > div > span": statLabel,
      ".gunsmith-stat-copy > div > strong": statValue,
    },
  });

  const condition = fakeElement({ tagName: "SELECT", value: "and-lte" });
  const input = fakeElement({ tagName: "INPUT" });
  const basicRow = fakeElement({
    nodes: {
      'select[aria-label="腰射精度条件"]': condition,
      'input[aria-label="腰射精度指定数值"]': input,
    },
  });

  const doc = fakeDoc({
    ids: {
      "gunsmith-basic-attributes-tab": optimizer.basic,
      "gunsmith-advanced-attributes-tab": optimizer.advanced,
    },
    nodes: {
      ".gunsmith-stat-row": [statRow],
      '.gunsmith-smart-preference[aria-label="腰射精度"]': basicRow,
    },
  });

  const result = await syncMetrics(doc, ["10008"], { waitFrame });

  assert.deepEqual(result.failed, []);
  assert.deepEqual(result.synced, [{ id: "10008", value: 53.995 }]);
  assert.equal(condition.value, "and-gte");
  assert.equal(input.value, "53.995");
  assert.equal(optimizer.basic.clicks, 1);
  assert.equal(optimizer.advanced.clicks, 1);
  assert.ok(optimizer.advanced.classList.contains("is-active"));
  assert.ok(!optimizer.basic.classList.contains("is-active"));
});

test("syncMetrics reports a missing row without throwing", async () => {
  const result = await syncMetrics(fakeDoc(), ["recoil-continuous-vertical"], {
    waitFrame: async () => {},
  });
  assert.deepEqual(result.synced, []);
  assert.equal(result.failed.length, 1);
  assert.equal(result.failed[0].id, "recoil-continuous-vertical");
});

test("syncMetrics refuses a disabled advanced row", async () => {
  const labStrong = fakeElement({ tagName: "STRONG", textContent: "0.57x" });
  const labRow = fakeElement({
    nodes: {
      ":scope > span": fakeElement({ tagName: "SPAN", textContent: "全自动垂直后坐力" }),
      ":scope > strong": labStrong,
    },
  });
  const priority = fakeElement({ tagName: "SELECT", value: "none", disabled: true });
  const advancedRow = fakeElement({
    nodes: {
      ".gunsmith-smart-advanced-preference-label": fakeElement({
        childNodes: [{ textContent: "全自动垂直后坐力" }],
      }),
      select: priority,
    },
  });
  const doc = fakeDoc({
    nodes: {
      ".gunsmith-lab-detail-list": fakeElement({ children: [labRow] }),
      ".gunsmith-lab-tabs > button": makeTabGroup(["后坐力控制"], "后坐力控制"),
      ".gunsmith-smart-advanced-groups button": makeTabGroup(["后坐"], "后坐"),
      ".gunsmith-smart-advanced-preference": [advancedRow],
    },
  });

  const result = await syncMetric(doc, "recoil-continuous-vertical", {
    waitFrame: async () => {},
  });

  assert.deepEqual(result.synced, []);
  assert.deepEqual(result.failed, [
    { id: "recoil-continuous-vertical", reason: "不可计算" },
  ]);
  assert.equal(priority.value, "none");
});

test("syncMetrics rejects unknown ids and deduplicates", async () => {
  const result = await syncMetrics(fakeDoc(), ["unknown", "unknown"], {
    waitFrame: async () => {},
  });
  assert.deepEqual(result.synced, []);
  assert.deepEqual(result.failed, [{ id: "unknown", reason: "未知指标" }]);
});
