# moligod 智能配件方案生成 逆向笔记

记录对象: https://moligod.com/gunsmith/ 左侧 "智能配装" 面板
数据版本: 7e60debf7b978ebec618a142
分析时间: 2026-10-21
相关文档: [gunsmith-numerical-model.md](./gunsmith-numerical-model.md), [gunsmith-rust-solver.md](./gunsmith-rust-solver.md)

本文只记录 "生成智能配件方案" 这一功能。数值公式、装配树、精校见另一篇。

## 0. TL;DR

- 没有服务端求解 API。点击按钮后浏览器内启动一个 module Web Worker:
  `/assets/optimizer.worker-5V9qh1RK.js`
- 主线程通过 `postMessage` 发送完整求解请求, worker 回一个 `{requestId, purpose, result}`。
- 求解器是带时间预算的启发式局部搜索(beam + 剪枝 + 采样), 不是穷举, 也不保证全局最优。
- UI 只发 `purpose: "solve"`。
- 想接管实现, 最稳的接口是 Worker 层: 拦下发往 `optimizer.worker-*.js` 的请求, 自己回同构的 result。
- 求解期间不发生任何网络请求。配件数据、价格在进入求解器前已经准备好。

## 1. 入口与流程

按钮位置: 左栏 "智能配装" 面板底部, 蓝色 `生成智能配件方案`。
按钮 DOM: `.gunsmith-optimizer-generate`。

面板控件:

| 控件 | 含义 | 对应请求字段 |
|---|---|---|
| 高级属性 / 基础属性 | 两组偏好设置 | `smartOptions` |
| 优先级 低/中/高/极高/最高 | 软偏好权重 | `smartOptions.preferences` 或 `advancedPreferences` |
| 条件 大于等于/小于等于 + 数值 | 硬阈值 | `smartOptions.preferenceConditions` 或 `advancedPreferenceTargets` |
| 搜索 [N] 秒 | 求解时间预算 | `timeBudgetMs`, `maxEvaluations` |
| 精调 | 是否优化精校 | `smartOptions.fineTuningEnabled` |
| 卡战备 | 达标后优先低市场价 | `targets.preparednessCostOptimized` |
| 最高预算 / 最低战备 | 价格约束 | `targets.budgetPrice` / `targets.preparednessValue` |
| 禁用配件 | 排除候选 | `excludedPartIds` |

流程:

```text
React 组件
  -> 组装 solverInput (当前配装 / 锁定 / 价格 / 偏好 / 目标)
  -> yu(request)
       Worker 懒创建
       requestId = 全局自增
       dataKey 未变则省略 data
       postMessage(request)
  -> worker.onmessage
       校验 requestId + purpose
       写入结果 state
  -> 渲染 "已生成智能配件方案" 卡片
       variant.candidate.selections + tuningState
       点 "应用方案" 调用 onApplySmartRecommendation
```

Worker 生命周期:

- 第一次生成时创建, 之后复用。
- 取消或组件重建时 `terminate()`, 下次生成重新创建, 并重新发送 `data`。
- worker 内部按 `dataKey` 缓存枪械数据。请求不带 `data` 时用缓存。

## 2. 数据来源与"API"

智能配装本身没有 HTTP API。它复用的是普通改枪页面已经拿到的数据:

```text
/api/gunsmith/manifest
/api/gunsmith/versions/{dataVersion}/weapons/{weaponId}
/api/gunsmith/weapons/{weaponId}/prices?version=...
/api/gunsmith/weapons/prices?version=...
```

价格在进入 worker 前被整理成:

```json
{
  "<officialObjectId>": {
    "currentPrice": 25012,
    "systemPrice": 12345
  }
}
```

`currentPrice` 来自市场价, `systemPrice` 来自系统价(战备值)。
`blackhawk` 模式下市场价改用 `blackhawkPrices`, 并受 `blackhawkModifyLimit` 限制。

面板输入会持久化到 localStorage, key:

```text
moligod:gunsmith-optimizer-inputs:v1:{weaponId}:{mode}:ordinary
moligod:gunsmith-optimizer-inputs:v1:{weaponId}:{mode}:preparedness
```

## 3. 主线程 -> Worker 请求

请求是结构化克隆, 完整形态:

