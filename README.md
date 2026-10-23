# gunsmith-mincost

三角洲行动（moligod 改枪页）的约束改枪求解器。核心用 Rust 实现并编译为 WASM，通过 Tampermonkey 脚本嵌入改枪页，在站点原有“生成智能配件方案”旁提供“最低开销”求解。

站点的优化器是客户端 beam search，目标是偏好分数，不保证最优。本仓库给出的是另一个问题：在给定约束下求最低总价，并尽可能证明最优。

## 能力

- 约束求解：后坐力、开火稳定性、面板属性、操控/稳定性等细分指标，支持 min/max 区间。
- 精确数值模型：复刻站点的曲线、取整、激光主副属性、脚架、虚拟件、内部默认件与精校规则。
- 搜索：结构槽 DFS + 叶子槽 Pareto DP + 分支定界；可指定节点/叶子上限、截止时间、首个可行解。
- 模式：`operations`、`warfare`、`blackhawk`（黑潮爆破；站点数据的 key 为 `blackhawk`，标签写作“黑鹰爆破”），支持当前价/系统价与缺价策略。
- 浏览器端单线程：WASM 求解默认放在独立 Web Worker（创建失败时回退主线程），页面主线程只做数据转换与结果回填。

## 最优性

默认完整搜索会在预算内尝试证明最优。结果 JSON 的 `optimal` 字段表示结论状态：

- `true`：已穷尽所有可能更便宜的装配，`best.total_price` 是该 payload、价格表和约束下的最低价。
- `false`：命中节点/叶子预算、截止时间，或内部前沿截断，返回当前最好解，不声称最优。`message` 会说明是预算耗尽还是内部上限；脚本状态栏显示“未证明最优”。

`--first`（或脚本关闭“完整模式”）只找第一个可行解，始终返回 `optimal: false`。内部还会限制调校前沿和 DP 前沿的规模以控制浏览器耗时，触发时同样保持 `optimal: false`。

## 仓库结构

```text
solver/src/            Rust 求解器
solver/src/bin/solve.rs  原生 CLI
solver/js/             WASM 封装、站点适配、时限处理、smoke 脚本
solver/tests/          Rust 集成测试与真实 payload fixture
userscript/src/        油猴脚本入口与 Worker 源码
userscript/build.mjs   打包单文件 userscript（内联 wasm）
userscript/test/       构建与 worker 协议测试
docs/                  数值模型、站点 worker 协议、求解器算法、脚本设计
```

## 构建

依赖：

- Rust stable，以及 `wasm32-unknown-unknown` target
- Node.js 18+（使用内置 `node:test`）

```bash
rustup target add wasm32-unknown-unknown

cargo build --release
cargo build --release --target wasm32-unknown-unknown -p gunsmith-mincost
node userscript/build.mjs
# 产物: userscript/dist/gunsmith-mincost.user.js
```

## 原生 CLI

```bash
# 完整请求（含 weaponPayload）从 stdin 读取
target/release/solve < request.json

# 或 payload 与请求分开
target/release/solve --payload weapon.json --request request.json

# 或直接指定 payload 和约束
target/release/solve --payload weapon.json --prices prices.json \
  --weapon 18010000049 --mode operations \
  --constraint recoil-continuous-vertical:0.6 \
  --constraint recoil-continuous-horizontal:0.6 \
  --constraint recoil-fire-stability:0.8
```

`--help` 列出全部参数。常用选项：

| 选项 | 说明 |
| --- | --- |
| `--request request.json` | 完整请求或仅含求解参数的请求；不能与 `--prices` 同时使用 |
| `--prices prices.json` | 价格表，支持裸 map 或 `{"prices": {...}}`；缺价默认排除，`--allow-missing-prices` 视为 0 |
| `--constraint id:max` / `id:min:max` | 指标约束，可重复 |
| `--laser-off` | 关闭激光 |
| `--bipod-deployed` | 两脚架展开 |
| `--first` | 找到第一个可行解即停止，不证明最优 |
| `--deadline-ms N` | 软截止时间，超时返回当前最好解并标记 `truncated` |
| `--max-nodes N` / `--max-leaves N` | 搜索预算 |
| `--exclude partId` | 排除配件，可重复 |

