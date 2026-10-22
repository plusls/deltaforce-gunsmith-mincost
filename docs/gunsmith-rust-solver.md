# Gunsmith 精确改枪求解器 (Rust / WASM)

本文记录站点的"生成智能配件方案"为什么不是最优搜索, 以及本仓库 Rust 求解器的算法, 复杂度和实测结果.

数值模型见 `docs/gunsmith-numerical-model.md`, 站点 worker 协议见 `docs/gunsmith-smart-build.md`.

## 1. 结论

- 站点优化器是客户端 beam search, 目标是"偏好分数", 不是"最低价格", 也没有最优性保证.
- 本仓库实现的是约束下的最低总价搜索. 算法为: 结构槽 DFS + 叶子槽 Pareto DP + 分支定界, 叶子用完整数值模型精确评估.
- 对 RM277 (`18010000049`), 约束 `全自动水平 <= 0.6`, `全自动垂直 <= 0.6`, `开火稳定性 <= 0.8`, 求得最优总价 **175,891** (枪械本体 93,765 + 配件 82,126), 搜索 5,223 节点 / 10,169 叶子, 原生约 0.9 秒, WASM 约 1.25 秒.
- 改枪码 `6LM1E480BO0F121KNDPVA` 无法离线解码, 其配置和价格未知. 站点接口 `/api/gunsmith/share-codes/<code>` 返回 404 `share_code_not_ready`. 改枪码只保存 selections 和 tuning, 不包含价格.
- 在本组约束下, 求解器的早期版本找到过 182,996 的可行解, 当前精确版本改进到 175,891. 站点 beam search 没有最优性保证.

## 2. 问题定义

给定:

- 武器 id, 模式 (`operations` / `warfare`)
- 激光开/关, 两脚架展开/收起
- 价格表 (`partId -> price`)
- 锁定路径, 锁定精校, 排除配件
- 约束列表: 指标 id + min/max

求:

    minimize  weaponPrice + sum(installed part price)
    subject to  metric_i(build) <= max_i
                metric_i(build) >= min_i
                装配约束 (socket, extraOccupy, virtual rules, defaults)
                精校值在合法范围内

支持的指标:

- `recoil-continuous-horizontal`
- `recoil-continuous-vertical`
- `recoil-single-horizontal`
- `recoil-single-vertical`
- `recoil-fire-stability`
- 面板属性 id: `10001`, `10005`, `10006`, `10007`, `10008`
- 细分指标:
  - handling: `handling-aim-time`, `handling-sprint-fire-delay`, `handling-aim-move-speed`, `handling-reload-time`, `handling-empty-reload-time`, `handling-holster-time`, `handling-switch-time`
  - stability: `stability-breath-range`, `stability-breath-duration`, `stability-movement-camera`, `stability-fire-camera`, `stability-hit-sway`, `stability-hold-breath-recoil`
  - hipfire: `hipfire-standing-spread`, `hipfire-maximum-spread`

细分指标的计算口径 (handling 分数, ADS 曲线, 运行时修正倍率, 稳定/腰射/受击摆动等) 见 `docs/gunsmith-numerical-model.md`, 与站点 worker 逐值对齐.

价格按"每个已安装槽位实例"计费. 同一配件装两片护木片就付两次钱. 虚拟配件价格为 0.

## 3. 站点的算法为什么不对

站点文件: `/assets/optimizer.worker-5V9qh1RK.js`.

已确认的行为:

1. 状态是 preference 打分, 按"操控/后坐力/稳定"等偏好权重排序, 不是价格.
2. 每轮保留固定数量的状态 (`128`), 最多 `24` 轮. 这是 beam search, 被剪掉的分支不会回来.
3. 候选数超过阈值 (`32`) 时先按偏好裁剪候选, 可能把便宜且刚好满足约束的配件提前删掉.
4. 有时间预算和 `truncated` 标志, 超时直接返回当前最好解.
5. "智能模式"会剥离未锁定的当前配件/精校, 只从锁定项开始搜索. 这是产品设计, 不是 bug, 但意味着它不会以当前方案为起点做局部改进.
6. 它没有任何价格下界, 也没有 Pareto 支配或分支定界, 因此无法证明最优.

结论: 站点算法适合"生成一个看起来不错的方案", 不适合"在约束下找最便宜方案".

## 4. 复杂度

条件感知的全组合约 `5.44e12`. 纯暴搜不可行.

