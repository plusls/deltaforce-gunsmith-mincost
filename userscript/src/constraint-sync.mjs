const LEADING_NUMBER_PATTERN = /^(-?\d+)(?:\.(\d+))?/;
const BASIC_DISPLAY_DECIMALS = 2;

export const BASIC_SYNC_GROUPS = [
  {
    id: "10005",
    source: "basic",
    label: "后坐力控制",
    panelLabel: "后坐控制",
    groupTab: null,
    operator: "gte",
  },
  {
    id: "10006",
    source: "basic",
    label: "操控速度",
    panelLabel: "操控速度",
    groupTab: null,
    operator: "gte",
  },
  {
    id: "10007",
    source: "basic",
    label: "武器稳定性",
    panelLabel: "据枪稳定",
    groupTab: null,
    operator: "gte",
  },
  {
    id: "10008",
    source: "basic",
    label: "腰射精度",
    panelLabel: "腰射精度",
    groupTab: null,
    operator: "gte",
  },
];

export const LAB_SYNC_METRICS = [
  {
    id: "recoil-continuous-vertical",
    source: "lab",
    label: "全自动垂直后坐力",
    panelLabel: "全自动垂直后坐力",
    labTab: "后坐力控制",
    groupTab: "后坐",
    operator: "lte",
  },
  {
    id: "recoil-continuous-horizontal",
    source: "lab",
    label: "全自动水平后坐力",
    panelLabel: "全自动水平后坐力",
    labTab: "后坐力控制",
    groupTab: "后坐",
    operator: "lte",
  },
  {
    id: "recoil-single-vertical",
    source: "lab",
    label: "单点/多连发垂直后坐力",
    panelLabel: "单点/多连发垂直后坐力",
    labTab: "后坐力控制",
    groupTab: "后坐",
    operator: "lte",
  },
  {
    id: "recoil-single-horizontal",
    source: "lab",
    label: "单点/多连发水平后坐力",
    panelLabel: "单点/多连发水平后坐力",
    labTab: "后坐力控制",
    groupTab: "后坐",
    operator: "lte",
  },
  {
    id: "recoil-fire-stability",
    source: "lab",
    label: "开火时武器稳定性",
    panelLabel: "开火时武器稳定性",
    labTab: "后坐力控制",
    groupTab: "后坐",
    operator: "lte",
  },
  {
    id: "handling-aim-time",
    source: "lab",
    label: "举镜时间",
    panelLabel: "举镜时间",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "lte",
  },
  {
    id: "handling-sprint-fire-delay",
    source: "lab",
    label: "跑射延迟",
    panelLabel: "跑射延迟",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "lte",
  },
  {
    id: "handling-aim-move-speed",
    source: "lab",
    label: "举镜移动速度",
    panelLabel: "举镜移动速度",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "gte",
  },
  {
    id: "handling-reload-time",
    source: "lab",
    label: "换弹时间",
    panelLabel: "换弹时间",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "lte",
  },
  {
    id: "handling-empty-reload-time",
    source: "lab",
    label: "空仓换弹时间",
    panelLabel: "空仓换弹时间",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "lte",
  },
  {
    id: "handling-holster-time",
    source: "lab",
    label: "收枪时间",
    panelLabel: "收枪时间",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "lte",
  },
  {
    id: "handling-switch-time",
    source: "lab",
    label: "切枪时间",
    panelLabel: "切枪时间",
    labTab: "操控速度",
    groupTab: "操控",
    operator: "lte",
  },
  {
    id: "stability-breath-range",
    source: "lab",
    label: "呼吸晃动范围",
    panelLabel: "呼吸晃动范围",
    labTab: "据枪稳定性",
    groupTab: "据枪",
    operator: "lte",
  },
  {
    id: "stability-breath-duration",
    source: "lab",
    label: "屏息时长",
    panelLabel: "屏息时长",
    labTab: "据枪稳定性",
    groupTab: "据枪",
    operator: "gte",
  },
  {
    id: "stability-hit-sway",
    source: "lab",
    label: "受击上抬倍率",
    panelLabel: "受击上抬倍率",
    labTab: "据枪稳定性",
    groupTab: "据枪",
    operator: "lte",
  },
  {
    id: "stability-movement-camera",
    source: "lab",
    label: "移动镜头稳定性",
    panelLabel: "移动镜头稳定性",
    labTab: "据枪稳定性",
    groupTab: "据枪",
    operator: "lte",
  },
  {
    id: "stability-fire-camera",
    source: "lab",
    label: "开火时镜头稳定性",
    panelLabel: "开火时镜头稳定性",
    labTab: "据枪稳定性",
    groupTab: "据枪",
    operator: "lte",
  },
  {
    id: "stability-hold-breath-recoil",
    source: "lab",
    label: "屏息后坐力相对裸枪修正",
    panelLabel: "屏息后坐力修正",
    labTab: "据枪稳定性",
    groupTab: "据枪",
    operator: "lte",
  },
  {
    id: "hipfire-standing-spread",
    source: "lab",
    label: "站姿腰射散布范围",
    panelLabel: "站姿腰射散布范围",
    labTab: "腰际射击精度",
    groupTab: "腰射",
    operator: "lte",
  },
  {
    id: "hipfire-maximum-spread",
    source: "lab",
    label: "站姿最大开火散布范围",
    panelLabel: "站姿最大开火散布范围",
    labTab: "腰际射击精度",
    groupTab: "腰射",
    operator: "lte",
  },
];

