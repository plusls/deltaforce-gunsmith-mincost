# Gunsmith 油猴脚本接口设计

目标: 在 moligod 改枪页面上加一个"最低开销"按钮, 用本仓库的 Rust/WASM 求解器旁路站点的 beam search.

实现位置:

```text
solver/js/site-adapter.mjs      站点请求 -> 求解请求, 求解结果 -> 站点结果
solver/js/deadline.mjs          时限常量, 站点时长解析, 生效时限与状态文案
solver/js/engine.mjs            WASM 引擎封装
userscript/src/main.mjs         Worker hook, 按钮, 设置
userscript/src/worker-main.mjs  独立 worker 内的求解循环
userscript/build.mjs            打包: 内联 wasm base64 和 worker 源码
userscript/test/build.test.mjs  打包与 worker 协议测试
```

构建:

```bash
cargo build --release --target wasm32-unknown-unknown
node userscript/build.mjs
# 产物: userscript/dist/gunsmith-mincost.user.js
```

测试:

```bash
node --test solver/js/*.test.mjs userscript/test/*.test.mjs
```

## 1. 原则

- 求解核心不持有任何武器数据. payload, 价格, 当前配装, 锁定项, 约束全部由调用方传入.
- 油猴脚本只做三件事: 取数据, 转协议, 回结果.
- 不依赖 React 内部状态, 不模拟配件点击.
- 不需要自己解密数据. 站点在把请求发给 optimizer worker 前已经完成解密, 并把 payload 和价格放进了 `postMessage` 参数.

## 2. 数据从哪来

站点主线程在 `Gunsmith` chunk 里创建 worker:

```js
new Worker(new URL("/assets/optimizer.worker-5V9qh1RK.js", import.meta.url), {
  type: "module",
})
```

发出去的请求已经包含全部需要的数据:

```js
{
  requestId, purpose: "solve", dataKey,
  data,                    // 完整 weapon payload, 解密后的对象
  weaponId, mode, laserEnabled,
  currentSelections, tuningState,
  lockedPathKeys, lockedTuningKeys, excludedPartIds,
  pricesByObjectId,        // { officialObjectId: { currentPrice, systemPrice } }
  targets, smartOptions,
  maxEvaluations, timeBudgetMs
}
```

关键点:

- `data` 只在 `dataKey` 变化时发送. 脚本必须按 `dataKey` 缓存, 后续请求可能只有价格和锁定项.
- `pricesByObjectId` 每次都发送.
- `timeBudgetMs` 是站点自己的搜索时长 (`smartSearchSeconds * 1000`), 同一请求还有 `maxEvaluations = smartSearchSeconds * 15000`.
- 脚本在 `document-start` 包一层 `Worker`, 只处理 URL 包含 `optimizer.worker` 的实例.
- 如果某次请求既没有 `data` 也没有缓存, 可以退回拦截 `crypto.subtle.decrypt` 或直接请求 API, 但这是异常路径.

## 3. 分层

```text
moligod 页面
  |
  | Worker.postMessage(站点请求)
  v
[userscript site adapter]      取 payload / 价格 / 锁定 / 当前配装
  |
  | GunsmithContext + MinCostOptions
  v
[userscript engine wrapper]    上下文 -> 求解请求; 求解结果 -> 站点结果
  |
  | JSON
  v
[Rust/WASM solver]             与站点数据完全解耦
```

对应文件:

```text
solver/src/*                  Rust 核心, 只认 JSON 请求
solver/js/solver.mjs          WASM 加载与 C ABI 封装
solver/js/site-adapter.mjs    上下文转换, 约束映射, 结果转换
solver/js/deadline.mjs        时限常量, 站点/自定义时限解析, 状态文案
solver/js/engine.mjs          创建引擎, 统一 solve 入口
userscript/src/main.mjs       入口, Worker hook, 按钮, 设置持久化
userscript/src/worker-main.mjs  worker 内初始化与求解
userscript/build.mjs          生成单文件 userscript
```

## 4. 接口

### 4.1 GunsmithContext

从站点请求中提取, 也是其他宿主 (CLI, 测试) 可以构造的对象.