但真正影响后坐力约束的配件只有 62 个, 且大量槽位只影响非约束属性. 求解器先做相关性裁剪, 再做结构分解.

## 5. 算法

### 5.1 结构分解

把配件分成两类:

- **结构配件 (structural)**: 满足任一条件
  - 有 `extraOccupySocketIds`
  - 有子槽位 (`slotsByHost[partId]` 非空)
  - 出现在 virtual rule 的 `conditionItemIds` 中
  - 有激光 UI effect
  - 有 `aimRecoilProfilesByMode`
- **叶子配件**: 其余配件.

结构配件会改变装配树或条件语义, 必须在 DFS 中显式决定. 叶子配件只改变可加/可乘指标, 可以延后到 DP.

对每个可见槽位, DFS 只分支结构配件. 同时提供三种特殊动作:

- `Unset`: 使用槽位内部默认件/虚拟件, 如果它是结构件
- `Remove`: 显式 `null`, 如果结果件是结构件
- `Defer`: 该槽位完全交给叶子 DP 选择 (只允许非结构件, 以及 unset/remove)

`Defer` 是必须的. 否则像瞄准镜这种"全部候选都是结构件"的槽位会强迫 DFS 选一个镜子, 永远走不到"不装镜子"的最优分支.

### 5.2 叶子 Pareto DP

对一个固定的结构组合, 剩余叶槽位的状态为:

    (cost,
     D,
     A_H, A_V,
     C_H, C_V, C_K,
     E_H, E_V, E_K,
     hs, adsm, eopen,
     受约束面板属性值...)

其中:

- `D`: 主属性 `10005` 加成之和
- `A_H/A_V`: 原始 `rawRecoil.horizontal/vertical` 之和
- `C_H/C_V/C_K`: 原始 `Mult_C` 倍率之积
- `E_H/E_V/E_K`: 该配件精校可达的通道收益集合. 各通道上限不能同时取到, 因此状态上记录的是四维 (H/V/K/开镜) 收益向量的可达包络, 不是逐维最大值
- `hs`: handling 分数 (主要来自 `10006` Addend), 决定 `adsTime` 曲线输入
- `adsm`: `GAiming_ADSTime` 的 `Mult_A` 倍率之和
- `eopen`: 精校在 `handling-aim-time` 方向上的最大收益
- 面板属性: `displayMods` 中对应 `attributeId` 的 Addend 之和

支配关系 (A 支配 B):

    A.cost <= B.cost
    A.D    >= B.D
    A.A    <= B.A
    A.C    <= B.C
    A.E    >= B.E          (能力包络包含, 不是逐维比较)
    A.panel <= B.panel   (仅对 max 约束)
    A.hs   >= B.hs       (仅当 aim 约束是 max, 即需要更大 handling)
    A.hs   <= B.hs       (仅当 aim 约束是 min)
    A.adsm <= B.adsm
    A.eopen >= B.eopen   (max 约束) / <= (min 约束)

`handling-aim-time` 的 `min` 和 `max` 都参与 DP, 方向由 `AimDir` 决定; 同时出现 min 和 max 时方向退化为 `Ignore`, 该维度不做支配剪枝. 其余细分指标在 `dp_ok` 中被判为不可 DP, 直接退回逐组合枚举; 其中可归约的单调稳定性曲线指标按下一节映射到面板约束, 不落入这条回退.

每个槽位的选项包括 unset, remove 和所有非结构候选. 逐槽做笛卡尔积后立即做 Pareto 剪枝.

设计要点:

- `D` 越大, 后坐力曲线 `Cr(D)` 和 `Gk(D)` 通常越小, 所以 `D` 方向为越大越好. 实现在做界时同时评估曲线两端, 兼容非单调曲线.
- 精校每个 tune 只影响一个后坐力通道 (`%额外后坐力控制` 同时作用于 H/V, 值是同一个). 但精校通道互斥, 逐维取最大会剪掉真实可行的解, 所以支配判定比较的是状态上的可达能力包络: 只有当 A 的包络在每一维都覆盖 B 的包络时, A 才支配 B. 包络随状态合并, 去重也按包络等价判断.
- 激光/两脚架/瞄准镜这些带条件或 profile 的配件都在结构层, 不会进入 DP 的乐观估计.
- 精校可用性用 `tuningSocketAvailability` 和槽位名判断, 不可用时不记收益.
- 面板 DP 支持单独的 max 或 min 约束; 同一属性同时有 min 和 max 时关闭 DP. `10001` 始终关闭 DP.

