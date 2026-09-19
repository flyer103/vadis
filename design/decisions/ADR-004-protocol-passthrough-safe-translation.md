# ADR-004 — native passthrough 优先；跨协议确定性翻译；v0.1 不做服务端状态

- 状态：accepted
- 日期：2026-09-19

## 背景

需求是支持 3 种协议（OpenAI chat completions / OpenAI responses / Anthropic messages）。同时"照顾
prompt cache"要求上游看到的前缀字节跨轮稳定。翻译层如果不做确定性约束，就会成为最大的缓存破坏源。
另外必须回答：是否需要支持服务端会话状态（`store:true` / `previous_response_id`）。

## 决策

1. **native passthrough 优先**：入站协议 == provider `wire_api` 时，只允许删除 router 自有字段，
   其余字节原样转发。
2. **跨协议翻译必须确定性**：映射器是 `(content, stable config)` 的纯函数，同一内容永远产出同一
   上游字节；有损点必须显式登记（spec §2 清单）并写 trace。
3. **v0.1 不做服务端状态**：入站 `store:true` 或非空 `previous_response_id` → **粘性路由到同一
   (provider, model)** 保真 + trace 打 `stateful_inbound` 标记；无法保证粘性时才 400。

## 证据（实测，2026-09-19）

对 codex CLI 0.137（`wire_api="responses"` → ZAI）做本地抓包（完整一轮，含一次工具调用）：

| 观测 | 值 |
|---|---|
| 请求体顶层字段 | `client_metadata, include, input, model, parallel_tool_calls, prompt_cache_key, reasoning, store, stream, tool_choice, tools` |
| `store` | `false`（两轮都是） |
| `previous_response_id` | 两轮都不存在 |
| `input` 长度 | 3 items（首轮）→ 6 items（工具调用后）→ **全量重发** |
| `prompt_cache_key` | 两轮相同（= session id），上游响应回显该字段 |
| 上游缓存 | `cached_tokens` 960/14409 → 14400/14520（99.2%） |

静态证据一致：`codex-rs/core/src/client.rs` 硬编码 `store: false` 且 HTTP 路径不设
`previous_response_id`（该字段只属于 WebSocket 增量传输与 remote compaction）；
`hermes/agent/transports/codex.py` 同样 `"store": False` + `prompt_cache_key=session_id`。

## 后果

- 服务端状态缺席的成本极低，而实现它的复杂度（session 表、失效、并发、跨 provider 语义差异）很高；
  这项复杂度被推迟到有实测需求为止（`stateful_inbound_rate` 指标持续监控）。
- `prompt_cache_key` 升为会话身份的一等来源，用于粘性表与 cache 账本键。
- 客户端全量重发 ⇒ 前缀缓存的命运完全由 router 的 transform 决定 ⇒ `prefix_continuity` 成为阻塞门。
- codex 若切到 WebSocket 传输，HTTP-only 的 v0.1 依赖其 fallback 到 HTTP（客户端已有
  `fallback_to_http`）；此事记录在案，作为未来观测项。