export const PRESET_IDS = [
  "recoil-continuous-vertical",
  "recoil-continuous-horizontal",
  "recoil-fire-stability",
  "handling-aim-time",
  "handling-sprint-fire-delay",
  "stability-fire-camera",
  "10008",
];

const DEFINITIONS = new Map(
  [...BASIC_SYNC_GROUPS, ...LAB_SYNC_METRICS].map((definition) => [
    definition.id,
    definition,
  ]),
);

export function parseLeadingNumber(text) {
  if (text == null) return null;
  const match = String(text).trim().match(LEADING_NUMBER_PATTERN);
  return match ? Number(match[0]) : null;
}

export function computeConstraint(definition, displayedText) {
  if (!definition || (definition.source !== "basic" && definition.source !== "lab")) {
    return null;
  }
  const match = String(displayedText ?? "").trim().match(LEADING_NUMBER_PATTERN);
  if (!match) return null;
  const displayed = Number(match[0]);
  if (!Number.isFinite(displayed)) return null;

  const operator = definition.operator === "lte" ? "lte" : "gte";
  // Basic rows always render two decimals; lab rows use the displayed precision.
  const decimals =
    definition.source === "basic" ? BASIC_DISPLAY_DECIMALS : (match[2]?.length ?? 0);
  const epsilon = 0.5 * 10 ** -decimals;
  const bound = operator === "lte" ? displayed + epsilon : displayed - epsilon;
  return {
    operator,
    value: Number(bound.toFixed(decimals + 1)),
  };
}

export async function syncMetric(doc, id, options = {}) {
  return syncMetrics(doc, [id], options);
}

export async function syncMetrics(doc, ids, options = {}) {
  const waitFrame = resolveWaitFrame(doc, options);
  const synced = [];
  const failed = [];
  const definitions = [];
  const seen = new Set();

  for (const rawId of Array.from(ids ?? [])) {
    const id = String(rawId);
    if (seen.has(id)) continue;
    seen.add(id);
    const definition = DEFINITIONS.get(id);
    if (!definition) {
      failed.push({ id, reason: "未知指标" });
      continue;
    }
    definitions.push(definition);
  }

  if (definitions.length === 0) return { synced, failed };

  const original = capturePanelState(doc);
  try {
    const pending = [];
    await readAll(doc, definitions, waitFrame, pending, failed);
    await writeAll(doc, pending, waitFrame, synced, failed);
  } finally {
    // Restoring the panel is cosmetic; never let it change the sync result.
    await restorePanelState(doc, original, waitFrame);
  }

  return { synced, failed };
}

async function readAll(doc, definitions, waitFrame, pending, failed) {
  const labGroups = new Map();
  for (const definition of definitions) {
    if (definition.source === "basic") {
      collectReading(definition, readBasicText(doc, definition.label), pending, failed);
      continue;
    }
    const group = labGroups.get(definition.labTab) ?? [];
    group.push(definition);
    labGroups.set(definition.labTab, group);
  }

  for (const [labTab, group] of labGroups) {
    await activateLabTab(doc, labTab, waitFrame);
    for (const definition of group) {
      collectReading(definition, readLabText(doc, definition.label), pending, failed);
    }
  }
}

function collectReading(definition, text, pending, failed) {
  const constraint = computeConstraint(definition, text);
  if (!constraint) {
    failed.push({ id: definition.id, reason: "未找到显示值" });
    return;
  }
  pending.push({ definition, constraint });
}