DP 前沿每层上限 200,000 状态 (种子 pass 为 8,192). 超过时按价格截断并置 `heuristic_incomplete`, 结果 `optimal=false`; 该上限本身不置 `truncated`, 与时间/预算截断区分开.

12 维前沿 (带 `handling-aim-time` 约束) 在部分武器上会让单层远超 8,192 状态, 单次 DP 扫描耗时数分钟. 此时第一次精确 pass 会在前沿超过 8,192 且该层运行超过 1 秒后投机中止, 不产出结果; 随后用有界启发式 (10,000 work unit 或 8 秒) 抬高 incumbent, 再用未放宽的完整精确搜索重跑. 最终结果始终来自完整精确搜索, 中止只影响找上界的快慢.

调校需求的反解不使用线性闭式解, 而是用与精确评估相同的 `scale_ratio` 对单通道调校总量二分, 避免 `round2` 台阶把可行调校判成不可行或把不可行判成可行. `recoil-fire-stability` 的乐观下界同样扣除 `round2` 余量, 保证边界解不会被剪掉.

边界处理:

- 槽位自带结构默认件时, Defer 会被压制, DP 无法再选非结构件; 这类槽位把非结构候选保留在显式 action 列表里
- 后坐力约束的 max 为负, 或加值/倍率为负时, 常规 Pareto 支配和下界不再成立, 对应剪枝关闭并回退到全枚举
- 激光件的所有非 `con` Addend 都按选中激光的加成计入, 不只 `pro`
- `optimal=false` 表示未证明最优 (含时间/预算截断和内部前沿上限); `truncated=true` 只表示时间或预算耗尽

#### 单调曲线细分指标的归约

`stability-fire-camera`, `stability-movement-camera`, `stability-hold-breath-recoil`, `stability-breath-range`, `stability-breath-duration` 都是稳定度标量 `dS` 经单调曲线得到的. 若 payload 中稳定度类 Addend 只来自无条件的 `10007`, 且指标的其他输入 (呼吸倍率, 呼吸/移动稳定性除子, 调校效果) 可证明为常数, 求解前按曲线反解出 `10007` 的 min/max, 交给现有面板 DP, 不再关闭 DP.

反解与精确评估使用同一套曲线和 `round2` 语义: 对 max 约束取使 `round2(curve(dS)) <= max` 成立的最小 `dS`, 对 min 约束取最大 `dS`, 再把边界向外放宽 `1e-7`, 保证映射后的约束不窄于原约束. 曲线非单调, 稳定度贡献不止 `10007`, 或存在变化的呼吸/移动稳定性输入时不做映射, 继续走原有枚举路径. 原始指标始终保留在最终精确评估中, 映射只影响搜索路径, 因此不会改变报告出来的指标与数值.

### 5.3 分支定界

1. 先跑一遍便宜优先的 DFS, 找到第一个可行解作为上界. 叶子 DP 会给出该结构分支下的最便宜可行状态.
2. 用上界 `best_cost` 剪枝:
   - DP 组合时, 部分状态总价已经 `>= best_cost` 的直接丢弃
   - DFS 中已决定配件价格加上武器价超过上界的剪掉
3. 用 `cannot_satisfy` 做乐观下界:
   - 未决定槽位用 `slot_bounds` 的乐观值 (最大 D, 最小 A/C, 最小精校收益)
   - 计算最小的 `F_H`, `F_V`, `S`, 若仍大于某个 max 约束, 剪枝
4. DFS 顺序为便宜优先, 让上界尽快变小.

如果不需要最优性证明, 可以设 `firstSolution: true`. 这时只跑第一阶段的便宜优先 DFS, 找到第一个可行解就返回 (`optimal=false`). RM277 这组约束下只要 14 个节点 / 297 个叶子, 原生 32 ms, WASM 70 ms. 这个解恰好是 175,891, 但该模式不保证最优.

注意: 下界只累计"已决定"的槽位价格. 如果把未决定槽位的最低价也加上, 会因为换父配件导致子槽位消失而高估, 从而错误剪枝. 这是实现时踩过的坑.

精校前沿有独立的候选上限: 单个配件内和跨结构组合并的中间候选列表各受 `TUNE_NEXT_LIMIT` (16,384) 限制, Pareto 剪枝后每组最多保留 `TUNE_CAP` (4,096) 个非支配解. 触发任一上限都会置 `heuristic_incomplete`, 该结果不能声明最优. 缓存的前沿会记录构建时是否被截断, 后续 pass 复用被截断的缓存时重新置位 `heuristic_incomplete`, 因此种子 pass 的截断前沿不会被精确 pass 静默复用.

