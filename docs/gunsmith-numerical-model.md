# moligod 改枪模拟器数值模型笔记

记录对象: https://moligod.com/gunsmith/
数据版本: 7e60debf7b978ebec618a142
分析时间: 2026-10-21
相关文档: [gunsmith-rust-solver.md](./gunsmith-rust-solver.md)

## 0. TL;DR

后坐力有两条通道:

1. 分数通道: 面板写 `后坐力控制 +8`, 配件给的是整数主属性分。同类配件的分先相加, 再代入每把枪自带的分段曲线, 由曲线给出实际倍率。有边际效应。
2. 倍率通道: 部分配件直接给 `horizontal / vertical / gunkickSpringMultC`, 面板上的 +N 通常不进入曲线。

组合规则:

| 组合 | 规则 |
|---|---|
| 分数 + 分数 | 先加, 再查一次曲线 |
| 水平/垂直倍率 A + A | 括号内加算 |
| 弹簧倍率 K + K | 乘算 |
| Mult_C + Mult_C | 乘算 |
| 精校百分比 + A | 括号内加算 |
| 精校百分比 + 精校百分比 | 同名相加 |

## 1. 数据获取与解密

接口:

```
GET /api/gunsmith/manifest
GET /api/gunsmith/versions/{dataVersion}/weapons/{weaponId}
GET /api/gunsmith/weapons/{weaponId}/prices?version=...
GET /api/gunsmith/weapons/prices?version=...
```

二进制格式:

```text
byte[0]      = 1
byte[1..13]  = IV, 12 字节
byte[13..]   = AES-GCM 密文
key          = SHA-256(响应头 x-gunsmith-key 的 ASCII)
AAD          = gunsmith-manifest-v1 / gunsmith-weapon-v1 / gunsmith-price-v1
明文          = gzip 后的 JSON
```

weapon payload 主要字段:

```text
parts                         id -> 配件完整数据
slotsByHost                   hostPartId -> 该配件提供的子槽位
defaultPresetsByWeapon        默认装配 + 默认精校 tunePairs
virtualRulesByWeapon          socketId + 条件 -> 自动填入的虚拟件
candidateOverridesByWeapon    隐藏候选
damageReplacementsByPartId    伤害档案替换
weaponStatsById               五维主属性 base/display/byMode
weaponNumericProfilesById     曲线, 弹道, 伤害, 弹药, 原始数值表
```

## 2. 配件字段与修饰符

```text
mainAttributeModifiersByMode  主属性修改, target = WeaponMainAttribute.MainAttrValues.N
modifiersByMode               显示属性修改, target = DisplayAttrValues.N
uiEffectsByMode               说明文案, 带 conditionId, 不参与计算
rawModifiersByMode            引擎层修改, GRecoil_H/V, GGunkickSpring, GAiming_ADSTime, ...
runtimeModifiersByMode        运行时修改子集, 只用于换弹和射速
rawRecoilMultipliersByMode    {horizontal, vertical, gunkickSpringMultC}
rawGunSwayBeHitScaleMultAByMode
aimRecoilProfilesByMode       可选, 替换弹道序列
tuningOptions                 精校定义
tuningSocketAvailability      精校槽位可用性覆盖
isVirtual                     虚拟件
extraOccupySocketIds          额外占用 socket
```

修饰符类型:

```text
Addend   x += delta
Mult_A   x *= (1 + param1)
Mult_C   x *= param1
Initial  x  = param1
```

约定:

- 只有 `conditionId == null` 的 Addend 进入主属性聚合。
- 多个 Mult_A 先把 param1 相加, 再一次性乘。
- 多个 Mult_C 连乘。

## 3. 装配树与槽位

槽位不是按配件 type 自动匹配, 而是显式候选表:

```text
slot = {
  id, label, name, type,
  candidateIds,
  virtualCandidateIds,
  defaultHidden,
  hideForPlayer,
  allCandidatesVirtual
}
```

解析优先级:

1. 用户选择, 必须在 candidateIds 中
2. 默认预设里的非虚拟件
3. virtualRules 命中的虚拟件
4. 空

候选过滤:

```text
part 存在
&& !part.isVirtual
&& !part.hidden
&& 不在 hide_candidate override 中
```

隐藏与占位:

- `allCandidatesVirtual = true`: 该槽位不显示为可选槽, 由虚拟件自动填充
- `defaultHidden` / `hideForPlayer`: 不显示
- 即使不显示, 选中件的子槽位仍会递归
- `extraOccupySocketIds` 声明额外占用, 按 `hostPathKey/socket:id` 去重, 先占先得