```ts
type PricePair = {
  currentPrice: number | null;
  systemPrice: number | null;
};

type GunsmithContext = {
  dataVersion: string;
  weaponId: string;
  mode: "operations" | "warfare" | "blackhawk";
  laserEnabled: boolean;
  bipodDeployed?: boolean;
  data: WeaponPayload;
  pricesByObjectId: Record<string, PricePair>;
  currentSelections: Record<string, string>;
  tuningState: Record<string, number>;
  lockedPathKeys: string[];
  lockedTuningKeys: string[];
  excludedPartIds: string[];
};
```

### 4.2 MinCostOptions

```ts
type SolverConstraint = {
  id:
    | "recoil-continuous-horizontal"
    | "recoil-continuous-vertical"
    | "recoil-single-horizontal"
    | "recoil-single-vertical"
    | "recoil-fire-stability"
    | "handling-aim-time"
    | "handling-sprint-fire-delay"
    | "handling-aim-move-speed"
    | "handling-reload-time"
    | "handling-empty-reload-time"
    | "handling-holster-time"
    | "handling-switch-time"
    | "stability-breath-range"
    | "stability-breath-duration"
    | "stability-movement-camera"
    | "stability-fire-camera"
    | "stability-hit-sway"
    | "stability-hold-breath-recoil"
    | "hipfire-standing-spread"
    | "hipfire-maximum-spread"
    | "10001" | "10005" | "10006" | "10007" | "10008";
  min?: number;
  max?: number;
};

type MinCostOptions = {
  constraints: SolverConstraint[];
  priceSource: "current" | "system";
  lockMode: "locked" | "current";
  mode: "full" | "fast";              // 默认 full

  costCap?: number | null;
  maxResults?: number;
  maxNodes?: number;
  maxLeaves?: number;
  deadlineMs?: number;
  bipodDeployed?: boolean;

  missingPricePolicy?: "reject" | "fallback-system";
};
```

字段语义:

| 字段 | 说明 |
|---|---|
| `constraints` | 硬约束. 只有表中列出的指标支持精确求解 |
| `priceSource` | `current` = 市场价 (最小开销), `system` = 系统价 (战备值) |
| `lockMode` | `locked` 只固定用户锁定槽位, 与站点行为一致; `current` 固定当前整套配装, 只用于对比 |
| `mode` | `full` 完整搜索并证明最优; `fast` 只找第一个可行解, 对应求解器 `firstSolution` |
| `costCap` | 总价上限, 对应站点预算 |
| `deadlineMs` | 软截止时间 (毫秒), 超时返回当前最好解并标记 `truncated`. 默认跟随站点"搜索时长", 没有站点控件时 full 5000 / fast 2000 |
| `missingPricePolicy` | 缺价时拒绝该配件, 或用系统价代替 |

`costCap` 与 `priceSource` 是解耦的. 站点预算的原始语义是**市场价上限** (站点 worker 始终用 `currentPrice` 判断 `budgetPrice`), 而脚本允许用同一个 `budgetPrice` 去约束系统价. 当 `priceSource: "system"` 且预算启用时, 求解器按系统价判断预算, 结果状态会附注 `预算按系统价约束, 市场价可能超预算`, 并且不再把该结果显示为无警告的"最优解". 系统价达标不代表市场价达标; 需要市场价预算时请把价格来源切回 `current`.

### 4.3 引擎 API

```ts
type MinCostResult = {
  kind: "optimal" | "feasible" | "infeasible" | "error";
  build?: {
    selections: Record<string, string>;
    tuning: Record<string, number>;
    installedPartKeys: string[];
    attributes: Record<string, number>;
    detail: Record<string, number | null>;
    recoil: Record<string, number>;
    totalPrice: number;
    attachmentPrice: number;
  };
  optimal: boolean;
  truncated: boolean;
  priceSource: "current" | "system";
  costCap: number | null;
  nodes: number;
  leaves: number;
  elapsedMs: number;
  message: string;
  alternatives?: MinCostResult["build"][];
};

interface MinCostEngine {
  solve(context: GunsmithContext, options: MinCostOptions): MinCostResult;
  solveSiteRequest(
    context: GunsmithContext,
    siteRequest: unknown,
    overrides?: MinCostOptions,
  ): MinCostResult;
}
```

创建引擎:

```js
const engine = await createMinCostEngine({
  wasm: wasmBytes,       // Uint8Array / ArrayBuffer / WebAssembly.Module
  wasmBase64: base64,    // 或直接传 base64 字符串
});
```