精确 pass 正常完成 (没有时间/预算截断) 后, 若该 pass 触发了精校前沿截断, 且距离 deadline 仍不少于 `TUNE_STAGE2_MIN_MS` (1,000 ms), 求解器会清空精校前沿缓存, 把上限提升到 `TUNE_STAGE2_CAP`/`TUNE_STAGE2_NEXT` (65,536/65,536) 重跑一次精确 pass. 重跑无截断时结果可证明最优; 撞到 deadline 时 `truncated=true`, 保留上一轮的 incumbent; 仍然截断时 `heuristic_incomplete` 保持. 第二阶段有独立的节点/叶子预算, 与既有多次 pass 的行为一致. `firstSolution` 模式只跑种子 DFS, 不进入第二阶段.

调校前沿的支配方向来自约束, 不是固定映射. `handling-sprint-fire-delay` 与 `handling-aim-time` 同为开镜速度通道 (`open`), `handling-aim-move-speed` 为移动通道, `stability-breath-*` 为呼吸通道, `stability-movement-camera` 为移动稳定性通道; 只有 max 时取"越大越好", 只有 min 时取"越小越好". 同一指标同时有 min 和 max (band) 时该通道为 `Equal`: 不同通道值的 offer 互不支配, 否则一个区间外的 offer 会把唯一落在区间内的 offer 支配掉, 导致更大调校空间反而报无解. `Equal` 通道用按 1e6 量化值分桶的 Pareto 剪枝, 并对相邻 ±1 桶探测, 避免 band 场景退化成 O(n²); 没有 `Equal` 通道或维度超过 3 时仍走全扫描. band 会显著增加前沿规模, 因此可能撞到精校上限并失去最优性证明, 但返回的仍是找到的最优价格.

前沿缓存把调校 key 按前沿存一份 (`Rc<Vec<String>>`), 每个 offer 只存 `Vec<f64>`, 不再为每个 offer 克隆 key 字符串. sprint band 用例 (18010000037) 因此从约 1.65 GB 峰值内存/102 s 降到约 465 MB/71 s, 结果与节点数不变.

pick 前的单调极值预过滤只在可证明安全时拒绝整条前沿: 开镜速度通道的区间跨越 `open = -100` (`1/(1+open/100)` 的 clamp 跳变点) 时跳过, 端点值方向与假设方向矛盾时 (例如 payload 出现负的呼吸曲线) 也跳过; 跳过只会少剪枝, 不会误剪.

### 5.4 叶子精确评估

DP 只负责生成候选组合和剪枝. 每个最终状态都会走一遍完整评估:

1. `Assembler::resolve` 解析真实装配
2. 计算精确的 `D/A/C`
3. 精校 DP: 每个可精校实例枚举 min/max/default/曲线拐点/步进值, 过滤被支配的精校组合, 选择满足约束且偏离默认值最小的方案
4. 计算 `F_H/F_V/S` 和显示倍率
5. 校验所有约束, 更新 best/alternatives

因此报告出来的数值与页面模型一致, DP 的乐观估计不会直接出现在结果里.

### 5.5 性能

搜索本身只走了 5,223 个节点, 时间主要花在每次节点/叶子的完整装配解析和精校评估上. 通过 perf 定位并做了以下优化:

- `decimals` 不再用 `format!` 解析浮点字符串, 改为数值循环
- 叶子配件的指标按 `(partId, slotName)` 缓存, 避免重复枚举精校
- 去重 key 用借用元组, 不再 `format!` 拼接字符串
- `VisibleSlot`/`InternalPart` 只保存需要的槽位字段 (`SlotView`), 避免克隆整个 `SlotDef`
- `Remove` 只在槽位存在真实默认件时才重新解析装配, 虚拟默认件直接复用 `Unset`

RM277 查询从 2.3 s 降到 0.9 s, WASM 从 3.2 s 降到 1.25 s.

### 5.6 精确性边界

