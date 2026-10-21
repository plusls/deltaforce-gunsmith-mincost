# Gunsmith 油猴脚本接口设计

目标: 在 moligod 改枪页面上加一个"最低开销"按钮, 用本仓库的 Rust/WASM 求解器替换或旁路站点的 beam search.

本文只定义接口和数据流, 不包含具体脚本实现.

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
solver/js/site-adapter.mjs    上下文转换, 结果转换 (待实现)
userscript/gunsmith.user.js   入口, Worker hook, 按钮 (待实现)
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
    | "10001" | "10005" | "10006" | "10007" | "10008";
  min?: number;
  max?: number;
};

type MinCostOptions = {
  constraints: SolverConstraint[];
  priceSource: "current" | "system";
  lockMode: "locked" | "current";

  costCap?: number | null;
  fast?: boolean;
  maxResults?: number;
  maxNodes?: number;
  maxLeaves?: number;
  deadlineMs?: number;

  missingPricePolicy?: "reject" | "fallback-system";
};
```

字段语义:

| 字段 | 说明 |
|---|---|
| `constraints` | 硬约束. 只有表中列出的指标支持精确求解 |
| `priceSource` | `current` = 市场价 (最小开销), `system` = 系统价 (战备值) |
| `lockMode` | `locked` 只固定用户锁定槽位, 与站点行为一致; `current` 固定当前整套配装, 只用于对比 |
| `costCap` | 总价上限, 对应站点预算 |
| `fast` | 只找第一个可行解, 不证明最优 |
| `deadlineMs` | 软截止时间, 超时返回当前最好解并标记 `truncated` |
| `missingPricePolicy` | 缺价时拒绝该配件, 或用系统价代替 |

### 4.3 引擎 API

```ts
type MinCostResult = {
  kind: "optimal" | "feasible" | "infeasible" | "error";
  build?: {
    selections: Record<string, string>;
    tuning: Record<string, number>;
    installedPartKeys: string[];
    attributes: Record<string, number>;
    recoil: Record<string, number>;
    totalPrice: number;
    attachmentPrice: number;
  };
  optimal: boolean;
  truncated: boolean;
  nodes: number;
  leaves: number;
  elapsedMs: number;
  message: string;
  alternatives?: MinCostResult["build"][];
};

interface MinCostEngine {
  solve(context: GunsmithContext, options: MinCostOptions): Promise<MinCostResult>;
}
```

引擎实现建议:

```js
const engine = await createMinCostEngine({
  wasm: wasmBytes,          // ArrayBuffer, base64, URL 或 WebAssembly.Module
  worker: true,             // 尽量放进独立 worker
});

const result = await engine.solve(context, options);
```

`solve` 必须是纯函数: 不读取全局状态, 不修改传入对象. 这样 CLI, Node 测试和油猴脚本共用同一套接口.

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
    bipodDeployed: false,
    lockedPathKeys: locked,
    lockedTuningKeys: lockedTuning,
    excludedPartIds: [...excluded],
    constraints: options.constraints,
    maxResults: options.maxResults ?? 3,
    maxNodes: options.maxNodes ?? 2_000_000,
    maxLeaves: options.maxLeaves ?? 1_000_000,
    costCap: options.costCap ?? null,
    firstSolution: options.fast ?? false,
  };
}
```

缺价处理是必须的. Rust 现在把缺失价格当作 0, 对"最小开销"是危险的. adapter 必须在进入求解器前就排除缺价件, 或显式用系统价替代.

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
      detailMetrics: {},
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
| `targets.budgetEnabled` | `costCap = budgetPrice` |
| `targets.preparednessEnabled` | 暂不支持, 需要双价格维度 |
| `preferences` 等级 | 忽略. 这是软偏好, 最小开销不参与 |
| 其余 15 个细分指标 | 暂不支持, 明确报错, 不要静默忽略 |

`operator: gte` -> `min`, `lte` -> `max`.

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

建议默认值:

```text
maxNodes    2,000,000
maxLeaves   1,000,000
deadlineMs  5,000
```

Rust 目前没有 `deadlineMs`, 需要补一个按墙钟检查的入口. 没有它, 用户可能等几分钟.

返回给用户的状态:

| 情况 | 显示 |
|---|---|
| `optimal=true` | "约束下最低总价" |
| `truncated=true` | "时间/预算内最好方案" |
| `infeasible` | "当前约束无解", 列出失败原因 |
| 超时 | "计算超时", 可以切到快速模式 |

快速模式 `firstSolution=true` 在 RM277 上是 32 ms 原生 / 70 ms WASM. 代价是没有最优性证明.

## 10. 兼容性检查

脚本启动时检查:

- optimizer worker 的 URL 匹配 `/optimizer.worker-.*\.js$/`
- payload 至少包含 `parts`, `slotsByHost`, `defaultPresetsByWeapon`, `virtualRulesByWeapon`
- `pricesByObjectId` 非空
- 数据版本与构建版本不同时只警告, 不阻止运行. Rust 侧大多数字段有默认值, 但新增字段可能被忽略

站点 chunk 更新后, 类名和 worker URL 可能变化. 所有选择器都要有失败分支, 不能影响页面原有功能.

## 11. 实施顺序

1. Rust 输出补齐: 完整 5 个面板属性, `installedPartKeys` (或由 adapter 从 selections 生成), `deadlineMs`.
2. `solver/js/site-adapter.mjs`: 上下文转换, 约束映射, 结果转换. 用固定 JSON 做 Node 测试.
3. `solver/js/engine.mjs`: WASM 加载, worker 封装, `MinCostEngine` 接口.
4. `userscript/gunsmith.user.js`: Worker hook, 上下文缓存, 按钮, 拦截与回包.
5. 用 browser-harness 在真实页面验证:
   - 抓一次站点请求, 确认 payload 和价格完整
   - 点击脚本按钮, 确认站点卡片渲染
   - 点"应用方案", 确认 selections 和精校生效
6. 打包: base64 wasm, userscript 头, 版本号.

## 12. 风险

- 站点更新导致 worker URL 或内部字段变化.
- `data` 缓存语义: 拦截请求后原 worker 缺数据.
- CSP 禁止 blob worker.
- 缺价件被当成 0, 产生虚假最低价.
- 不支持的细分指标被静默忽略.
- 战备下限暂不支持.
- min 约束和无解约束当前可能很慢, 必须有超时和明确提示.
- 应用方案时 selections 里的 `null` 必须过滤, 站点只接受已安装件.