```json
{
  "requestId": 3,
  "purpose": "solve",
  "dataKey": "7e60debf7b978ebec618a142:18010000049",
  "data": { "...": "weapon payload" },
  "weaponId": "18010000049",
  "mode": "operations",
  "laserEnabled": false,
  "currentSelections": { "": "18010000049", "...": "..." },
  "tuningState": { "...": 50 },
  "lockedPathKeys": ["..."],
  "lockedTuningKeys": ["..."],
  "excludedPartIds": ["..."],
  "pricesByObjectId": { "...": { "currentPrice": 0, "systemPrice": 0 } },
  "targets": { "...": "..." },
  "smartOptions": { "...": "..." },
  "maxEvaluations": 900000,
  "timeBudgetMs": 60000
}
```

字段:

| 字段 | 类型 | 说明 |
|---|---|---|
| `requestId` | number | 每次生成自增, 响应必须原样带回 |
| `purpose` | string | 当前 UI 固定 `"solve"` |
| `dataKey` | string | 数据版本 + 武器 id 的缓存键 |
| `data` | object? | 完整 weapon payload; 仅当 `dataKey` 变化时发送 |
| `weaponId` | string | 武器 id |
| `mode` | string | `operations` / `warfare` / `blackhawk` |
| `laserEnabled` | boolean | 激光是否开启, 影响镭射词条; 缺省 false |
| `currentSelections` | object | `slotPathKey -> partId` |
| `tuningState` | object | `slotPathKey\0partId\0tuneId -> number` |
| `lockedPathKeys` | string[] | 用户锁定的槽位 |
| `lockedTuningKeys` | string[] | 用户锁定的精校 |
| `excludedPartIds` | string[] | 禁用配件 + UI 排除件 |
| `pricesByObjectId` | object | officialObjectId -> current/system price |
| `targets` | object | 硬约束与优先级 |
| `smartOptions` | object? | 偏好; 有它才产生智能推荐 |
| `maxEvaluations` | number | 状态评估次数上限 |
| `timeBudgetMs` | number | 墙钟预算, 毫秒 |

缓存语义:

- `dataKey` 没变时 `data` 为 `undefined`。
- worker 没有对应缓存会抛 `"枪械数据缓存未初始化，请重试"`。
- 自己实现 hook 时必须保持这个缓存行为, 或者忽略请求里的 `data` 缺失情况。

`dataKey` 生成方式:

```js
`${data.dataVersion || `${data.version}:${data.generatedAt}`}:${weaponId}${data.activeMode ? `:${data.activeMode}` : ""}`
```

参数换算:

```text
timeBudgetMs   = searchSeconds * 1000
maxEvaluations = searchSeconds * 15000
searchSeconds  = clamp(输入值, 2, 60)
勾选精调时     = clamp(输入值, 5, 60)
```

## 4. targets

结构:

```json
{
  "attributeTargets": {
    "10001": 55,
    "10005": 48,
    "10006": 35,
    "10007": 71,
    "10008": 24
  },
  "enabledAttributeIds": [],
  "budgetEnabled": false,
  "budgetPrice": 0,
  "preparednessEnabled": false,
  "preparednessValue": 0,
  "preparednessCostOptimized": false,
  "priority": "attributes"
}
```

- `attributeTargets` 默认取当前面板值, 并夹到 `[0, max(provisionalMax, 当前值)]`。
- `enabledAttributeIds` 由 UI 初始化为空, 且没有入口修改。因此求解器的 `Yt()` 硬约束实际上只剩预算和战备。
- `priority` 取值 `attributes` / `preparedness` / `budget`。
- `budgetEnabled` 为 false 时预算不参与硬约束。
- 战备开启时 `preparednessCostOptimized` 对应 "卡战备": 达标后优先更低市场总价。

`blackhawk` 模式下 worker 会先改写 targets:

```text
preparednessEnabled = false
preparednessCostOptimized = false
budgetEnabled = true
budgetPrice = min(原预算, blackhawkPrice[weaponId] + blackhawkModifyLimit)
```

## 5. smartOptions

结构:

```json
{
  "preferences": {
    "recoil": "none",
    "handling": "none",
    "stability": "none",
    "hipfire": "none"
  },
  "preferenceConditions": {
    "recoil": { "join": "and", "operator": "gte", "value": null },
    "handling": { "join": "and", "operator": "gte", "value": null },
    "stability": { "join": "and", "operator": "gte", "value": null },
    "hipfire": { "join": "and", "operator": "gte", "value": null }
  },
  "advancedPreferences": {
    "recoil-continuous-vertical": "custom",
    "...": "none"
  },
  "advancedPreferenceTargets": {
    "recoil-continuous-vertical": { "operator": "lte", "value": 0.46 }
  },
  "fineTuningEnabled": true
}
```

基础偏好的组到主属性:

```text
recoil    -> 10005 后坐力控制
handling  -> 10006 操控速度
stability -> 10007 武器稳定性
hipfire   -> 10008 腰射精度
```

等级与软权重:

| 等级 | 权重 |
|---|---:|
| low | 1 |
| medium | 20 |
| high | 50 |
| extreme | 70 |
| highest | 100 |

- 基础偏好的等级只用于排序打分, 不是硬约束。
- 基础偏好里的 `指定数值` 会写入 `preferenceConditions`, 作为该组的硬阈值。
- 高级偏好里的 `custom` 会写入 `advancedPreferenceTargets`, 作为该细分指标的硬阈值。
- 高级偏好里的等级同样只用于打分。

20 个细分指标 id:

```text
recoil-continuous-vertical
recoil-continuous-horizontal
recoil-single-vertical
recoil-single-horizontal
recoil-fire-stability
handling-aim-time
handling-sprint-fire-delay
handling-aim-move-speed
handling-reload-time
handling-empty-reload-time
handling-holster-time
handling-switch-time
stability-breath-range
stability-breath-duration
stability-movement-camera
stability-fire-camera
stability-hit-sway
stability-hold-breath-recoil
hipfire-standing-spread
hipfire-maximum-spread
```

开启精调后, 基础偏好会自动展开成该组每个细分指标的软偏好, 已单独设置过的不覆盖。

## 6. Worker 内部算法

### 6.1 求解状态

一个候选状态包含:

```text
selections            槽位 -> 配件 id
tuningState           精校值
assembly              解析后的可见槽位 + 内部虚拟件
installedParts        已装件
metrics.attributes    10001/10005/10006/10007/10008
metrics.detailMetrics 20 个细分指标
currentPrice          市场总价
systemPrice           系统总价(战备)
missing*PriceCount    缺价件数
installedPartCount    已装件数
```

智能模式下起点不是当前配装:

```text
无锁定       -> 空配装
有锁定槽位   -> 只保留锁定件, 其余全部移除
未锁定精校   -> 重置为默认值
```

这一点容易被误认为 "生成方案会保留我当前装的东西", 实际不会。
UI 展示时用 `changedPartCount` 与真实当前配装比较。

### 6.2 装配解析与指标

- 用 `slotsByHost` / `virtualRules` / 默认预设解析装配树。
- 每换一个件, 逐层重算可见槽位, 处理互斥占用 `extraOccupySocketIds`。
- 面板指标沿用普通改枪的 display Addend 聚合。
- 细分指标由完整数值模型计算。
- 不满足锁定条件或包含禁用件的状态直接丢弃。

### 6.3 候选剪枝

每个槽位不是遍历所有候选。默认快路径:

```text
候选数 <= 32 -> 全部评估
候选数 >  32 -> 只保留一个子集
```

子集包含:

- 当前已选件
- utility 最高的 4 个
- 每个偏好指标的最高值和最低值件
- 市场价最低件
- 系统价最高件
- 能开出子槽位的件(branch parts)
- 有精调潜力时, 每个细分偏好再取至多 4 个投影最优件

utility 是 `Ge()` 的加权分, 与最终推荐排序不是同一套分数。

### 6.4 Beam 搜索

```text
初始状态 = 只保留锁定件的配装 (无锁定则为空)
每轮:
  从候选状态里挑槽位
  对每个槽位取剪枝后的候选
  逐个应用配件, 生成新状态
  用排序函数保留至多 128 个状态
  最多 24 轮
  时间或评估次数耗尽则停止
```

每轮优先探索的槽位:

- utility 最高的槽位
- 每个偏好指标的最优槽位
- 按 hash 采样的槽位
- 剩余槽位按名次补齐

状态排序 `$t()` 在默认 `enabledAttributeIds` 为空时近似为:

```text
[缺价数, 超预算/缺战备, 缺系统价, 缺市场价, 超预算,
 -属性总分, -属性总分, -系统价, -市场价, 已装件数]
```

数组按字典序比较。注意属性总分出现了两次, 相当于权重加倍。这可能是实现遗留, 也可能是有意加权。

### 6.5 精调

只有勾选 "精调" 才优化精校。

```text
关闭: 所有未锁定精校保持默认值
开启: 对状态转换生成精校变体
      每条转换最多 3 或 5 个变体, 取值一般是 默认 / 下界 / 上界
      最后对最多 12 或 24 个候选做坐标上升
      每轮只尝试下界和上界, 直到不再改进或预算耗尽
```

精调有时间片预算, 超出后返回当前最好结果, 并把 `truncated` 标记为 true。

### 6.6 推荐与可行性

推荐阶段不复用 beam 的排序, 而是:

1. 对所有已评估状态计算 `zt()` 失败列表。
2. 失败列表为空的状态视为可行。
3. 可行状态里用偏好分数选最优。
4. 一个可行状态都没有时, 返回 `kind: "infeasible"`, 附带失败原因。

`zt()` 检查的硬条件:

```text
预算上限
战备下限
preferenceConditions (基础偏好里的指定阈值)
advancedPreferenceTargets (高级偏好里的 custom 阈值)
细分指标不可计算
```

`zt()` 不检查 `targets.enabledAttributeIds`, 因为 UI 从不启用它。

偏好分数:

```text
对每个偏好指标:
  Gt(value) = direction == higher ? value : -value
  有 target 时按 gte/lte 截断
  best = 所有候选里的最大 Gt
  score = (current - value) / max(0, best - current)
```

也就是 "拿下了多少比例的可提升空间"。候选比当前差时, 比值为负。
按等级从高到低比较:

```text
highest -> extreme -> high -> medium -> low
每个等级先比 detailed, 再比 primary
每个类型先比最弱指标(weakest), 再比总和(total)
```

所以单个指标拖后腿会明显影响排序。

### 6.7 多方案 variants

只有同时满足以下条件才生成多方案:

```text
至少两个基础偏好被设为 "最高"
```

最多 3 个:

- 一个平衡方案
- 每个 "最高" 属性各一个侧重方案

每个 variant 带 `focusAttributeId`、改件数、改精校数。

### 6.8 预算与常量

```text
时间预算      timeBudgetMs, 其中约 4% 留给收尾
评估上限      maxEvaluations, 其中约 3% 留给收尾
单轮状态上限  128
最大轮数      24
候选数快路径  32
精调采样上限  4096
精调候选上限  12 / 24
优先级方案数  4
变体上限      3
```

worker 的默认预算(客户端没传时):

```text
solve: maxEvaluations 30000, timeBudgetMs 1250
其它:  maxEvaluations 20000, timeBudgetMs 2000
```

客户端实际传入的是 `秒数 * 15000` 和 `秒数 * 1000`, 所以墙钟预算通常先耗尽。

## 7. Worker -> 主线程响应

成功:

```json
{
  "requestId": 3,
  "purpose": "solve",
  "result": {
    "selections": { "slotPathKey": "partId" },
    "metrics": {
      "attributes": {
        "10001": 72,
        "10005": 71,
        "10006": 35,
        "10007": 71,
        "10008": 24
      },
      "detailMetrics": { "recoil-continuous-vertical": 0.46 },
      "currentPrice": 123456,
      "systemPrice": 98765,
      "missingCurrentPriceCount": 0,
      "missingSystemPriceCount": 0,
      "installedPartCount": 8
    },
    "ranges": {
      "attributes": {
        "10005": { "min": 48, "max": 90, "recommended": 71, "available": true }
      },
      "budgetPrice": { "min": 0, "max": 0, "recommended": 0, "available": false },
      "preparednessValue": { "min": 0, "max": 0, "recommended": 0, "available": false }
    },
    "rangeWitnesses": {
      "attributeMaxSelections": {},
      "budgetMinSelections": null,
      "preparednessMaxSelections": null
    },
    "unmetItems": [],
    "allTargetsSatisfied": true,
    "elapsedMs": 1234.5,
    "evaluatedStates": 12345,
    "generatedStates": 67890,
    "truncated": false,
    "smartRecommendation": {
      "kind": "recommendation",
      "currentMetrics": { "attributes": {}, "detailMetrics": {} },
      "candidate": {
        "selections": {},
        "tuningState": {},
        "metrics": {},
        "installedPartKeys": [],
        "detailMetrics": {}
      },
      "changedPartCount": 3,
      "changedTuningCount": 0,
      "score": { "byAttributeId": {}, "tiers": {} },
      "variants": [],
      "failures": []
    },
    "candidates": [
      { "priority": "10005", "selections": {}, "metrics": {} }
    ]
  }
}
```