- 无约束或只有 max 约束, 且未触发预算/前沿/时间截断: 在 DP 支配关系成立的假设下是全局最优.
- 有 min 约束: `dp_ok=false`, 完整枚举, 可能较慢.
- DP 前沿超过 200,000 (种子 pass 8,192): 置 `heuristic_incomplete`, `optimal=false`, 不置 `truncated`; 该上限不触发精校前沿的第二阶段重跑.
- 精校前沿超过 4,096 / 16,384 上限: 同样置 `heuristic_incomplete`, `optimal=false`. 若精确 pass 未因时间/预算被截断, 且剩余时间不少于 1,000 ms, 会用 65,536 上限重跑一次精确 pass, 见 5.3.
- 节点/叶子预算耗尽: 同样标记 `truncated`.
- `deadlineMs` 到期: 返回当前最好解, `truncated=true`, `optimal=false`. 时钟每 64 次预算检查读一次, 实际超时可能比设定值晚几十毫秒.
- `forbidBindings` 不参与优化. 站点自身也只在生成改枪码时用它剔除需要折叠状态的配件, 优化器不检查.

## 6. 代码结构

    solver/
      src/
        model.rs      payload serde 模型, 曲线求值
        assembly.rs   装配解析 (可见槽位/内部件/socket 占用/虚拟件)
        metrics.rs    数值模型, 精校, 价格, 面板
        search.rs     DFS + Pareto DP + 分支定界
        lib.rs        请求结构, C ABI, WASM 导出
        bin/solve.rs  CLI
      js/
        solver.mjs    WASM 加载与调用封装
        smoke.mjs     Node 冒烟测试

### 6.1 JSON 请求

```json
{
  "prices": { "partId": 1234 },
  "weaponId": "18010000049",
  "mode": "operations",
  "laserEnabled": true,
  "bipodDeployed": false,
  "lockedPathKeys": { "path": "partId" },
  "lockedTuningKeys": { "slotPath\u0000partId\u0000tuneId": 50 },
  "excludedPartIds": ["..."],
  "constraints": [
    { "id": "recoil-continuous-horizontal", "max": 0.6 },
    { "id": "recoil-continuous-vertical", "max": 0.6 },
    { "id": "recoil-fire-stability", "max": 0.8 }
  ],
  "maxEvaluations": 5000000,
  "maxLeaves": 2000000,
  "maxResults": 5,
  "costCap": null,
  "firstSolution": false,
  "deadlineMs": 5000
}
```

`weaponPayload` 可以在同一个对象里, 也可由 `solve_slices` 单独传入. 价格 key 用 `officialObjectId`, 缺失时回落到 `id`.

### 6.2 输出

```json
{
  "optimal": true,
  "truncated": false,
  "nodes": 5223,
  "leaves": 10169,
  "best": {
    "attachment_price": 82126,
    "total_price": 175891,
    "recoil": { "...": 0.55 },
    "panel": { "10005": 72.0 },
    "detail": { "aim_time": 355.0, "handling_score": 61.0, "...": null },
    "tuning": { "...": 50.0 },
    "constraints": [{ "id": "...", "value": 0.55, "max": 0.6, "ok": true }],
    "installed_parts": [{ "slot_path": "...", "part_id": "...", "price": 3541 }],
    "installed_part_keys": ["slotPath\u0000partId"],
    "selections": { "path": "partId" }
  },
  "alternatives": [],
  "message": "search complete"
}
```

### 6.3 CLI

```bash
cargo build --release

./target/release/solve \
  --payload weapon.json \
  --prices prices.json \
  --weapon 18010000049 \
  --constraint recoil-continuous-horizontal:0.6 \
  --constraint recoil-continuous-vertical:0.6 \
  --constraint recoil-fire-stability:0.8 \
  --first \
  --deadline-ms 5000 \
  --max-nodes 5000000 \
  --max-leaves 2000000
```

也可以把完整请求写进文件:

```bash
./target/release/solve --payload weapon.json --request request.json
```

约束语法:

- `id:max`
- `id:min:max`

### 6.4 WASM

```bash
cargo build --release --target wasm32-unknown-unknown -p delta-weapon-solver
```

产物: `target/wasm32-unknown-unknown/release/delta_weapon_solver.wasm`.

导出 C ABI:

```
dws_alloc(len: usize) -> *mut u8
dws_dealloc(ptr: *mut u8, len: usize)
dws_solve(payload_ptr, payload_len, request_ptr, request_len) -> *mut u8
```

`dws_solve` 返回 4 字节小端长度 + JSON 字节. 调用方读完后用 `dws_dealloc(ptr, len + 4)` 释放.

WASM 通过 `env.dws_now_ms` 导入单调毫秒时钟, 用于 `deadlineMs`. 实例化时必须提供:

```js
WebAssembly.instantiate(module, {
  env: { dws_now_ms: () => Date.now() },
});
```