RM277 例子:

```text
根槽: 后握把(1) 枪管(2) 护木(4) 弹匣(5) 瞄准镜(11)
      枪托垫(28) 托腮板(29) 侧瞄具(31)

护木槽 4 的候选全是虚拟件:
  barrel 13020000596 -> 虚拟护木 13050000398
  barrel 13020000602 -> 虚拟护木 13050000404
  barrel 13020000601 -> 虚拟护木 13050000405

虚拟护木 13050000404 再提供:
  前握把(8) 上导轨(17) 左导轨(19) 右导轨(20) 左贴片(34) 右贴片(35)

barrel 13020000602 提供枪口槽(6), 16 个实体候选 + 1 个虚拟枪口
弹匣 13120000263 提供弹匣座槽
瞄准镜 13110000051 提供上副瞄具槽(40), 默认虚拟 13110000060
```

## 4. 符号定义

### 4.0 符号速查

游戏内没有这些字母, 它们只是笔记里为了写公式起的名字。对照:

后坐力:

| 符号 | 游戏内概念 | 数据来源 | 含义 |
|---|---|---|---|
| `D` | 后坐力控制这一栏背后的分数部分 | `mainAttributeModifiersByMode[10005]` 的 Addend 之和 | 曲线横坐标, 不是最终倍率 |
| `Cr(D)` | 后坐力控制的实际效果 | `recoilControlCurve` | 把 D 换成倍率, D 越高倍率越低, 且边际递减 |
| `A_H` | 水平后坐力 -24% / +16% 这类词条 | `rawRecoilMultipliers.horizontal` | 带符号的直接倍率修正 |
| `A_V` | 垂直后坐力 -24% / +12% 这类词条 | `rawRecoilMultipliers.vertical` | 同上 |
| `K` | 开火时武器稳定性 -32% 这类词条 | `rawRecoilMultipliers.gunkickSpringMultC` | 逐件乘算 |
| `C_H` | UI 上不单独列出 | `rawModifiersByMode` 的 Mult_C, target `GRecoil_H` | 硬乘数, 脚架展开时常见 |
| `C_V` | UI 上不单独列出 | 同上, target `GRecoil_V` | 硬乘数 |
| `C_K` | 开火时武器稳定性里的硬乘数部分 | 同上, target `GGunkickSpring` | 硬乘数 |
| `T_H` | 精校 "额外后坐力控制 +4%" | tuning runtime `GRecoil_H` | 写的是"控制", 所以符号反过来 |
| `T_V` | 精校 "额外后坐力控制 +4%" | tuning runtime `GRecoil_V` | 同上 |
| `T_K` | 精校 "开火时武器稳定性 +x%" | tuning runtime `GGunkickSpring` | 同上 |
| `F_H` / `F_V` | 配件页显示的全自动水平/垂直 `0.59x` | 上面几项合并 | 最终水平/垂直后坐力倍率 |
| `S` | 配件页显示的开火稳定性 `0.8x` | `Gk` + `K` + `T_K` + `C_K` | 最终开火稳定性倍率 |
| `Gk(D)` | 开火时武器稳定性的曲线部分 | `gunkickCurve` | 分数越高越稳, 与后坐力曲线不是同一条 |

操控、稳定性、腰射:

| 符号 | 游戏内概念 | 数据来源 | 含义 |
|---|---|---|---|
| `HandScore` | 操控速度那一栏的总分 | base 10006 + `mainAttributeModifiersByMode[10006]` | ADS 曲线用绝对分数 |
| `dHand` | 操控分数相对基准的变化 | `HandScore - baselineHandlingScore` | 跑射延迟/举镜移速曲线用它 |
| `T_ads` | 精校 "开镜速度 +x%" | 同名精校效果求和 | 显示层倍率 `1/(1+x)` |
| `dS` | 稳定性那一栏的分数变化 | `modifiersByMode[10007]` | 呼吸/镜头稳定性曲线的横坐标 |
| `dHip` | 腰射那一栏的分数变化 | `modifiersByMode[10008]` | 腰射散布曲线的横坐标 |
| `M` | 腰射散布倍率 | `hipSpreadCurve(dHip)` | 乘在散布面积上 |
| `E[name]` | 精校条目里的 `%xxx +y%` | 各精校 `curve_points` 求和 | 同名效果跨配件相加 |

注意命名冲突:

- `A_H` / `T_H` / `C_H` / `F_H` 里的 `H` 是 Horizontal (水平)
- `HandScore` / `dHand` 里的 Hand 是 Handling (操控)