支持的指标：`recoil-continuous-horizontal`、`recoil-continuous-vertical`、`recoil-single-horizontal`、`recoil-single-vertical`、`recoil-fire-stability`、面板属性 id `10001`、`10005`、`10006`、`10007`、`10008`，以及 `handling-*`、`stability-*`、`hipfire-*` 细分指标（完整列表见 `solver/src/metrics.rs` 的 `DETAIL_METRICS`）。

请求 JSON 的字段定义见 `solver/src/lib.rs` 的 `SolveRequest`，核心字段：

```json
{
  "weaponPayload": { "...": "站点解密后的完整 payload" },
  "weaponId": "18010000049",
  "mode": "operations",
  "laserEnabled": true,
  "bipodDeployed": false,
  "prices": { "18010000049": 93765, "partId": 12345 },
  "lockedPathKeys": {},
  "lockedTuningKeys": {},
  "excludedPartIds": [],
  "constraints": [
    { "id": "recoil-continuous-vertical", "max": 0.6 },
    { "id": "10005", "min": 50 }
  ],
  "maxEvaluations": 5000000,
  "maxLeaves": 2000000,
  "maxResults": 10,
  "deadlineMs": 5000,
  "firstSolution": false
}
```

### WASM

```bash
node solver/js/smoke.mjs \
  target/wasm32-unknown-unknown/release/gunsmith_mincost.wasm \
  weapon.json request.json
```

## 油猴脚本

1. 用 Tampermonkey 新建脚本，粘贴 `userscript/dist/gunsmith-mincost.user.js`。
2. 打开 moligod 改枪页，站点“生成智能配件方案”按钮下方会出现“最低开销”按钮和设置项。
3. 在站点面板设置至少一个硬阈值，再点“最低开销”。

设置：

- 完整模式：开启时完整搜索并证明最优；关闭时只找第一个可行解。
- 市场价：使用当前市场价或系统价。
- 缺价排除：缺价配件不参与求解；关闭时按缺失价格处理。
- 时限：留空跟随站点“搜索”时长；填 1-300 覆盖。

脚本在 `document-start` 包装页面的 `Worker`，只拦截 optimizer worker 的请求，从中取得已解密的 payload、价格、锁定项和当前装配。数据不会上传到任何地方。

## 测试

```bash
cargo test
CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release
CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release -- --ignored   # 慢回归
node --test solver/js/*.test.mjs userscript/test/*.test.mjs
```

`Cargo.toml` 的 release profile 设置了 `panic = "abort"`，release 模式下直接跑测试会链接失败，因此需要上面的环境变量。发布 WASM/userscript 时不要带该变量。

除常规测试外，仓库提供独立于搜索剪枝的暴力枚举器 `bf`，用于验证“没有更便宜的可行解”：

```bash
cargo build --release
target/release/bf spec.json
node solver/js/brute-diff.mjs cases/*.json
```

`brute-diff.mjs` 会对每个 case 同时运行 `solve` 和 `bf`，比较总价、约束值与复算结果，出现 `DIFF` 时以非零状态退出。详见 `docs/gunsmith-testing.md`。

## 致谢

本项目的数值模型、站点数据结构与“智能配件方案”的分析起点都来自 moligod 的改枪页实现，没有它就没有可复刻的精确规则和可用的数据格式。求解器、WASM 封装、油猴脚本与测试工具是基于这些工作另外实现的。

## 说明

本项目与游戏发行商、站点运营方无关。所有武器数据、价格和配装都由浏览器页面在本地提供，脚本只在本机内存中处理，不上传数据。使用前请自行确认符合站点条款与所在地法规。

## 文档

- `docs/gunsmith-numerical-model.md`：站点数值模型，后坐力/精校/曲线公式
- `docs/gunsmith-smart-build.md`：站点“生成智能配件方案”的 worker 协议与 beam search 分析
- `docs/gunsmith-rust-solver.md`：求解器算法、复杂度与实测结果
- `docs/gunsmith-userscript.md`：脚本数据来源、接口约定、构建与真实页面验证
- `docs/gunsmith-testing.md`：测试分层、暴力枚举 oracle、批量差分与 fixture 说明