浏览器/Node 封装:

```js
import { loadSolver } from "./solver/js/solver.mjs";

const solver = await loadSolver("/delta_weapon_solver.wasm");
const result = solver.solve(weaponPayload, request);
```

Node 冒烟测试:

```bash
node solver/js/smoke.mjs \
  target/wasm32-unknown-unknown/release/delta_weapon_solver.wasm \
  weapon.json request.json
```

## 7. 实测结果

### 性能回归用例

`solver/tests/slow_regression_test.rs` 固化了曾经很慢的真实请求 (开镜时间 mix、no_aim、aim400 等), 断言最优价格、`optimal=true`、`truncated=false`. 这些用例默认标记为 ignored, 改动求解器后需要显式跑一遍:

```bash
cargo test --release --test slow_regression_test -- --ignored --nocapture
```

原生 release 下六个用例单线程耗时合计约 18.5 s (测试默认并行执行).

AK12 五约束 (垂直/水平/开火稳定/开镜时间/开火镜头稳定) 加上锁定配件: 前端 200 s 预算下原先只返回首个可行解 438,540; 加入前沿中止与有界 incumbent 提升后 9.8 s 证明最优 208,037.

环境: WSL2, Rust 1.98.1, release + LTO.

### 7.1 RM277 约束搜索

约束:

```
recoil-continuous-horizontal <= 0.6
recoil-continuous-vertical   <= 0.6
recoil-fire-stability        <= 0.8
```

| 方案 | 总价 | 配件价 | 全自动 H/V | 开火稳定性 |
|---|---:|---:|---:|---:|
| 本求解器最优 | 175,891 | 82,126 | 0.55 / 0.55 | 0.79 |
| 早期 DFS 可行解 | 182,996 | 89,231 | 0.56 / 0.56 | 0.80 |
| 文档示例配置 | 305,822 | - | 0.59 / 0.59 | 0.80 |
| 本求解器 firstSolution | 175,891 | 82,126 | 0.55 / 0.55 | 0.79 |

最优解:

| 槽位 | 配件 | 价格 |
|---|---|---:|
| 后握把 | M7稳定后握把 `13030000110` | 3,541 |
| 枪口 | 钛金竞赛制退器 `13130000160` | 37,482 |
| 前握把 | 破晓垂直手电握把 `13140000041` | 13,398 |
| 托垫 | RM277托垫 `13240000009` | 27,705 |
| 武器 | RM277 `18010000049` | 93,765 |

精校: 后握把 `1019=50`, 托垫 `1016=50 / 1017=50`, 破晓握把 `1009=20`.

搜索结果:

- 节点 5,223
- 叶子 10,169
- 原生耗时约 0.9 s (`cargo build --release` 的 x86_64 CLI)
- Node + WASM 约 1.25 s (Node 24 加载 `.wasm`)
- `firstSolution` 模式: 原生 32 ms, WASM 70 ms, 同样得到 175,891
- `optimal=true`, `truncated=false`

### 7.2 价格上限

| 上限 | 结果 | 耗时 |
|---:|---|---:|
| 183,000 | 175,891 (最优) | 0.9 s |
| 170,000 | 无可行解 | 0.6 s |

上限比当前最优低时, 求解器会给出"无解"而不是返回超预算的解.

### 7.3 其他约束

| 约束 | 总价 | 结果 |
|---|---:|---|
| 无约束 | 93,765 | 裸枪, 无配件 |
| 开火稳定性 <= 0.7 | 141,739 | 可行 |
| 全自动水平 <= 0.45 | 175,557 | 可行, 0.44 |
| 面板 10005 <= 55 | 93,765 | 裸枪, 面板 48 |

面板 max 约束已纳入 DP 状态, 可以快速求解. 面板 min 约束会走完整枚举.

## 8. 改枪码

`6LM1E480BO0F121KNDPVA` 对应的接口:

```
GET https://moligod.com/api/gunsmith/share-codes/6LM1E480BO0F121KNDPVA
404 {"code":"share_code_not_ready","error":"改枪码尚未生成。"}
```

站点前端确实存在解码函数: 把服务端返回的树结构还原成 `selections` 和 `tuningState`, 再走同一套 `resolve`. 但该码没有在服务端生成记录, 因此无法离线恢复. 改枪码不包含价格, 即使解出 selections, 也需要价格表才能比较总价.

因此本文不与改枪码直接比较价格. 该码的价值在于提供了一个人工认为性价比高的参照, 促使我们实现了精确的最低总价搜索.