面板上看到的 `+N` 和 `D` 不是同一个量。只有 `mainAttributeModifiersByMode[10005]` 的 Addend 进入 `D`, 倍率通道配件常常只加面板分。

只在子属性计算里使用的部件集合:

```text
P = 可见槽位选中的实体件 + 内部虚拟件
```

面板只使用可见槽位选中的实体件。

每个部件 p:

```text
addMain(p, id) = sum(mainAttributeModifiersByMode[mode][id].Addend.delta)
addDisp(p, id) = sum(modifiersByMode[mode][id].Addend.delta)
```

后坐力聚合量:

```text
D   = sum_p addMain(p, 10005)          分数输入
A_H = sum_p rawRecoil.horizontal       水平直接倍率, 加算
A_V = sum_p rawRecoil.vertical         垂直直接倍率, 加算
K   = product_p (1 + gunkickSpringMultC_p)    弹簧倍率, 乘算
C_H = product_p product(Mult_C target = GRecoil_H)
C_V = product_p product(Mult_C target = GRecoil_V)
C_K = product_p product(Mult_C target = GGunkickSpring)
T_H = tuning runtime GRecoil_H         通常是 -百分比/100
T_V = tuning runtime GRecoil_V
T_K = tuning runtime GGunkickSpring
```

曲线函数:

```text
Cr(d) = recoilControlCurve(d)
Gk(d) = gunkickCurve(d)
```

最终后坐力倍率:

```text
F_H = Cr(D) * (1 + A_H + T_H) * C_H
F_V = Cr(D) * (1 + A_V + T_V) * C_V
S   = Gk(D) / Gk(0) * (1 + T_K) * K * C_K
```

`F_H` / `F_V` 是水平/垂直弹道 scale 的总倍率。
`S` 是开火时武器稳定性倍率。

重要: 分数通道只查一次曲线, 倍率通道整体乘在曲线结果上。

### 4.1 各项含义与符号约定

`1 + A_H` 不是 "在基础后坐力上再加后坐力"。
`A_H` 是相对基准的带符号修正, `1` 代表 100% 基准倍率。

| 符号 | 数据来源 | 含义 | 中性值 |
|---|---|---|---|
| `Cr(D)` | `recoilControlCurve(D)` | 分数通道倍率 | `1` |
| `A_H` | `sum(rawRecoilMultipliers.horizontal)` | 直接倍率修正, 负数减后坐力 | `0` |
| `T_H` | `-sum(%额外后坐力控制)/100` | 精校修正, 正收益转负数 | `0` |
| `C_H` | `product(Mult_C target = GRecoil_H)` | 硬乘数 | `1` |

所以:

```text
无配件:   F = 1 * (1 + 0 + 0)   * 1     = 1.00
装补偿器: F = 1 * (1 - 0.24 + 0) * 1    = 0.76
精校 +18%: F = 1 * (1 - 0.24 - 0.18) * 1 = 0.58
正收益配件: F = 1 * (1 + 0.16) * 1      = 1.16  -> 后坐力变大
展开脚架:  F = 1 * 1 * 0.5              = 0.50  -> Mult_C 硬减半
```

`A_H` 可正可负:

- `-0.24` 表示水平后坐力乘 `0.76`
- `+0.16` 表示水平后坐力乘 `1.16`
- 正数通常出现在有 tradeoff 的配件上, 比如共振三代握把的 `H +16%`, 折叠握把的 `V +12%`

`T_H` 的符号是反的, 因为 `%额外后坐力控制` 是 "控制" 而不是 "后坐力":

- 精校给 `+18%` 额外后坐力控制 -> `T_H = -0.18`
- 精校给 `-4%` 额外后坐力控制 -> `T_H = +0.04`

`C_H` 是真正的硬乘数, 来自 `rawModifiersByMode` 里 `modifierType = Mult_C` 且 `targetPath = GRecoil_H` 的条目。
大多数配件没有这种条目, 所以 `C_H = 1`。
例如虚拟两脚架 `13170000015` 有 `GRecoil_H = 0.5`, 展开脚架后直接把水平后坐力再乘 0.5。

### 4.2 为什么 A 是加算

数据里 `rawRecoilMultipliers.horizontal` 的语义是 `Mult_A`:

```text
x *= (1 + param1)
```

多个 Mult_A 的约定是先把 param1 相加, 再乘一次:

```text
两个 -0.24 / -0.16:
1 + (-0.24) + (-0.16) = 0.60
```

