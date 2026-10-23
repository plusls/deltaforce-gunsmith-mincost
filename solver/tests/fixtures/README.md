# Fixtures

测试数据按来源和格式分几类。新增 fixture 时请在这里补充一行，说明来源和用途。

## 真实 payload

- `weapon-18060000009.json.gz`
- `weapon-18070000010.json.gz`

站点 optimizer worker 请求里的 `data`，即解密后的完整武器 payload。`dataVersion` 为 `7e60debf7b978ebec618a142`，`generatedAt` 为 2026-09-30。gzip 只为压缩体积，读取用 `flate2::read::GzDecoder`。

`prices-*.json` 是同一批数据对应的价格快照：

- `prices-18060000009.json`：裸 map，`{ "partId": price }`，价格已取整。
- `prices-18070000010.json`：带元数据，`{ "dataVersion", "generated_at", "prices": { ... } }`。

## 真实请求

- `tune_cap/*.json.gz`：完整 case，`{ "payload": ..., "request": ... }`。用于慢回归与节点/叶子预算用例。
- `tune_cap/*.json`：只有 request，配合 `real_18010000037_4.json.gz` 的 payload 使用，覆盖跑射延迟、锁定精校与精校前沿。

这些用例真实、体量大，默认以 `#[ignore]` 跳过：

```bash
CARGO_PROFILE_RELEASE_PANIC=unwind cargo test --release -- --ignored
```

## 合成 payload

为特定边界手写的最小 payload：

- `edge/laser_non_pro_addend.json`：激光 pro/con 加成。
- `edge/narrow_notch.*`、`edge/narrow_spike.*`：窄可行带与尖峰曲线。
- `edge/negative_recoil_bound.json`、`edge/negative_recoil_max.json`：负值约束。
- `edge/struct_default_alternative.json`：结构槽默认件与备选件。
- `tuning_round2_boundary.json`：取整带开边界，同时被 `brute_oracle_test.rs` 使用。
- `rm277-stability-fire-camera.payload.json`：稳定性约束归约。
- `ak12-aim-recoil.payload.json`：瞄准后坐力倍率与慢回归。

合成 payload 只保留测试相关字段，不保证能被站点直接使用。

## 校验

- 求解器与暴力枚举的差分见 `docs/gunsmith-testing.md`。
- 修改 fixture 或数值模型后，至少跑一遍常规测试和 `brute_oracle_test.rs`。