async function writeAll(doc, pending, waitFrame, synced, failed) {
  const basics = pending.filter((entry) => entry.definition.source === "basic");
  if (basics.length > 0) {
    await activateOptimizerTab(doc, "basic", waitFrame);
    for (const entry of basics) writeBasicEntry(doc, entry, synced, failed);
  }

  const advancedGroups = new Map();
  for (const entry of pending) {
    if (entry.definition.source !== "lab") continue;
    const group = advancedGroups.get(entry.definition.groupTab) ?? [];
    group.push(entry);
    advancedGroups.set(entry.definition.groupTab, group);
  }

  for (const [groupTab, entries] of advancedGroups) {
    await openAdvancedGroup(doc, groupTab, waitFrame);
    for (const entry of entries) {
      await writeAdvancedEntry(doc, entry, waitFrame, synced, failed);
    }
  }
}

function writeBasicEntry(doc, entry, synced, failed) {
  const { definition, constraint } = entry;
  const row = doc.querySelector(
    `.gunsmith-smart-preference[aria-label="${definition.panelLabel}"]`,
  );
  if (!row) {
    failed.push({ id: definition.id, reason: "未找到基础属性行" });
    return;
  }
  const condition = row.querySelector(
    `select[aria-label="${definition.panelLabel}条件"]`,
  );
  const input = row.querySelector(
    `input[aria-label="${definition.panelLabel}指定数值"]`,
  );
  if (!condition || !input) {
    failed.push({ id: definition.id, reason: "未找到基础属性控件" });
    return;
  }
  setNativeValue(doc, condition, `and-${constraint.operator}`);
  setNativeValue(doc, input, String(constraint.value));
  synced.push({ id: definition.id, value: constraint.value });
}

async function writeAdvancedEntry(doc, entry, waitFrame, synced, failed) {
  const { definition, constraint } = entry;
  let row = findAdvancedRow(doc, definition.panelLabel);
  if (!row) {
    failed.push({ id: definition.id, reason: "未找到高级属性行" });
    return;
  }

  const priority = row.querySelector("select");
  if (!priority) {
    failed.push({ id: definition.id, reason: "未找到高级属性控件" });
    return;
  }
  if (priority.disabled === true || priority.getAttribute?.("aria-disabled") === "true") {
    failed.push({ id: definition.id, reason: "不可计算" });
    return;
  }

  if (priority.value !== "custom") {
    // The condition select and value input only mount after priority becomes custom.
    setNativeValue(doc, priority, "custom");
    await waitFrame();
    row = findAdvancedRow(doc, definition.panelLabel);
  }

  const condition = row?.querySelector(
    `select[aria-label="${definition.panelLabel}比较条件"]`,
  );
  const input = row?.querySelector(
    `input[aria-label="${definition.panelLabel}自定义值"]`,
  );
  if (!condition || !input) {
    failed.push({ id: definition.id, reason: "未找到高级属性控件" });
    return;
  }
  setNativeValue(doc, condition, constraint.operator);
  setNativeValue(doc, input, String(constraint.value));
  synced.push({ id: definition.id, value: constraint.value });
}

function readLabText(doc, label) {
  const list = doc.querySelector(".gunsmith-lab-detail-list");
  if (!list) return null;
  for (const row of [...list.children]) {
    if (textOf(row.querySelector(":scope > span")) !== label) continue;
    const strong =
      row.querySelector(":scope > strong") ||
      row.querySelector(".gunsmith-lab-dual-value > strong");
    return strong ? strong.textContent : null;
  }
  return null;
}

function readBasicText(doc, label) {
  for (const row of doc.querySelectorAll(".gunsmith-stat-row")) {
    if (textOf(row.querySelector(".gunsmith-stat-copy > div > span")) !== label) continue;
    const strong = row.querySelector(".gunsmith-stat-copy > div > strong");
    return strong ? strong.textContent : null;
  }
  return null;
}

function findAdvancedRow(doc, label) {
  for (const row of doc.querySelectorAll(".gunsmith-smart-advanced-preference")) {
    const labelEl = row.querySelector(".gunsmith-smart-advanced-preference-label");
    if (textOf(labelEl?.childNodes?.[0]) === label) return row;
  }
  return null;
}

function findButtonByText(doc, selector, text) {
  for (const button of doc.querySelectorAll(selector)) {
    if (textOf(button) === text) return button;
  }
  return null;
}