如果数据给的是 `Mult_C`, 才会逐项相乘:

```text
0.76 * 0.84 = 0.6384
```

所以 `1 + A_H` 里的 `1` 只是把 "变化量" 转成 "倍率":

```text
变化 -24%  -> 倍率 1 - 0.24 = 0.76
变化 +16%  -> 倍率 1 + 0.16 = 1.16
```

### 4.3 Mult_C 是什么

`C_H` (也就是先前对话里的 `MultC_h`, 全文统一写作 `C_H`) 是第二条硬倍率通道, 来自原始修饰符表:

```text
C_H = product(
  rawModifiersByMode[mode]
    .filter(m => m.modifierType == "Mult_C" && m.targetPath == "GRecoil_H")
    .map(m => m.param1)
)
```

与 `A_H` 的区别:

- `A_H` 来自预计算字段 `rawRecoilMultipliersByMode.horizontal`, 语义是 `Mult_A`, 在括号内加算
- `C_H` 来自 `rawModifiersByMode`, 语义是 `Mult_C`, 独立连乘
- 大多数配件没有这个条目, 所以默认 `C_H = 1`
- 脚架展开/收起这类状态效果常用 `Mult_C` 表达

RM277 实例:

| 配件 | 目标 | Mult_C | 生效条件 |
|---|---|---:|---|
| 伸缩脚架握把 | GRecoil_H / GRecoil_V / GGunkickSpring | 0.5 | 展开脚架 |
| 实用两脚架 | GRecoil_H / GRecoil_V / GGunkickSpring | 0.5 | 展开脚架 |
| 虚拟两脚架 13170000015 | GRecoil_H / GRecoil_V / GGunkickSpring | 0.5 | 展开脚架 |
| 死寂消音器 | GGunkickSpring | 0.68 | 始终 |
| 薪火螺旋消焰器 | GGunkickSpring | 0.92 | 始终 |
| CR棱镜阻手器 | GGunkickSpring | 0.88 | 始终 |
| 微型瞄具增高架 | GGunkickSpring | 1.04 | 始终 |

脚架收起时, 这些 `Mult_C` 会被过滤, 等效于 `C_H = 1`。
脚架展开后, `C_H = 0.5`:

```text
F_H = Cr(D) * (1 + A_H + T_H) * 0.5
```

防重复计算:

同一份 -32% 效果在数据里存了两份:

| 字段 | 值 | 语义 |
|---|---:|---|
| `rawModifiersByMode` 里 `Mult_C`, target `GGunkickSpring` | 0.68 | 引擎原始修饰符 |
| `rawRecoilMultipliersByMode.gunkickSpringMultC` | -0.32 | 预计算摘要 |

两者描述的是同一个效果。如果直接套公式:

```text
S = ... * (1 - 0.32) * 0.68 = ... * 0.68 * 0.68 = 0.4624
```

会变成 -53.76%, 但游戏内该配件写的是 -32%。所以站点按配件只取一条:

```text
if part 有 GGunkickSpring 的 Mult_C:
    springFactor = Mult_C 的 param1          # 0.68
    # 跳过该配件的 gunkickSpringMultC
else:
    springFactor = 1 + gunkickSpringMultC    # 0.68
```

多个配件之间仍然连乘。

H/V 轴没有这个互斥规则, 因为两条通道本来就是独立的:

- `A_H` 来自摘要 `rawRecoilMultipliers.horizontal`
- `C_H` 来自原始表 `Mult_C GRecoil_H`

如果数据里同时给了两者, 两者都会生效。
现有数据里 H/V 配件通常只占一条: 补偿器/握把走 `A_H`, 脚架走 `C_H`。


## 5. 五维主属性面板

基础值来自 `weaponStatsById[id].byMode[mode].display`。

### 5.1 后坐力/操控/稳定性/腰射

```text
total = base + sum(addDisp(p, id))
delta = total - base
```

数据来源:

- 面板 delta: 统一用 `modifiersByMode`
- 后坐力曲线输入 10005: 用 `mainAttributeModifiersByMode`
- 操控曲线输入 10006: 用 `mainAttributeModifiersByMode`
- 稳定性曲线输入 10007: 用 `modifiersByMode`
- 腰射曲线输入 10008: 用 `modifiersByMode`

面板只统计可见槽位选中的实体件, 不统计内部虚拟件。
子属性计算会把内部虚拟件算进去。

### 5.2 有效射程 10001

```text
total = round(((Initial ?? base) + sumAddend) * (1 + sumMult_A))
delta = total - base
```

