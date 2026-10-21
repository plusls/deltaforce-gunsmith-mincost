## 关于数值系统

参考资料：
https://www.bilibili.com/video/BV1x26KYTEwk

### 后坐力

后坐力分为 

+ 全自动垂直后坐力倍率
+ 全自动水平后坐力倍率
+ 单点垂直后坐力倍率
+ 单点水平后坐力倍率
+ 开火稳定性

上述数值均为倍率，也就是不同配件之间的倍率会进行乘法运算, 面板的后坐力控制数值仅供参考 (个人理解无实际意义, 直接参考子属性即可)

### 操控速度

操控分为以下子属性

+ 开镜时间
+ 跑射延迟
+ 开镜移速

其中配件可能只影响某个子属性，例如幻影的 +8 操控，只影响开镜时间

#### 开镜时间

开镜时间和操控速度的数值直接相关，讲人话就是，两把不同的枪，只要操控数值相同那么开镜时间就相同（整体是对的，但是幻影垂直握把除外）

变化趋势：

操控越大，开镜时间越短。操控越大，开镜时间降低的趋势越小（斜率变小，存在边际效应）

#### 跑射延迟

由子属性变化值影响




## 文档与工具

- `docs/gunsmith-numerical-model.md`: 站点数值模型, 后坐力/精校/曲线公式
- `docs/gunsmith-smart-build.md`: 站点"生成智能配件方案"的 worker 协议与 beam search 分析
- `docs/gunsmith-rust-solver.md`: 约束下最低总价求解器 (Rust + WASM), 算法与实测结果
- `docs/gunsmith-userscript.md`: 油猴脚本的数据来源, 接口约定与站点集成设计

```bash
cargo build --release
cargo build --release --target wasm32-unknown-unknown -p delta-weapon-solver
node solver/js/smoke.mjs target/wasm32-unknown-unknown/release/delta_weapon_solver.wasm weapon.json request.json
```
