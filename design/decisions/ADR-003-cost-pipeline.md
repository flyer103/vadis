# ADR-003 — 省成本 = 可逆、声明式、逐条计账的 transform 管线

- 状态：accepted
- 日期：2026-09-19

## 背景

"内置省 token"最容易变成一堆硬编码技巧，无法归因、无法回滚，且常常**与提示词缓存冲突**——压缩改写
早期上下文会让整段前缀缓存失效，省下的 30% 被重算的 re-prefill 吃掉。社区已有可复用的成熟形态：
rtk（Apache-2.0，Rust，声明式 TOML 过滤管线 + 内联测试 + 原文 tee/retrieve、fail-safe passthrough、
<10ms 开销）；caveman（其 proxy 为 BSL-1.1，仅作设计参考，不嵌入）。

## 决策

1. **省成本 = transform 管线**，每条 transform 是一个插件，必须：
   可逆（ADR-002）、内容确定（禁用轮次/时间/随机依赖）、逐条计账、失败回退原文。
2. **优先级固定**（一阶在前，实测支撑：同会话第二轮 `cached_tokens` 14400/14520 = 99.2%）：

   | 级 | 手段 | v0.1 |
   |---|---|---|
   | P0 | 前缀缓存保真（零改写、粘性、断点注入、cache 账本） | ✅ |
   | P1 | 输入侧载荷压缩（tool_result/日志/JSON/diff/搜索结果；原文 tee + retrieve） | ✅ |
   | P2 | 输出侧纪律（append-only 指令注入、max_tokens/stop、结构化输出约束） | ✅ |
   | P3 | provider 套利（命中计价、峰谷、配额套餐优先级、batch） | ✅ |
   | P4 | 去重/裁剪/摘要（改写早期内容，与缓存冲突最大） | 延后 |

3. **规则即数据**：压缩规则用 TOML 描述（形态参考 rtk：管线阶段、match_output、keep/strip_lines、
   truncate、head/tail、max_lines、on_empty），**每条规则必须内联测试**（input/expected），三级覆盖
   （项目 → 用户 → 内置）。新规则的准入 = 内联测试全绿 + 缓存回归通过。
4. **计账二元口径**：`verified`（上游 usage 实测差值，需对照回合）与 `inferred`（本地估算）。只有
   verified 能进 gate、能对外报数；报告必须声明口径、样本量、时间窗。
5. **许可纪律**：可复用 Apache-2.0/MIT 的实现思路与代码；BSL/非 OSI 许可（如 caveman runtime）只
   读设计不嵌入。

## 后果

- 每条 transform 都会带来"added tokens 换 saved tokens"的净收益问题；净收益为负的规则必须能被
  gate 拦下（这也是 verified 口径存在的意义）。
- 输入侧压缩**只作用于工具/环境载荷**，绝不改写用户意图：第三方实测（JetBrains 86 任务；Adobe
  CAVEWOMAN, arXiv:2606.24083）显示压缩人类 prompt 会让模型答得更长更差。
- P4 延后是刻意决定：任何改写历史的规则都要先有 `prefix_continuity` 度量与回退机制。