RM277: base 55, 枪管 Mult_A 0.3 -> `round(55 * 1.3) = 72`, 面板 `72(+17)`。

### 5.3 激光与去重

带 `uiEffects` 条件 `10038` (开启激光) 的配件:

- laser off: 只计 con 修饰符
- laser on: con + pro 总和最大的那一个镭射件的 pro

镭射件同时有 -4 和 +16 两个 Addend, 靠这个规则在开/关之间切换。

## 6. 后坐力规范公式

### 6.1 公式

```text
D     = sum_p addMain(p, 10005)
Cr    = recoilControlCurve(D)
A_H   = sum_p horizontal_p
A_V   = sum_p vertical_p
T_H   = -sum(%额外后坐力控制) / 100
T_V   = -sum(%额外后坐力控制) / 100
F_H   = Cr * (1 + A_H + T_H) * product(Mult_C(GRecoil_H))
F_V   = Cr * (1 + A_V + T_V) * product(Mult_C(GRecoil_V))

G     = gunkickCurve(D) / gunkickCurve(0)
T_K   = -sum(%开火时武器稳定性) / 100
S     = G * (1 + T_K) * product(1 + gunkickSpringMultC) * product(Mult_C(GGunkickSpring))
```

展示层:

```text
全自动水平比例 = F_H * (baseContHS / profileContHS)
全自动垂直比例 = F_V * (baseContVS / profileContVS)
单点水平比例   = F_H * (baseSingleHS / profileSingleHS)
单点垂直比例   = F_V * (baseSingleVS / profileSingleVS)
开火稳定性     = S
底层水平       = baseContHS * 全自动水平比例
底层垂直       = baseContVS * 全自动垂直比例
```

没有 `aimRecoilProfile` 时, `profileScale == baseScale`, 比例就等于 `F_H` / `F_V`。
RM277 属于这种情况。

### 6.2 RM277 曲线

```text
(-15, 1.5)  Linear
(  0, 1.0)  Cubic Hermite, LeaveTangent  -0.021162324
( 55, 0.4)  Cubic Hermite, ArriveTangent -0.009876262
```

| D | Cr(D) | 每 +1 分边际收益 |
|---:|---:|---:|
| 0 | 1.0000 | 0.0208 |
| 1 | 0.9792 | 0.0201 |
| 2 | 0.9591 | 0.0194 |
| 5 | 0.9027 | 0.0182 |
| 8 | 0.8518 | - |
| 10 | 0.8207 | 0.0153 |
| 14 | 0.7648 | - |
| 15 | 0.7519 | 0.0128 |
| 17 | 0.7276 | 0.0120 |
| 20 | 0.6940 | 0.0108 |
| 25 | 0.6446 | 0.0093 |
| 30 | 0.6015 | 0.0082 |
| 40 | 0.5249 | - |
| 55 | 0.4000 | - |

负数方向更陡: 0 到 -15 是线性, 每点 0.0333。

### 6.3 分数与倍率是两条通道

RM277 实测分类:

| 配件 | 面板 10005 | 曲线 main 10005 | H | V | K |
|---|---:|---:|---:|---:|---:|
| RM277鲸鲨枪管组合 | +8 | +8 | 0 | 0 | 0 |
| AR MOE后握把 | +3 | +3 | 0 | 0 | 0 |
| RM277托垫 | +4 | +4 | 0 | 0 | 0 |
| RM277托腮板 | +2 | +2 | 0 | 0 | 0 |
| 堡垒水平补偿器 | +9 | 0 | -0.24 | 0 | 0 |
| 沙暴垂直补偿器 | +9 | 0 | 0 | -0.24 | 0 |
| 死寂消音器 | +8 | 0 | 0 | 0 | -0.32 |
| RK-0前握把 | +6 | 0 | 0 | -0.16 | 0 |
| 密令斜角握把 | +9 | 0 | -0.24 | 0 | 0 |
| 共振人体工程握把 | +9 | 0 | 0 | -0.24 | 0 |
| X25U斜侧战斗握把 | +10 | 0 | -0.20 | -0.20 | 0 |
| K1精英斜角握把 | +6 | 0 | -0.16 | 0 | 0 |
| 共振三代握把 | +4 | 0 | +0.16 | -0.32 | 0 |
| 伸缩脚架握把 | 0 | 0 | 0 | 0 | -0.50 |
| 实用两脚架 | 0 | 0 | 0 | 0 | -0.50 |
| 折叠握把 | -3 | 0 | 0 | +0.12 | 0 |
| 微型瞄具增高架 | -1 | 0 | 0 | 0 | +0.04 |