`solve` 是同步纯函数: WASM 初始化后不读取全局状态, 不修改传入对象. 油猴脚本把它放进独立 worker, 所以页面主线程不阻塞. CLI, Node 测试和油猴脚本共用同一套接口.

### 4.4 上下文 -> 求解请求

```js
function toSolveRequest(ctx, options) {
  const prices = {};
  const excluded = new Set(ctx.excludedPartIds);

  for (const [id, price] of Object.entries(ctx.pricesByObjectId)) {
    const value = options.priceSource === "system"
      ? price.systemPrice
      : price.currentPrice;
    if (value != null && value > 0) {
      prices[id] = Math.round(value);
    } else if (options.missingPricePolicy !== "fallback-system") {
      excluded.add(id);
    } else if (price.systemPrice > 0) {
      prices[id] = Math.round(price.systemPrice);
    } else {
      excluded.add(id);
    }
  }

  const locked = {};
  for (const path of ctx.lockedPathKeys) {
    locked[path] = ctx.currentSelections[path] ?? null;
  }
  if (options.lockMode === "current") {
    for (const [path, partId] of Object.entries(ctx.currentSelections)) {
      if (partId) locked[path] = partId;
    }
  }

  const lockedTuning = {};
  for (const key of ctx.lockedTuningKeys) {
    if (Number.isFinite(ctx.tuningState[key])) {
      lockedTuning[key] = ctx.tuningState[key];
    }
  }

  return {
    prices,
    weaponId: ctx.weaponId,
    mode: ctx.mode,
    laserEnabled: ctx.laserEnabled,
    bipodDeployed:
      typeof options.bipodDeployed === "boolean"
        ? options.bipodDeployed
        : ctx.bipodDeployed === true,
    lockedPathKeys: locked,
    lockedTuningKeys: lockedTuning,
    excludedPartIds: [...excluded],
    constraints: options.constraints,
    maxResults: options.maxResults ?? 3,
    maxNodes: options.maxNodes ?? 2_000_000,
    maxLeaves: options.maxLeaves ?? 1_000_000,
    costCap: options.costCap ?? null,
    firstSolution: options.mode === "fast",
    deadlineMs: options.deadlineMs ?? null,
  };
}
```

缺价处理是必须的. Rust 核心把缺失价格当作 0, 对"最小开销"是危险的. adapter 在进入求解器前就排除缺价件, 或显式用系统价替代. CLI 的 `--prices` 也采用严格模式: 官方配件缺价直接排除, 除非传 `--allow-missing-prices`.

脚架状态: 站点 optimizer worker 完全不读取 `bipodDeployed` (不传 ls/or), 只有站点在线面板会应用脚架状态. 为了与 optimizer 基线一致, 脚本求解时固定传 `bipodDeployed: false`, 不再读取 UI 的 `[aria-label="脚架状态"]` 按钮或 `moligod:gunsmith-draft:v1` 草稿; 镭指等其他项仍跟随站点请求, 因此不会被 UI 状态改变. Rust/CLI 侧的 `--bipod-deployed` 保留, 直接调用求解器时仍可显式开启. `window.__moligodMinCostLast.options.bipodDeployed` 也固定为 `false`.

### 4.5 求解请求 -> 站点结果

站点 `smartRecommendation` 只读取以下字段:

```text
kind
candidate.selections
candidate.tuningState
candidate.installedPartKeys
candidate.metrics.attributes
changedPartCount / changedTuningCount
```

转换:

```js
function toSiteResult(ctx, r) {
  if (r.kind === "infeasible" || r.kind === "error" || !r.build) {
    return {
      selections: ctx.currentSelections,
      metrics: { attributes: currentAttributes(ctx) },
      ranges: {},
      rangeWitnesses: {},
      unmetItems: [],
      allTargetsSatisfied: false,
      elapsedMs: r.elapsedMs || 0,
      evaluatedStates: r.nodes || 0,
      generatedStates: r.leaves || 0,
      truncated: r.truncated || false,
      smartRecommendation: {
        kind: "infeasible",
        currentMetrics: { attributes: currentAttributes(ctx) },
        candidate: undefined,
        failures: [{ kind: "solver", message: r.message }],
      },
      candidates: [],
    };
  }

  const selections = {};
  for (const [path, partId] of Object.entries(r.build.selections)) {
    if (typeof partId === "string") selections[path] = partId;
  }

  const candidate = {
    selections,
    tuningState: r.build.tuning,
    installedPartKeys: r.build.installedPartKeys,
    metrics: {
      attributes: r.build.attributes,
      detailMetrics: detailMetricMap(r.build.detail),
    },
  };

  return {
    selections: ctx.currentSelections,
    metrics: { attributes: currentAttributes(ctx) },
    ranges: {},
    rangeWitnesses: {},
    unmetItems: [],
    allTargetsSatisfied: true,
    elapsedMs: r.elapsedMs,
    evaluatedStates: r.nodes,
    generatedStates: r.leaves,
    truncated: !r.optimal,
    smartRecommendation: {
      kind: "recommendation",
      currentMetrics: { attributes: currentAttributes(ctx) },
      candidate,
      changedPartCount: 0,
      changedTuningCount: 0,
      score: 0,
      variants: [],
      failures: [],
    },
    candidates: [candidate],
  };
}
```

`changedPartCount` 和 `changedTuningCount` 站点会自己重算, 这里给 0 即可.

## 5. 站点约束映射

| 站点字段 | 求解约束 |
|---|---|
| `preferenceConditions.recoil` | `10005` |
| `preferenceConditions.handling` | `10006` |
| `preferenceConditions.stability` | `10007` |
| `preferenceConditions.hipfire` | `10008` |
| `advancedPreferenceTargets["recoil-continuous-vertical"]` | 同名指标 |
| `advancedPreferenceTargets["recoil-continuous-horizontal"]` | 同名指标 |
| `advancedPreferenceTargets["recoil-single-vertical"]` | 同名指标 |
| `advancedPreferenceTargets["recoil-single-horizontal"]` | 同名指标 |
| `advancedPreferenceTargets["recoil-fire-stability"]` | 同名指标 |
| `advancedPreferenceTargets["handling-*"]` | 同名指标 (7 个 handling 细分指标) |
| `advancedPreferenceTargets["stability-*"]` | 同名指标 (6 个 stability 细分指标) |
| `advancedPreferenceTargets["hipfire-*"]` | 同名指标 (2 个 hipfire 细分指标) |
| `advancedPreferences` / `advancedPreferenceValues` / `advancedPreferenceOperators` | 旧版三元组, 与 `advancedPreferenceTargets` 等价 |
| `targets.budgetEnabled` | `costCap = budgetPrice` |
| `targets.preparednessEnabled` | 暂不支持, 需要双价格维度 |
| `preferences` 等级 | 忽略. 这是软偏好, 最小开销不参与 |

`operator: gte` -> `min`, `lte` -> `max`.

细分指标的计算口径见 `docs/gunsmith-numerical-model.md`, 与站点 worker 逐值对齐. 结果通过 `detailMetrics` 回传, 键名与约束 id 相同, 值为 `null` 的字段被省略.

支持的指标在求解器里有不同代价:

- 面板属性和后坐力指标走叶子 DP, 最快.
- `handling-aim-time` 的 `min`/`max` 都参与 DP (状态维度 `hs`, `adsm`, `eopen`), 可以和后坐力约束混合.
- 其余 14 个细分指标会关闭叶子 DP, 退化为逐组合枚举. 约束越松越慢; 建议同时设 `deadlineMs`.

对不支持的指标, 必须把失败原因返回给用户. 静默忽略会生成一个违反用户硬阈值的"最优解".

站点 UI 还有一个坑: `enabledAttributeIds` 永远为空, `attributeTargets` 实际不参与硬约束. 脚本不要把它当成用户约束.

## 6. 战备 / 双价格

站点支持"卡战备": 在市场总价尽量低的同时保证系统价不低于某个值. 这不是单目标最小化.

当前 Rust 请求只有一维 `prices`. 要支持这个功能, 需要:

```ts
prices: Record<string, number>;          // 市场价, 优化目标
systemPrices: Record<string, number>;    // 系统价, 约束维度
minSystemPrice: number | null;
```

算法改动:

- DP 状态增加 `system_price`
- 支配关系: `cost <=` 且 `system_price >=`
- 分支定界: 市场价下界剪枝, 系统价上界判断可行性
- 结果同时输出 `market_price` 和 `system_price`

在实现之前, 脚本遇到 `preparednessEnabled` 应当直接提示"暂不支持战备下限", 而不是返回一个可能不满足战备的方案.