function textOf(node) {
  return node && typeof node.textContent === "string" ? node.textContent.trim() : "";
}

function isActive(element) {
  if (!element) return false;
  if (element.getAttribute?.("aria-selected") === "true") return true;
  if (element.classList?.contains?.("is-active")) return true;
  return typeof element.className === "string"
    ? element.className.split(/\s+/).includes("is-active")
    : false;
}

async function activateOptimizerTab(doc, tab, waitFrame) {
  const button = doc.getElementById(
    tab === "basic" ? "gunsmith-basic-attributes-tab" : "gunsmith-advanced-attributes-tab",
  );
  if (!button || isActive(button)) return;
  button.click();
  await waitFrame();
}

async function activateLabTab(doc, labTab, waitFrame) {
  const button = findButtonByText(doc, ".gunsmith-lab-tabs > button", labTab);
  if (!button || isActive(button)) return;
  button.click();
  await waitFrame();
}

async function openAdvancedGroup(doc, groupTab, waitFrame) {
  await activateOptimizerTab(doc, "advanced", waitFrame);
  const button = findButtonByText(doc, ".gunsmith-smart-advanced-groups button", groupTab);
  if (!button || isActive(button)) return;
  button.click();
  await waitFrame();
}

function capturePanelState(doc) {
  const tab = activeOptimizerTab(doc);
  return {
    tab,
    group: tab === "advanced" ? activeAdvancedGroup(doc) : null,
    labTab: activeLabTab(doc),
  };
}

function activeLabTab(doc) {
  for (const button of doc.querySelectorAll(".gunsmith-lab-tabs > button")) {
    if (isActive(button)) return textOf(button);
  }
  return null;
}

function activeOptimizerTab(doc) {
  const basic = doc.getElementById("gunsmith-basic-attributes-tab");
  const advanced = doc.getElementById("gunsmith-advanced-attributes-tab");
  const basicActive = isActive(basic);
  const advancedActive = isActive(advanced);
  if (basicActive && !advancedActive) return "basic";
  if (advancedActive && !basicActive) return "advanced";
  if (doc.querySelector(".gunsmith-smart-advanced-preference")) return "advanced";
  if (doc.querySelector(".gunsmith-smart-preference")) return "basic";
  return null;
}

function activeAdvancedGroup(doc) {
  for (const button of doc.querySelectorAll(".gunsmith-smart-advanced-groups button")) {
    if (isActive(button)) return textOf(button);
  }
  return null;
}

async function restorePanelState(doc, original, waitFrame) {
  if (!original) return;
  try {
    if (original.tab) {
      await activateOptimizerTab(doc, original.tab, waitFrame);
      if (original.tab === "advanced" && original.group) {
        const button = findButtonByText(
          doc,
          ".gunsmith-smart-advanced-groups button",
          original.group,
        );
        if (button && !isActive(button)) {
          button.click();
          await waitFrame();
        }
      }
    }
    if (original.labTab) {
      const button = findButtonByText(
        doc,
        ".gunsmith-lab-tabs > button",
        original.labTab,
      );
      if (button && !isActive(button)) {
        button.click();
        await waitFrame();
      }
    }
  } catch {
    /* ignore */
  }
}

function resolveWaitFrame(doc, options) {
  if (typeof options.waitFrame === "function") return () => options.waitFrame();
  return async () => {
    const view = doc.defaultView;
    const raf =
      view && typeof view.requestAnimationFrame === "function"
        ? view.requestAnimationFrame.bind(view)
        : null;
    if (!raf) return;
    await new Promise((resolve) => raf(() => raf(resolve)));
  };
}

function setNativeValue(doc, element, value) {
  const view = doc.defaultView || globalThis;
  const constructorName = element.tagName === "SELECT" ? "HTMLSelectElement" : "HTMLInputElement";
  const constructor = view[constructorName] || globalThis[constructorName];
  const setter =
    constructor && Object.getOwnPropertyDescriptor(constructor.prototype, "value")?.set;
  const text = String(value);
  if (setter) setter.call(element, text);
  else element.value = text;

  const EventConstructor =
    typeof view.Event === "function" ? view.Event : typeof Event === "function" ? Event : null;
  for (const type of ["input", "change"]) {
    const event = EventConstructor
      ? new EventConstructor(type, { bubbles: true })
      : { type, bubbles: true };
    element.dispatchEvent(event);
  }
}
