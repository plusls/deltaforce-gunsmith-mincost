# 测试与验证

本仓库的求解器同时维护两条验证路径：常规测试负责回归，暴力枚举 oracle 负责证明“没有更便宜的可行解”。

## 1. 常规测试

```bash
cargo test
CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release
CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release -- --ignored
node --test solver/js/*.test.mjs userscript/test/*.test.mjs
```

- `cargo test`：单元测试与集成测试，覆盖数值模型、装配、搜索和站点适配。
- `--ignored`：真实的慢用例与调校前沿性能回归，默认不跑。
- release 下的环境变量是必须的：`Cargo.toml` 为发布 WASM 设置了 `panic = "abort"`，libtest 需要 unwind。构建发布产物时不要带这个变量。
- JS 测试覆盖 WASM 封装、站点适配、时限逻辑、worker 协议与打包。

## 2. 暴力枚举 oracle

`solver/src/bin/bf.rs` 是一个独立于搜索算法的枚举器：

- 按可见槽位递归枚举所有装配（含默认内部件、候选配件、锁定项）。
- 对每个装配枚举精校值，逐个用 `Evaluator` 完整评估。
- 统计可行解、最小总价、枚举数量与是否被上限截断。
- 默认还会把同一请求交给求解器，并复算求解器给出的解，输出 `recheckSolution`。

它不复用求解器的任何剪枝或前沿逻辑，因此适合发现漏解、次优解和数值不一致。

构建：

```bash
cargo build --release --bin bf
```

输入是一个 spec JSON：

```json
{
  "payload": { "...": "站点 payload" },
  "request": {
    "weaponId": "18010000049",
    "mode": "operations",
    "laserEnabled": true,
    "bipodDeployed": false,
    "prices": { "partId": 12345 },
    "constraints": [{ "id": "recoil-continuous-vertical", "max": 0.6 }]
  },
  "maxAssemblies": 2000000,
  "maxEvaluations": 20000000,
  "maxValuesPerTune": 128,
  "solution": { "selections": {}, "tuning": {} }
}
```

字段说明：

- `maxAssemblies` / `maxEvaluations`：枚举上限，触发时输出 `assembliesTruncated` / `evalsTruncated`。
- `maxValuesPerTune`：单个精校的枚举值上限；超过 `4 * maxValuesPerTune` 时该装配记为 `skippedTuningGroups`。
- `solution`：可选。显式指定要复算的解；缺省时复算求解器返回的最优解。
- `dump` + `evalSelections`：调试用，可导出指定装配的全部指标，或复算给定 selections/tuning。

运行：

```bash
target/release/bf spec.json
```

输出结构：

```json
{
  "brute": {
    "minTotalPrice": 1050,
    "feasibleCount": 2,
    "assemblies": 2,
    "tuningEvals": 129,
    "skippedTuningGroups": 0,
    "assembliesTruncated": false,
    "evalsTruncated": false,
    "witness": { "...": "最优解" }
  },
  "solve": { "ok": true, "optimal": true, "totalPrice": 1050 },
  "recheckSolution": { "feasible": true, "totalPrice": 1050 },
  "compare": null
}
```

`brute.minTotalPrice` 是独立枚举得到的最低价。若它与 `solve.totalPrice` 不同，或 `recheckSolution.feasible` 为 false，就是求解器缺陷。

`tests/brute_oracle_test.rs` 用两个已知用例（取整带边界、201 值精校网格）固定了这条路径。

## 3. 批量差分

`solver/js/brute-diff.mjs` 接受多个 case JSON（`payload` + `request`），对每个 case 分别运行 `solve` 和 `bf` 并比较结果：

```bash
cargo build --release
node solver/js/brute-diff.mjs cases/*.json
```

它会输出每个 case 的判定：

- `OK`：价格一致、解可复算且指标一致。
- `OK-no-solution`：双方都无解。
- `DIFF`：漏解、次优、不可复算或指标不一致。

任一 `DIFF` 时进程以非零状态退出，可直接用于 fuzz 脚本或 CI 的差分任务。

## 4. fixture

测试数据分两类：

- 真实 payload：`weapon-*.json.gz`、`tune_cap/*.json.gz`，来自站点 optimizer worker 的请求数据，压缩后入库。
- 合成 payload：`edge/`、`tuning_round2_boundary.json` 等，为特定边界手写的最小数据。

说明见 `solver/tests/fixtures/README.md`。