## 9. 复现

1. 从站点获取当前数据版本 payload 和价格, 存为 `weapon.json`, `prices.json`.
2. 构造请求 JSON, 参考 6.1.
3. 运行 CLI 或 WASM smoke.

如果使用站点 API, payload 是 AES-GCM 加密的, 解密流程见 `docs/gunsmith-numerical-model.md`.

## 10. 复杂枪械的可解性

### 10.1 复杂度来自哪里

制约搜索量的不是配件总数, 而是:

- 会被 DFS 分支的结构槽数量. 结构件会改变装配树或虚拟规则, 必须在 DFS 里显式决定.
- 每个结构槽的结构候选数. `Defer`/`Unset`/`Remove` 各算一个动作.
- 叶子槽的候选组合数. 由 Pareto DP 处理, 前沿上限 200,000.
- 是否存在 min 约束. 后坐力指标的 min 约束会关闭叶子 DP; 面板和 `handling-aim-time` 的 min 约束仍可 DP, 但同一指标同时有 min/max 会关闭 DP; 其余细分指标一律关闭 DP, 退化为逐组合枚举.
- 是否能尽快找到可行解. 找到上界后分支定界才能剪枝; 无解时没有上界.

搜索顺序是便宜优先, 因此"便宜但全部违反约束"的分支会被先枚举. 约束很紧时 (例如同时要求后坐力三项和 `handling-aim-time`), 首个可行解可能出现在很深的叶节点, 5 秒预算内找不到是可能的, 即使可行解实际存在.

截止时间的检查点: 每个 DFS 节点, 每个叶节点, 每次 DP 状态更新, 以及精校 Pareto 的每 1024 个候选. 精校前沿的两类中间候选列表 (单个配件内、跨结构组合并) 各上限 16,384, Pareto 剪枝后最多保留 4,096 个非支配解; 触发任一上限置 `heuristic_incomplete` (结果 `optimal=false`), 不置 `truncated`. 若精确 pass 未被时间/预算截断且剩余时间不少于 1,000 ms, 会清空精校前沿缓存并用 65,536/65,536 的上限重跑一次精确 pass; 重跑仍截断时 `heuristic_incomplete` 保持, 撞到 deadline 时保留上一轮 incumbent 并置 `truncated=true`. 带 `handling-aim-time` max 约束时, DP 状态会先用"最大开镜速度精校潜力"算一次乐观瞄准时间, 连乐观值都超限的状态直接丢弃.

### 10.2 静态复杂度

对已抓取的 46 把武器做静态统计 (结构件判定与 `Solver::new` 一致, 只统计与后坐力/稳定性相关的槽). `branch` 是上界: 各相关槽的结构候选数加一之后连乘, 不是实际 DFS 节点数.

| 武器 | 配件数 | 结构件 | 相关件 | 相关槽 | 有结构候选的槽 | 单槽最多 | branch |
|---|---:|---:|---:|---:|---:|---:|---:|
| M16A4 `18010000014` | 155 | 44 | 94 | 64 | 36 | 10 | 2.40e21 |
| CAR-15 `18010000031` | 151 | 45 | 94 | 64 | 36 | 10 | 2.67e21 |
| M4A1 `18010000001` | 153 | 43 | 93 | 57 | 33 | 10 | 2.50e19 |
| AKM `18010000006` | 161 | 42 | 103 | 41 | 22 | 12 | 1.45e12 |
| AUG `18010000015` | 103 | 29 | 50 | 31 | 22 | 10 | 3.54e14 |
| G3 `18010000023` | 139 | 31 | 83 | 33 | 19 | 10 | 4.73e09 |
| M249 `18040000002` | 136 | 31 | 79 | 31 | 19 | 10 | 3.08e11 |
| M700 `18060000009` | 136 | 35 | 70 | 29 | 18 | 14 | 1.49e09 |
| RM277 `18010000049` | 126 | 28 | 68 | 31 | 18 | 10 | 8.87e10 |

静态上界很大, 但实测节点数远低于上界, 主要靠便宜优先的种子解与分支定界.

以上统计基于 `7e60debf7b978ebec618a142` 版本, `operations` 模式. 全量 68 把武器中有 46 把已完成 payload 和价格抓取, 其余 22 把因接口返回 429 暂未抓取, 需要补测.

### 10.3 实测: 只有 max 约束