失败:

```json
{ "requestId": 3, "purpose": "solve", "error": "错误信息" }
```

`smartRecommendation` 两种形态:

```json
{
  "kind": "recommendation",
  "currentMetrics": {},
  "candidate": {},
  "changedPartCount": 3,
  "changedTuningCount": 2,
  "score": {},
  "variants": [
    {
      "candidate": {},
      "focusAttributeId": "10005",
      "changedPartCount": 4,
      "changedTuningCount": 0
    }
  ],
  "failures": []
}
```

```json
{
  "kind": "infeasible",
  "currentMetrics": {},
  "candidate": {},
  "failures": [
    {
      "kind": "budget",
      "requiredValue": 150000
    },
    {
      "kind": "preference-target",
      "attributeId": "10005",
      "requiredValue": 62,
      "targetValue": 70,
      "operator": "lte"
    }
  ]
}
```

失败原因 `kind`:

```text
budget
preparedness
preference-target
detail-unavailable
```

## 8. UI 如何消费结果

- 只认 `requestId` 和 `purpose` 都匹配的响应。
- 结果按 `contextKey` 缓存, 输入一变即失效。
- `smartRecommendation.kind === "recommendation"` 时显示方案卡片。
- 卡片里的五项属性直接取 `candidate.metrics.attributes`。
- `variants` 为空时把主 candidate 包成一条展示。
- 点 "应用方案" 时同时应用 `selections` 和 `tuningState`。
- 应用后重新计算锁定状态:
  - 被方案移除的锁定件会自动解除
  - 被方案保留的锁定精校会保留
- `truncated` 为 true 时显示 "本次搜索范围内最优"。
- `kind === "infeasible"` 时显示失败列表和需要的数值。

## 9. Hook 到自己的实现

### 9.1 推荐接口: Worker 层

主线程代码等价于:

```js
const worker = new Worker(
  new URL("/assets/optimizer.worker-5V9qh1RK.js", import.meta.url),
  { type: "module" }
);
worker.onmessage = (e) => {
  const msg = e.data;
  if (msg.requestId !== requestId || msg.purpose !== purpose) return;
  if (msg.error || !msg.result) reject(new Error(msg.error));
  else resolve(msg.result);
};
worker.postMessage({ ...request, requestId, dataKey, data });
```

因此只要替换这个 Worker, 保持消息协议即可:

```js
const NativeWorker = window.Worker;

const myWorker = {
  onmessage: null,
  onerror: null,
  postMessage(req) {
    Promise.resolve()
      .then(() => solveLocally(req))
      .then((result) => {
        this.onmessage?.({
          data: { requestId: req.requestId, purpose: req.purpose, result }
        });
      })
      .catch((err) => {
        this.onmessage?.({
          data: {
            requestId: req.requestId,
            purpose: req.purpose,
            error: String(err && err.message || err)
          }
        });
      });
  },
  terminate() {},
  addEventListener() {},
  removeEventListener() {}
};

window.Worker = new Proxy(NativeWorker, {
  construct(target, args) {
    if (String(args[0]).includes("/assets/optimizer.worker-")) {
      return myWorker;
    }
    return new target(...args);
  }
});
```

注意:

- 必须在页面脚本创建 worker 之前安装, 例如扩展的 `document_start`。
- 页面只赋值 `onmessage` / `onerror`, 不完全依赖 `addEventListener`。
- `terminate()` 后 worker 会被重新创建, 代理必须能重复返回实例。
- 主线程不会等待你完成旧请求, `terminate()` 之后应丢弃在途结果。
- 回复里的 `requestId` 和 `purpose` 必须与请求完全一致, 否则被静默丢弃。
- 请求的 `data` 可能为空, 需要按 `dataKey` 缓存上一次的 weapon payload。

### 9.2 其他 hook 位置