## 7. 按钮与触发流程

推荐做法: 复用站点已有的生成按钮和结果卡片, 不自己画结果 UI.

```text
用户点击脚本按钮 "最低开销"
  -> 设置 nextRunMinCost = true
  -> 触发站点 .gunsmith-optimizer-generate 的 click
  -> 站点组装 solverInput, 调用 optimizer worker
  -> 脚本的 Worker 包装器收到请求
       - 缓存 data / prices / locks
       - 如果 nextRunMinCost, 不转发给原 worker
       - 转换为求解请求, 调 WASM
       - 用同一个 requestId / purpose 回一条站点格式的结果
  -> 站点 promise 解析, 渲染 "已生成智能配件方案"
  -> 用户点 "应用方案", 走站点原有 onApplySmartRecommendation
```

这样不需要碰 React state, 也不需要模拟槽位点击.

注意事项:

- 原 worker 可能被站点复用. 如果某次请求被脚本拦截而没有转发, 下一次转发时要检查 `data` 是否存在, 缺失则从脚本缓存注入, 否则原 worker 会报"枪械数据缓存未初始化".
- 站点可能同时存在多个请求. 每次响应必须带正确的 `requestId` 和 `purpose`.
- 脚本按钮触发生成前, 用户可以继续在站点面板里设置阈值. 脚本读取的就是这次请求里的 `targets` 和 `smartOptions`.
- 如果没有任何硬阈值, 最小开销就是裸枪. UI 应提示用户至少设置一个后坐力/稳定性/面板阈值.

## 8. WASM 加载与线程

WASM 产物约 773 KB, gzip 后约 225 KB.

推荐分发方式:

- 构建时把 wasm 转成 base64, 内联进脚本, 避免 CORS, `@connect` 和外部依赖.
- 启动时 `atob` -> `Uint8Array` -> `WebAssembly.compile`, 缓存 `WebAssembly.Module`.

线程:

- 原生约 0.9 s, 浏览器 WASM 约 1.2 s. 放在主线程会卡住页面.
- 优先在独立 Web Worker 中运行.
- 某些站点的 CSP 可能禁止 `blob:` worker. 需要探测, 失败时退回主线程, 并显示"计算中".
- 不要依赖站点自己的 optimizer worker 来执行任意代码, 它加载的是远程 bundle, 无法注入.

## 9. 时间预算与失败处理

### 9.1 站点自己的时限

站点在"生成智能配件方案"按钮旁有一个搜索时长输入:

```text
选择器: .gunsmith-smart-search-duration input[aria-label="搜索时长（秒）"]
单位:   秒, step 1
默认:   15
范围:   2-60; 勾选"精调"时下限 5 秒 (内部常量 lu = 5000 ms)
上限:   60 秒, 没有"无限"选项
持久化: moligod:gunsmith-optimizer-inputs:v1:<weaponId>:<mode>:<ordinary|preparedness>
        字段 smartSearchSeconds
```

请求字段:

```text
timeBudgetMs    = smartSearchSeconds * 1000
maxEvaluations  = smartSearchSeconds * 15000
```

站点 worker 在 `timeBudgetMs` 缺省时回退 1250 ms (`purpose=solve`) 或 2000 ms (`estimate`); 精调模式下下限 50 ms. 这些可以从站点 optimizer worker 的 bundle 中核对.

### 9.2 脚本的"时限"设置

| 值 | 行为 |
|---|---|
| 留空 (自动, 默认) | 优先级: 页面上站点输入框的当前值 > 该武器/模式的 `smartSearchSeconds` > 站点请求里的 `timeBudgetMs` > full 5000 ms / fast 2000 ms |
| 1-300 秒 | 覆盖站点值, 四舍五入到整秒; 超出范围按边界处理 |

300 秒是脚本上限 (`SOLVE_DEADLINE_MAX_MS`). 求解跑在独立 Worker 中, 页面不会卡, 但一次运行不会被自动打断, 所以不提供"无限". 需要更快反馈时用快速模式. 站点上限 60 秒只在自动模式下生效; 自定义可以超过它, 但不要在没有必要时设得过大.

