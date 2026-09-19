# ADR-006 — 计账单位为整数 NanoUsd（定点金额，禁止 f64 累加）

- 状态：accepted
- 日期：2026-09-19
- 关联：spec §6（成本）/§7（计账口径）；DESIGN §5/§12.4；实现 `crates/router-core/src/cost.rs`

## 背景

router 的产出口径是"这条策略省了多少钱"，而这个数字要能**逐字节复算**：trace 里记下的成本必须能被
`router replay` 用同一份代码重算出完全相同的值。单请求成本量级是 1e-7 ~ 1e-3 USD，用 `f64` 累加在
百万级请求上会漂移，且 IEEE-754 的取整行为依赖运算顺序 —— 两个"实现等价"的路径会给出不同字节的报表，
gate 上的"净收益 > 0"因此会变成"取决于谁先算"。

同时 config 里的价格是人类可读的字符串（USD/1K，官方页多以 USD/1M 公布），**装载期**不可避免要接触
小数文本。问题不是"能不能用浮点"，而是"浮点能在哪一层停留"。

## 决策

1. **金额定点为整数纳美元（nano-USD，1e-9 USD）**：`NanoUsd(u64)`、单价 `Price(u64)` = nano-USD / 1K token。
   决定路径（选模、guard、quota、breakeven、计账、trace）**全程整数**。
2. **金额 → 纳美元的转换点唯一且只在装载期**：config 的 `USD/1K` 一次性整数化为 `Price((v * 1e9).round() as u64)`
   （DESIGN §12.5）；运行期不再出现任何小数，`v < 0` 或 `round` 后为 0 视为装载错误。
3. **禁止 `f64` 参与成本累加**：`router-core` 以 crate 级 `#![deny(clippy::float_arithmetic)]` 强制；
   唯一的豁免是 `Usage::cache_hit_rate()` —— 它是**派生指标**（不是金额路径），逐点标注 `allow`。
4. **取整点唯一**：每档 `tokens × price(每 1K)` 除以 1000 时向下取整（`saturating_div_1k`），聚合用
   整数（饱和）加法；除这一处外全程无取整。将来若引入按比例分摊，必须复用同一个取整点，不得新增第二个。
5. **溢出用饱和，不用 panic**：金额路径不得因异常输入 panic（`saturating_add` / `saturating_mul_pct`）；
   区间估算用 `u128` 中间量。
6. **小数只出现在最终格式化层**：报表/trace 的字符串化在边界处发生，内部类型不承载"近似值"。

## 理由

- 整数让 trace → 报表的复算是**同构**的：同一份 trace 两次回放必然逐字段相同（CONF-19 的断言基础）。
- 转换点唯一 ⇒ "price 少写一个 0" 这类错误的暴露位置只有一个（装载），而不是散落在决策路径里。
- 饱和而非 panic ⇒ 上游给的畸形 usage（例如 `cached > total`）降级为"数字偏大"而不是"进程崩"，与
  spec §8「fail-safe、不阻塞请求」一致（`uncached()` 同样用 `saturating_sub`）。

## 后果

- 五档价与峰谷、quota、breakeven 的边界用例全部可用整数锚点钉死（例：`gain×100 == sf×cost` 恰好相等
  必须判 `Stay(NotPaying)`），不再依赖浮点比较的容差。
- 上游 `usage` 缺失时不得"估算补数"：缺口记 `result.usage_missing`，成本按 0 计并显式标注（§7 口径）。
- 价格口径的权威仍在 config（`source` + TODO 标注），本 ADR 只定**表示**，不动价格来源纪律（AGENTS 硬约束 5）。
- 若将来接入按 token 计价的分层/阶梯价，必须显式建模为新的档位而不是在中间层引入浮点。