| 方式 | 说明 | 风险 |
|---|---|---|
| Service Worker 拦截 `/assets/optimizer.worker-*.js` | 返回自己的 module worker 脚本 | 需要控制 scope, 构建 hash 会变 |
| 浏览器扩展重定向资源 | 把 worker 文件映射到本地实现 | 同样依赖 URL 模式 |
| 自建镜像站 | 直接替换该 asset | 站点更新后需同步数据协议 |
| 劫持 worker 的 `postMessage` | 包装原生 worker, 请求/响应改道 | 只能在首次创建前安装 |
| 改压缩后的主 bundle 函数 | 直接替换 `yu` / `mu` | 混淆名每次构建都会变, 不推荐 |

### 9.3 自己实现求解器需要保证的输出

最小可用 result:

```json
{
  "selections": {},
  "metrics": {
    "attributes": { "10001": 0, "10005": 0, "10006": 0, "10007": 0, "10008": 0 },
    "currentPrice": 0,
    "systemPrice": 0,
    "missingCurrentPriceCount": 0,
    "missingSystemPriceCount": 0,
    "installedPartCount": 0
  },
  "ranges": {},
  "rangeWitnesses": {},
  "unmetItems": [],
  "allTargetsSatisfied": true,
  "elapsedMs": 0,
  "evaluatedStates": 0,
  "generatedStates": 0,
  "truncated": false,
  "smartRecommendation": {},
  "candidates": []
}
```

UI 真正读取的字段:

```text
smartRecommendation.kind
smartRecommendation.candidate.selections
smartRecommendation.candidate.tuningState
smartRecommendation.candidate.metrics.attributes
smartRecommendation.candidate.installedPartKeys
smartRecommendation.variants
smartRecommendation.failures
truncated
```

`variant.changedPartCount` / `changedTuningCount` 会被前端重新计算一遍, 可以随便给。

## 10. 观察到的实现问题与风险

以下基于静态代码阅读, 方便对比自己的实现:

1. 结果不是全局最优。beam 宽度 128, 最多 24 轮, 每槽位还剪枝到少量候选。
2. 时间预算是墙钟时间, 同一配置在不同机器上可能得到不同结果。
3. 勾选精调时最短搜索时间被抬到 5 秒; 不勾选时最短约 2 秒。短时间搜索很容易 `truncated`。
4. 智能模式会清空未锁定的当前配件和精校, 推荐不是 "在当前方案上微调", 而是从空方案重建。
5. 基础偏好的等级是软权重; 只有 "指定数值" 才变成硬约束。
6. `targets.enabledAttributeIds` 永远为空, 所以主属性目标值本身不构成硬约束。
7. 偏好归一化用 "本次搜索采样到的最大值" 作分母。采样不全会让同一方案在不同搜索深度下分数变化。
8. 状态排序里属性总分出现两次, 实际权重被放大。
9. `ranges` 只统计已评估状态, 不是理论可达范围。
10. 多方案只在至少两个偏好组同时选 "最高" 时出现, 其他等级不会产生侧重方案。
11. 精调关闭时, 未锁定精校全部回默认值, 当前精校配置不会保留。
12. 细分偏好缺少指标时会直接产生 `detail-unavailable` 失败, 而不是忽略该偏好。
13. 预算/战备缺价时, 缺价计数优先级高于属性收益, 可能导致推荐偏保守。
14. worker 被 terminate 后重新创建, weapon payload 会重新传输一次。
15. `purpose: "analyze"` 分支存在于 worker, 但当前前端不发。
16. 所有求解都在本地, 抓包看不到智能配装的接口; 如果要复现, 只能走 worker 协议。

## 11. 复现顺序

```text
1. 打开 /gunsmith/ 并选好武器, 等 manifest + weapon + prices 加载完
2. 在页面脚本执行前安装 Worker 代理
3. 点 "生成智能配件方案"
4. 截获第一个 postMessage:
   - 检查 dataKey / weaponId / mode / smartOptions / targets
   - data 可能为空, 用 dataKey 缓存上次 payload
5. 按第 7 节构造 result
6. postMessage 形状的 {requestId, purpose, result} 交回主线程
7. 页面应显示 "已生成智能配件方案" 和方案卡片
8. 点 "应用方案" 验证 selections 与 tuningState 生效
```