倍率通道配件的面板 +N 通常不进入曲线。
因此不能直接用面板 +N 比较两个配件的实际后坐力收益。

## 7. 算例: 曲线 + 直接倍率

### 7.1 例 1: 一条水平倍率

装配:

| 配件 | 面板 10005 | 曲线 main 10005 | 直接倍率 |
|---|---:|---:|---:|
| RM277鲸鲨枪管组合 | +8 | +8 | - |
| RM277托垫 | +4 | +4 | - |
| RM277托腮板 | +2 | +2 | - |
| 堡垒水平补偿器 | +9 | 0 | H -24% |

第一步, 面板:

```text
display delta = 8 + 4 + 2 + 9 = 23
后坐力控制 = 48 + 23 = 71(+23)
```

第二步, 曲线输入:

```text
D = 8 + 4 + 2 + 0 = 14
Cr = Cr(14) = 0.7648
```

面板 +23, 曲线输入只有 +14。补偿器的 +9 是展示分。

第三步, 直接倍率括号:

```text
H 括号 = 1 - 0.24 = 0.76
V 括号 = 1
```

第四步, 合并:

```text
F_H = 0.7648 * 0.76 = 0.5812
F_V = 0.7648 * 1.00 = 0.7648
```

第五步, 页面显示:

```text
全自动水平 = 0.58x, 底层 = 1.5 * 0.5812 = 0.87x
全自动垂直 = 0.76x, 底层 = 1.7 * 0.7648 = 1.30x
开火稳定性 = Gk(14) / Gk(0) = 0.895 -> 0.90x
```

对照: 拿掉堡垒补偿器后, 面板从 +23 变成 +14, 但曲线输入仍然是 14。
水平从 0.58x 变成 0.76x, 全部变化来自 -24% 括号。

### 7.2 例 2: 两条倍率 + 精校

装配:

| 配件 | 面板 10005 | 曲线 main 10005 | 直接倍率 | 精校 |
|---|---:|---:|---:|---|
| RM277鲸鲨枪管组合 | +8 | +8 | - | - |
| RM277托垫 | +4 | +4 | - | - |
| RM277托腮板 | +2 | +2 | - | - |
| 共振三代握把 | +4 | 0 | H +16%, V -32% | 配重 +20 -> +4% 额外后坐力控制 |
| 堡垒水平补偿器 | +9 | 0 | H -24% | - |

第一步, 面板:

```text
display delta = 8 + 4 + 2 + 4 + 9 = 27
后坐力控制 = 48 + 27 = 75(+27)
```

第二步, 曲线输入:

```text
D = 8 + 4 + 2 = 14
Cr = 0.7648
```

第三步, 直接倍率括号:

```text
H 括号 = 1 + (-0.24) + (+0.16) - 0.04 = 0.88
V 括号 = 1 + (-0.32) - 0.04 = 0.64
```

精校 +4% 转成运行时 -0.04, 与两个配件倍率加在同一个括号里。

第四步, 合并:

```text
F_H = 0.7648 * 0.88 = 0.6730 -> 0.67x, 底层 1.5 * 0.6730 = 1.01x
F_V = 0.7648 * 0.64 = 0.4894 -> 0.49x, 底层 1.7 * 0.4894 = 0.83x
开火稳定性 = 0.90x
```

如果精校和倍率是乘算, 水平会是 `0.7648 * 1.16 * 0.76 * 0.96`。
实际是 `0.7648 * (1 - 0.24 + 0.16 - 0.04)`。

### 7.3 例 3: K 是乘算

装配:

| 配件 | 面板 10005 | 曲线 main 10005 | K |
|---|---:|---:|---:|
| 死寂消音器 | +8 | 0 | -32% |
| 伸缩脚架握把 | 0 | 0 | -50% |

计算:

```text
面板 = 48 + 8 = 56(+8)
D = 0
Cr = Cr(0) = 1
F_H = 1.00x
F_V = 1.00x
S = 1 * (1 - 0.32) * (1 - 0.50) = 0.68 * 0.50 = 0.34
开火稳定性 = 0.34x
```

如果 K 是加算, 会得到 `1 - 0.32 - 0.50 = 0.18`。
站点给 0.34, 所以 K 是乘算。

注: 这两个配件的数据里同时有 `gunkickSpringMultC` 和 `Mult_C GGunkickSpring`。
按 4.3 的去重规则实际走 `C_K`, 数值相同: `0.68 * 0.50 = 0.34`。

### 7.4 组合规则验证