设置保存在原来的 `moligod:gunsmith-mincost:v1`, 新增 `timeLimitSeconds`. 旧设置没有该字段时按自动处理. `optionsFromSiteRequest` 也会用站点请求里的 `timeBudgetMs` 作为 `deadlineMs` 的默认值 (同样按 300 秒上限截断), 显式 override 优先; 脚本在页面控件和 localStorage 都取不到时会用请求里的这个值, 而不是直接落到 5/2 秒.

### 9.3 求解器预算

```text
maxNodes    2,000,000
maxLeaves   1,000,000
deadlineMs  见 9.2 (站点或脚本设置)
```

Rust 侧通过 `env.dws_now_ms` 读取墙钟, 在 DFS 节点, 叶节点, DP 状态更新和精校 Pareto 循环中检查截止时间. 超时返回当前最好解并置 `truncated=true`; 若一个可行解都没有则返回 `infeasible`.

脚本状态栏 (生成按钮下方) 显示实际生效的时限:

| 情况 | 显示 |
|---|---|
| 运行中 | `求解中 · 时限 N 秒` |
| `optimal=true` | `最优解 · 用时 N 秒` |
| `truncated=true` 且有解 | `时限 N 秒 用尽 · 当前最好方案, 未证明最优` |
| 超时且无解 | `时限 N 秒 用尽 · 未找到可行方案` |
| 未截断且无解 | `当前约束无解 · 用时 N 秒` |
| 快速模式找到解, 未截断 | `找到可行方案 · 用时 N · 未证明最优` |

`truncated=true` 时不会声称最优. 站点结果对象也带 `truncated` 标记.

快速模式 `firstSolution=true` 在 RM277 上是 32 ms 原生 / 70 ms WASM. 代价是没有最优性证明.

## 10. 兼容性检查

脚本启动时检查:

- optimizer worker 的 URL 匹配 `/optimizer.worker-.*\.js$/`
- payload 至少包含 `parts`, `slotsByHost`, `defaultPresetsByWeapon`, `virtualRulesByWeapon`
- `pricesByObjectId` 非空
- 数据版本与构建版本不同时只警告, 不阻止运行. Rust 侧大多数字段有默认值, 但新增字段可能被忽略

站点 chunk 更新后, 类名和 worker URL 可能变化. 所有选择器都要有失败分支, 不能影响页面原有功能.

## 11. 实施状态与验证

已完成的阶段:

1. Rust 输出补齐: 5 个面板属性, `installedPartKeys`, `deadlineMs`.
2. `solver/js/site-adapter.mjs`: 上下文转换, 约束映射, 结果转换. 19 个 Node 测试.
3. `solver/js/engine.mjs`: WASM 加载, 统一 `solve` / `solveSiteRequest`. 6 个 Node 测试.
4. `userscript/src/*`: Worker hook, payload 缓存, 独立 worker 求解, 按钮和设置. 3 个构建/协议测试.
5. 真实页面验证 (RM277, 数据版本 `8629d01bf694f212aad051c5`).
6. 细分指标真实页面验证 (RM277, 2026-10-21): `handling-aim-time` 约束由脚本求解, 站点接受 "应用方案" 并用站点自身模型重算.
7. 可配置时限 (2026-10-21): 站点搜索时长控件和 worker 字段已核对; 脚本新增"时限"输入, 自动跟随站点, 支持 1-300 秒覆盖. `solver/js/deadline.mjs` 集中放常量与解析, 新增 deadline 单元测试和 1 个 adapter 测试, Node 测试全部通过.

验证结果:

```text
约束: vertical <= 0.46, horizontal <= 0.58, stability <= 0.77
锁定: 瞄具槽, 枪管槽
laserEnabled: false
完整模式: optimal, 总价 307614, 配件 201749, 5587 节点 / 9288 叶
          浏览器 2347 ms (含首次 WASM 编译), Node 1375 ms
快速模式: feasible, 0.26 s
应用方案: 站点精校生效, 属性变为 vertical 0.34, horizontal 0.57, stability 0.77
```

细分指标验证 (真实页面, 锁定瞄具/枪管, laser 关闭):

```text
约束: handling-aim-time <= 500
结果: optimal, 总价 199458, aim_time 396.11650485436894, 73 ms
应用方案后站点显示: 市场价 199458, 举镜时间 396ms, 跑射延迟 220.4ms,
                    换弹 2.2s, 空仓换弹 2.5s, 收枪/切枪 0.4s
```