约束 `全自动水平 <= 0.6`, `全自动垂直 <= 0.6`, `开火稳定性 <= 0.8`:

| 武器 | 耗时 | 节点 | 叶子 | 结果 | 总价 |
|---|---:|---:|---:|---|---:|
| M16A4 | 357 ms | 2,307 | 4,474 | 最优 | 53,881 |
| CAR-15 | 318 ms | 1,417 | 5,238 | 最优 | 65,889 |
| M4A1 | 114 ms | 757 | 481 | 最优 | 130,902 |
| AKM | 1,423 ms | 1,990 | 23,229 | 最优 | 135,456 |

四把静态最复杂的武器都在 1.5 秒内给出最优解, `truncated=false`. 无约束时 (裸枪) 都是 10 到 50 ms. 这组结果说明: 对常见的后坐力上限约束, 结构分解加 Pareto DP 足以压住组合爆炸.

### 10.4 实测: 会失败的情况

| 武器 | 约束 | 结果 |
|---|---|---|
| M16A4 | 面板 `10005 >= 60` | 外部 timeout 60 s 杀掉 |
| AKM | 面板 `10005 >= 60` | 外部 timeout 60 s 杀掉 |
| CAR-15 | 面板 `10005 >= 60` | 1.9 s, 最优 23,144 |
| M16A4 | `H <= 0.2` 且 `V <= 0.2` | 外部 timeout 60 s 杀掉 |
| AKM | `H <= 0.2` 且 `V <= 0.2` | 外部 timeout 60 s 杀掉 |
| CAR-15 | `H <= 0.2` 且 `V <= 0.2` | 17.5 s, 证明无解 |

失败原因是搜索退化成了完整枚举:

1. **min 约束关闭叶子 DP**. `Solver` 的 `dp_ok` 要求所有约束都没有 `min`. 面板 `10005 >= 60` 会让 `dp_ok=false`: 不再生成 `Defer` 动作, DFS 必须对每个叶子槽逐一枚举全部候选, 同时失去 Pareto 剪枝.
2. **种子阶段不做剪枝**. 第一阶段为了尽快找到可行解, `seed=true` 时跳过 `cannot_satisfy` 和价格剪枝. 如果题目本身无解, 就没有上界, 第二阶段也不会启动, 整个结构空间被完整枚举.
3. **没有时间预算**. 默认 `max_nodes=5,000,000`, `max_leaves=2,000,000`, 单节点成本约几十到几百微秒, 跑满预算需要几分钟. 代码在 DFS 节点处检查预算, `leaf` 每 4096 个叶子检查一次, 但单次装配解析和精校评估无法中途中断.
4. **乐观下界不够紧**. `cannot_satisfy` 用 `slot_bounds` 的最大 D、最小 A/C、最小精校收益做乐观估计, 对 `H <= 0.2` 这种远低于可达范围的约束仍可能认为"有机会". 而且它在 `seed=true` 时完全不执行.

### 10.5 结论

- 对 max-only 的后坐力/稳定性约束: 可以, 已验证最复杂的几把枪都能在 1.5 s 内给出精确最优解.
- 对面板 min 约束: 不保证. 当前会退化成完整枚举, 复杂武器可能分钟级或更久.
- 对无解约束: 不保证及时返回. 简单武器 (CAR-15) 17.5 s 能证明无解, 复杂武器超过 60 s.
- 对"给一个可行解"的场景: `firstSolution` 模式只跑种子阶段, 通常几十毫秒, 但找不到时同样要枚举完.

### 10.6 已确定的改进方向

1. 让 DP 支持 min 约束: 按约束方向翻转支配关系, 或对每个受约束属性同时保留上下两端.
2. 种子阶段也执行 `cannot_satisfy` 剪枝. 该剪枝是可靠的, 不依赖已有可行解.
3. 在叶子枚举内部加入时间/状态检查点, 超时返回 `truncated=true`, 而不是无限运行.
4. 无可行解时用乐观下界做完整的最优性/无解证明, 不再只依赖种子解.
5. 结构 DFS 的顶层分支并行化, 原生多线程, WASM 用 worker.

## 11. 后续可做

- 支持面板 min 约束的 DP (需要按约束方向翻转支配关系).
- 把 DP 前沿做成真正的多目标优化, 输出价格-后坐力曲线.
- 用真实浏览器 worker 对照站点 beam search 的结果, 做逐约束的对比基准.
- 把求解器接到站点 worker 的 hook 上, 替换"生成智能配件方案". 目前未修改站点代码.