| 情况 | 站点规则 | 数值 |
|---|---|---|
| 两个分数 8 + 9 | 先加, 查一次曲线 | Cr(17) = 0.7276 |
| 若按逐件乘 | Cr(8) * Cr(9) | 0.8518 * 0.8360 = 0.7121 |
| 两个 A -0.24, -0.16 | 括号内加算 | 0.60 |
| 若按逐件乘 | 0.76 * 0.84 | 0.6384 |
| 两个 K -0.32, -0.50 | 乘算 | 0.34 |
| 若按加算 | 1 - 0.32 - 0.50 | 0.18 |
| 三个精校 +6% | 同名相加 | +18%, 括号 -0.18 |

## 8. 操控规范公式

```text
HandScore = baseDisplay(10006) + sum_p addMain(p, 10006)
dHand     = HandScore - baselineHandlingScore

ads      = adsTimeCurve(HandScore) * (1 + sum(Mult_A GAiming_ADSTime))
sprint   = baseSprintToFireSpeed * sprintToFireCurve(dHand)
aimMove  = baseAimMoveSpeed
         + StandRunSpeed * (adsStrafeSpeedCurve(dHand)
                          + sum(Addend GMovement_ADSSpeed)) / 100

baseAimMoveSpeed = StandRunSpeed * (1 + StandAimSpeedFactor) / 100
```

显示层精校系数:

```text
T_ads = clamp(1 / (1 + sum(%开镜速度|%举镜瞄准速度|%举枪瞄准速度) / 100), 0.25, 2.5)
页面举镜时间 = ads * T_ads
页面跑射延迟 = sprint * T_ads
```

关键点:

- ADS 曲线用绝对分数 `HandScore`
- 跑射和举镜移速曲线用 `dHand`
- `T_ads` 同时作用于举镜时间和跑射延迟

换弹:

```text
ChangeClipTime / ChangeClipTimeWhenEmpty
受 runtimeModifiers: GRateOfFire Mult_A, 自身 Initial/Mult_A 影响
```

射速:

```text
rpm = 60 / FireInterval
runtimeModifiers GRateOfFire: Initial / Addend / Mult_A / Mult_C
变射速枪按弹匣容量查 variableFireRateCurves
```

收枪/切枪直接取 `EquipTime` / `UnEquipTime`。

## 9. 稳定性规范公式

```text
dS = sum_p addDisp(p, 10007)

breathRange = StandBreathFactor * breathFactorMultC * 0.4 * breathScaleCurve(dS)
              / max(0.1, 1 + %瞄准时呼吸稳定性 / 100)
breathHold  = 100 / (AttenSpeed * breathTimeCurve(dS))
              / max(0.1, 1 + %瞄准时呼吸稳定性 / 100)
fireCamera  = fireCameraStabilityCurve(dS)
moveCamera  = GS_Movement_CameraScale * movementCameraStabilityCurve(dS)
              / max(0.1, 1 + %移动时据枪稳定性 / 100)
holdBreath  = RecoilScale.X/Y * holdBreathRecoilCurve(dS)
hitSway     = GunSwayBeHitScale * (1 + sum(rawGunSwayBeHitScaleMultA))
```

## 10. 腰射、伤害、射速

腰射:

```text
dHip = sum_p addDisp(p, 10008)
M    = hipSpreadCurve(dHip)
ui   = LuaDisplaySpreadUIFactor / 1.25

standingStill = AreaSize * M * 80 * ui
maximum       = SpreadMax * M * 80 * ui
moving        = standingStill * LuaDisplaySpreadFactor
```

伤害:

```text
profile = damage_by_mode[mode].profile
如果有已装件的 replacement_profile, 最后一次替换生效

operations 才吃弹药:
  Damage      *= ammo.DamageRate
  ArmorDamage *= ammo.ArmorDamageRate
  穿甲等级      = ammo.penetrate_level

部位伤害 = floor(Damage * ammoRate * 部位倍率 * 弹药部位倍率)
```

warfare 不吃弹药倍率。

## 11. 精校规范公式

### 11.1 取值

key:

```text
slotPathKey \0 partId \0 tune_id
```

优先级:

1. 用户值
2. 默认预设 tunePairs
3. default_value

然后:

```text
v = clamp(value, min, max)
v = round((v - min) / step) * step + min    # step > 0 时
```

### 11.2 效果

精校值只是曲线横坐标:

```text
percent_effect = interpolate(curve_points, v)
```

同名效果跨配件求和:

```text
E[name] = sum_parts sum_effects percent_effect
```