```text
约束: handling-aim-time <= 400, recoil-continuous-vertical <= 0.46
结果: feasible (truncated), 总价 264656, aim_time 399.737876802097, 1922 ms
应用方案后站点显示: 市场价 264656, 举镜时间 400ms, vertical 0.45x
```

所有数值与求解器 `detail`/`recoil` 输出一致. 这两个用例同时暴露了一个真实 bug: 精校 Pareto 的 O(n^2) 循环没有截止时间检查, 单次 `runMinCost` 曾跑到 268 秒并冻结页面. 修复后同样的请求在 5 秒内返回.

时限验证 (真实页面, RM277, 2026-10-21):

```text
站点控件: input[aria-label="搜索时长（秒）"], 当时 min=5 (精调开), max=60, value=60
站点请求: purpose=solve, timeBudgetMs=5000, maxEvaluations=75000 (输入 5 时)
2 秒自定义, 约束 aim-time<=400 + fire-stability<=0.77 + vertical<=0.46:
  kind=error, truncated=true, 2064 ms, 状态栏 "时限 2 秒 用尽 · 未找到可行方案"
15 秒自定义, 同约束:
  kind=error, truncated=true, 15039 ms, 状态栏 "时限 15 秒 用尽 · 未找到可行方案"
自动模式 + 站点 5 秒:
  deadlineMs=5000, source=site, 状态栏 "求解中 · 时限 5 秒"
去掉 aim-time / fire-stability 后, 15 秒自定义:
  kind=optimal, 256 ms, 状态栏 "最优解 · 用时 256 毫秒"
```

用同样的 context / options 在 Node 里重跑, 节点数, 叶节点数, selections 和总价与浏览器完全一致, 说明 adapter 映射没有偏差.

站点自带的"生成智能配件方案"在同一组约束下返回无解, 说明 beam search 的 128 状态 / 24 轮限制确实会漏解.

安装: Tampermonkey 新建脚本, 粘贴 `userscript/dist/gunsmith-mincost.user.js`. 打开改枪页, 在站点生成按钮下方会出现"最低开销"按钮, 以及"完整模式", "市场价", "缺价排除"三个开关和"时限"输入. 开关样式沿用站点面板的深色风格; 取消"市场价"时"缺价排除"自动置灰. "时限"留空表示跟随站点"搜索"输入框, 填 1-300 表示覆盖. 按钮下方的状态栏显示本次实际时限和结果.

## 12. 已知限制

- 站点全部 15 个细分指标均已支持; 其中除 `handling-aim-time` 外的 14 个会关闭叶子 DP, 完整模式下可能接近截止时间.
- 后坐力三项加 `handling-aim-time` 同时收紧时, 5 秒完整模式可能只返回 `truncated` 且没有方案. 这是搜索顺序问题, 不代表无解; 可以放宽预算或减少约束.
- 多个非 DP 细分指标同时启用时, 单次叶子枚举可能长时间不返回, 状态栏会一直显示"求解中"; 此时只能关闭页面/刷新取消 (脚本没有暴露取消按钮).
- 站点没有"无限"时限, 60 秒是站点上限; 脚本自定义上限 300 秒是人为风险边界, 不是站点能力.
- 战备下限 (`preparednessEnabled`) 暂不支持, 会明确报错而不是静默忽略.
- 站点更新导致 worker URL, payload 字段或 class 名变化时脚本会失效. hook 只匹配 `/optimizer\.worker/`, 不影响其他 worker.
- 浏览器 CSP 禁止 `blob:` worker 时自动退回主线程, 大枪械会卡住页面约 1 到 2 秒.
- 内置 wasm 使脚本约 1.17 MiB, gzip 后约 370 KB.

## 13. 风险

- 站点更新导致 worker URL 或内部字段变化.
- `data` 缓存语义: 拦截请求后原 worker 缺数据.
- CSP 禁止 blob worker.
- 缺价件被当成 0, 产生虚假最低价.
- 未知的细分指标 id 被静默忽略 (必须先把失败原因返回给用户).
- 战备下限暂不支持.
- min 约束和无解约束当前可能很慢, 必须有超时和明确提示.
- 自定义时限过大 (最大 300 秒) 时单次运行会占用一个 Worker 线程最多 5 分钟; 页面仍可操作, 但结果迟迟不返回.
- 应用方案时 selections 里的 `null` 必须过滤, 站点只接受已安装件.