示例:

- X25U `安装位置` 0..100, default 50, 曲线 `(0,-4) (50,0) (100,+4)`
  - 50 -> 0%
  - 100 -> +4%
  - 0 -> -4%
- 共振三代 `配重` -20..20
  - +20 -> `%额外后坐力控制 +4`, `%举镜瞄准速度 -4`
- 3 个配件各有 +6% 额外后坐力控制
  - 合计 +18%

### 11.3 精校百分比进入公式

因为符号约定, 正收益会变成负的运行时修正:

```text
T_H = -E["%额外后坐力控制"] / 100
T_V = -E["%额外后坐力控制"] / 100
T_K = -E["%开火时武器稳定性"] / 100
T_move = +E["%瞄准时移动速度"] / 100
```

注意: 每个精校效果只在其 tune 的 `runtime_targets` 包含目标时才计入。
多数后坐力配件的 tune 同时包含 `GRecoil_H` 和 `GRecoil_V`。

按名字匹配的部分:

| 名字 | 用途 |
|---|---|
| `%开镜速度` / `%举镜瞄准速度` / `%举枪瞄准速度` | 操控显示倍率 `1/(1+x)` |
| `%瞄准时呼吸稳定性` | 呼吸范围/时长除数 |
| `%移动时据枪稳定性` | 移动镜头稳定性除数 |
| `%枪口初速` | 当前页面不展示 |
| `mm眼距` / `倍放大倍率` | 仅数值展示 |

精校不改变右侧五维主属性面板。
精校曲线在现有数据里基本都是线性, 所以精校本身通常没有边际效应。
整体边际效应来自主属性曲线。

## 12. 特殊状态与坑

- `laserEnabled`: 影响镭射件 pro/con 计入
- `bipodDeployed`: 影响折叠件/脚架 Mult_C 是否计入
- 镭射判定: `uiEffect.conditionId == 10038` (开启激光镭指时)
- 脚架/折叠判定: `uiEffect.conditionId == 10047` (主动部署脚架时), 或虚拟件同时有 `GGunkick* + GRecoil_H + GRecoil_V + StandBreathFactor` 的 Mult_C
- 相关条件 ID: `10038` 开启激光, `10047` 主动部署脚架, `10076` 关闭激光, `10077` 脚架收起
- 同一修饰符按 `partId + target + type + param + ruleKey` 去重
- `rawModifiers` 的 `conditionId` 在多数聚合里没有过滤, 折叠语义靠上面的规则近似
- 精校 `runtime_targets` 用整个 tune 的 target 集合, 不按 `effect_index` 精确配对
- 面板显示时间普遍 `Math.round(toFixed(2))`, 毫秒是 `Math.floor(Math.fround(seconds) * 1000)`
- 倍率通道配件的面板 +N 不等于曲线输入
- 内部虚拟件的属性只影响子属性, 不影响五维面板
- `GGunkickRandom`, `GGunkickH/V`, `GRecoil_HShake` 等原始数据存在, 但当前模型不使用
- 弹簧 `gunkickSpringMultC` 与同配件的 `GGunkickSpring` Mult_C 互斥, 后者存在时跳过前者

## 13. 验证样例

RM277 + AR MOE 后握把 + 鲸鲨枪管 + HAMR + 托垫 + 托腮板:

```text
配件 Addend 合计: 10005 +17, 10006 -15, 10007 +12, 10008 -20
有效射程: 55 * 1.3 = 71.5 -> 72
面板: 65(+17) 35(-15) 71(+12) 24(-20) 72(+17)
后坐力: 0.72759 * 0.82 = 0.59662
全自动: 0.59x (底层 0.89 / 1.01)
开火稳定性: 0.8x
举镜时间: 479ms * 1/(1-0.06) = 510ms
```

以上均与页面显示一致。

## 14. 复现顺序

```text
1. 解密并读取 manifest + weapon payload
2. 按 slotsByHost / virtualRules / defaultPreset 解析装配树
3. 收集 visibleSlots.selectedPart + internalParts.part
4. 聚合:
   a. main 10005/10006 Addend -> recoil/handling 曲线输入
   b. display 10007/10008 Addend -> stability/hipfire 曲线输入
   c. rawRecoilMultipliers -> A/K
   d. rawModifiers Mult_C -> Mult_C 因子
   e. tuning curves -> 百分比和运行时修正
5. 用曲线计算子属性
6. 用 display Addend + 有效射程乘法计算五维面板
7. 用 damage profile + 弹药 + 部位倍率计算伤害
```
