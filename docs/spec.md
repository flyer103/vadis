# Spec (WHAT) — router v0.1

口径：本文是**对外行为与配置契约**的单一真相。实现细节见 `design/DESIGN.md`；为什么这么做见
`design/decisions/`。

## 1. 目标与非目标

**是什么**：一个本地优先的多协议 LLM 网关。客户端（codex / hermes / claude code）把 base_url 指过来，
router 按配置把请求送到指定的 (provider, model)，在此过程中**不破坏上游前缀缓存**地省 token，并把
每次决定与每分钱记成可回放的 trace。

**v0.1 非目标**（明确的排除项，不要顺手加）：

| 不做 | 原因 / 后续路径 |
|---|---|
| 服务端会话状态（`store:true`、`previous_response_id`） | 实测客户端不使用（§7）；缺失时粘性路由保真 |
| 自动选模型 / 效果优化 | 显式指定为主；`auto` 留给插件槽（ADR-004） |
| 语义响应缓存、上下文摘要 | 与前缀缓存冲突面大，需先有实测账本（P4） |
| 多用户、多租户、多节点、DB | 单操作者本地进程；状态仅落盘 snapshot |
| bandit / MF / BT 等历史算法 | 作为实验资产，后期以 tier-A 插件形式回流 |
| `tee` 原文的取回通道（retrieve 端点） | 规则可声明 `tee`，但存储位置与取回通道**本版不实现**（§4） |

## 2. 协议契约

三种入站协议语义等价（同一 router 决定链），按协议镜像上游语义：

| 入站 | 端点 | 出站 native 条件 |
|---|---|---|
| OpenAI chat completions | `POST /v1/chat/completions` | provider `wire_api: chat` |
| OpenAI responses | `POST /v1/responses` | provider `wire_api: responses` |
| Anthropic messages | `POST /v1/messages` | provider `wire_api: anthropic` |

**出站选择规则**（3×3）：入站协议与 provider 的 `wire_api` 相同 → **native passthrough**（字节保真）；
不同 → **确定性翻译**（同一内容永远产出同一上游字节，保证前缀缓存稳定）。每个 provider 需在 config
声明其支持能力；翻译格必须显式标注有损点。

有损点清单（翻译时必须逐条处理，不是"尽力而为"）：

| 语义 | chat | responses | anthropic | 处理 |
|---|---|---|---|---|
| 系统指令 | `messages[0].role=system` | `input[0].role=developer`（实测 codex 不填顶层 `instructions`） | 顶层 `system` | 映射；保持位置稳定（在 prefix 最前） |
| 工具调用 | `tool_calls` + `role=tool` | `function_call` / `function_call_output` item | `tool_use` / `tool_result` block | 双向映射，保留 id 与顺序 |
| 推理内容 | `reasoning` 字段 | `reasoning` item（`include:["reasoning.encrypted_content"]`） | `thinking` block | 不透传即丢弃并记录 lossy 标记；**禁止重新编码** |
| 缓存断点 | 无 | `prompt_cache_key` | `cache_control` 断点 | 向 anthropic 上游翻译时按稳定规则注入断点（同内容 → 同位置） |
| usage | `usage.prompt/completion_tokens` | `usage.input_tokens_details.cached_tokens` 等 | `usage.input_tokens/cache_read_input_tokens` | 归一化到内部 `Usage`（§6） |

## 3. 选择语义

`model` 字段接受三种形式：

1. `provider/model` — 直接命中 roster 中的一条；
2. **别名** — config `aliases:` 中定义的名字（例：`coding-fast → deepseek/deepseek-v4-pro`）；
3. `auto` — v0.1 返回 `400` + 明确错误体（提示由插件接管）；结构上预留 `Selector` 槽。

显式指定时，router 仍执行 **Guard 阶段**（配额/成本上限/能力/`max_tokens`），guard 命中则按配置
策略处理（拒绝并说明原因，或降级到 `fallback` 链）。

## 4. 配置 schema（`config.example.yaml` 的契约）

```yaml
server:   { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m }
session:  { key_sources: ["prompt_cache_key", "header:session-id", "header:thread-id"], ttl: 12h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace:    { dir: "./state/traces", rollover: hourly }

providers:
  - name: deepseek
    base_url: https://api.deepseek.com/v1
    api_key_env: DEEPSEEK_API_KEY      # secrets 只从 env 读
    wire_api: chat                     # chat | responses | anthropic
    supports: [chat, responses]        # 可用于翻译的入站协议
    models:
      - id: deepseek-v4-pro
        context: 128k
        price:                         # 五档价，USD / 1K token
          input_miss: 0.00042
          input_hit: 0.000042
          cache_write: 0.0
          output: 0.00168
          peak: { multiplier: 1.0, windows: [] }
        source: "https://api-docs.deepseek.com/quick_start/pricing @2026-09-19"   # 必填，可追溯
    quota:                             # 订阅制套餐（coding plan 等），可选
      - { models: ["kimi-k3"], window: monthly, tokens: 100_000_000, reset_day: 1,
          over_quota: block }

aliases:  { coding-fast: deepseek/deepseek-v4-pro }

plugins:
  - id: cache-guard
    kind: builtin/cache_guard          # tier-A：编译进产品
    config: { strict_prefix: true }
  - id: tool-output-rules
    kind: builtin/transform_rules      # 规则即数据（TOML + 内联测试）
    config: { rules_file: ./rules/tool_output.toml, on_failure: passthrough }
  - id: judge-experiment
    kind: process                      # tier-B：进程外插件
    url: unix:///tmp/router-plugins/judge.sock
    inject: [cache_ledger, session_table]   # 依赖声明（coeffect）：未满足时停在加载等待
    isolate: false                     # 独立 realm：可与另一版本并存做 shadow 对比
    intercept: { sample: 0.05, shadow: true }
    disabled: true

fallback: [deepseek/deepseek-v4-pro, moonshot/kimi-k3]   # 有序 route 列表，全局粒度（§8）
```

配置变更语义（对齐 Cordis 的 keyed diff，见 ADR-002）：`config` 变更 → 交给插件自行 diff 后 reload
（不重建进程）；`disabled: true` → 卸载该 fiber 并完整回滚其 effect；`id`/`kind` 变更 → 重建该 entry。

### 4.1 `trace`（观测介质的落盘参数）

| 键 | 类型 / 取值 | 语义 |
|---|---|---|
| `dir` | 路径 | trace JSONL 的落盘目录。相对路径按**本 config 文件所在目录**解析（不是 CWD） |
| `rollover` | `hourly` | 滚动粒度；`hourly` → 文件名 `YYYY-MM-DDTHH.jsonl`（UTC，小时起始）。v0.1 只定义该值 |

- 路径进 config（便于把 state 放到别处）；**保留期不进 config**：v0.1 不做自动清理，文件只追加、由运维手工归档；
  将来要加 `retention` 属于新增键（不破坏既有配置）。
- 写失败**不阻塞请求**（§8）；trace 内容见 §6。

### 4.2 `fallback`（failover 链，§8）

| 键 | 类型 | 语义 |
|---|---|---|
| `fallback` | 有序 `provider/model` 列表 | **全局粒度**（v0.1 无按模型/按别名的链）；空列表 = 不 failover |

上游 5xx / 429 / 配额耗尽时，按序切到"下一个**尚未尝试**的 route"；切换会失去前缀缓存，trace 必须记
`failover_from` 与由此产生的 re-prefill 成本（§6「结果」）。链用尽 → `502 upstream_error`（上游尝试超时
→ `504 upstream_timeout`）。列表项必须是 roster 中存在的 route（别名不参与 fallback）。

### 4.3 `inject`（插件依赖声明）

`inject: [<service>…]` 声明该插件依赖的服务槽（Cordis 的 coeffect 声明，ADR-002/DESIGN §4）。
未满足时该 fiber 停在**加载等待**（不报错，也不影响其它插件），服务就绪后继续装载。
服务名是产品定义的类型化槽名（例：`cache_ledger`、`session_table`），不是任意字符串。

### 4.4 规则文件（`rules/*.toml`）与 `tee`

`builtin/transform_rules` 的规则文件格式见 `rules/tool_output.toml`（ADR-003 的声明式管线：过滤阶段、
`match_output`、行保留/剔除、截断、`on_empty`、内联测试）。

- 规则可声明 `tee = true`。命中且确有删行时，在输出**末尾追加一行**：
  `[router:tee sha256=<原始载荷 sha256 前 16 位 hex> lines_dropped=<n> bytes_original=<n>]`。
- **原文的存储位置与取回通道（retrieve）在 v0.1 未实现**——本版**不提供**取回端点、不定义落盘目录。
  trace 只记 `tee_id`（§6「transform」，未启用时为 `null`）。实现属于 P1 压缩方向（D3）之后的独立变更；
  在它落地前，不要把 `tee` 当作"可回取原文"的能力使用。

## 5. 接入前提（必做）

客户端必须绕开可能存在的本地系统代理，否则**请求根本到不了 router**：

```bash
export NO_PROXY=127.0.0.1,localhost
```

实测（2026-09-19）：macOS 系统代理配置为 `127.0.0.1:8080` 且例外列表含 `127.0.0.1`，但 `reqwest`
（codex）不认例外列表，localhost 请求一样被送进代理，客户端最终报 `503 Service Unavailable`。
`httpx`（hermes）同理。

## 6. 观测契约（每请求一条 DecisionRecord）

| 字段组 | 内容 |
|---|---|
| 身份 | `request_id`、`client`（UA 归一）、`session`（来自 §4 `key_sources`，优先 `prompt_cache_key`）、`thread_id`、`turn_index` |
| 协议 | `protocol_in`、`protocol_out`、`translated`（bool）、`lossy[]` |
| 决定 | `provider`、`model`、`selection_source`（explicit/alias/plugin）、`plugin_chain[]`、`decision_ms` |
| 状态 | `stateful_inbound`（`store != false` 或 `previous_response_id` 非空）、`sticky_hit`、`cache_control_breaks` |
| 前缀 | `prefix_blocks[]`（每块 token 数 + hash；**块粒度与 hash 定义见下**）、`prefix_continuity`（相对同 session 上一请求的最长公共块比例） |
| transform | 每步：`plugin`、`added_input_tokens`、`saved_input_tokens`、`saved_output_tokens`、`cache_impact`、`verdict`（verified/inferred）、`tee_id`（可选；v0.1 未启用 tee 时为 `null`） |
| usage | 归一化 `Usage { input_total, input_cached, cache_write, output, reasoning }` |
| 成本 | `cost.input_miss`、`cost.input_hit`、`cost.cache_write`、`cost.output`、`cost.total`、`quota_after` |
| 结果 | `status`、`upstream_status`、`failover_from`、`overhead_ms`、`upstream_ms` |
| 失败明细 | `errors[]`（数组；**无失败 = 空数组，不省略**），每条 `{ kind, message, plugin?, details? }`；`kind ∈ {transform_error, upstream_error, trace_write_failed, internal}`（§8） |

**`prefix_blocks[]` 的定义**（可比较性的前提，两个实现不得各自发挥）：

- 块 = 上游可见前缀区（`messages` / `input` / 系统指令位）里的**结构单元**：一条 message / 一个 tool 定义 /
  一个 input item。**不是**定长 token 桶（结构与缓存断点对齐，见 ADR-007）。
- 每块记 `tokens`（该块 token 数）与 `hash`；`hash = sha256(块原始字节)` 的 hex **前 16 位**。
- 该定义域**不含** router 自有字段，故"删 router 自有字段"不改变任何块 hash（§2 字节边界）。
- 落盘路径/滚动由 §4.1 的 `trace` 指定；写失败不阻塞请求（§8），并记 `errors[].kind = trace_write_failed`。

**指标定义**（`router stats` 暴露，autowork gate 引用）：

- `cache_hit_rate` = Σ`input_cached` / Σ`input_total`
- `stateful_inbound_rate` = stateful 请求数 / 总请求数（用于持续确认"是否依赖 stateful"）
- `prefix_continuity_p50` = 同 session 相邻请求的最长公共前缀块比例的中位数（**保真度指标**：掉下来
  说明某个 transform 在破坏缓存）
- `verified_savings_tokens` = 仅统计 `verdict=verified` 的 transform 收益
- `overhead_ms_p99` = router 自身开销（不含上游）

## 7. 计账口径（不得含糊）

| 口径 | 定义 | 用途 |
|---|---|---|
| `verified` | 来自上游 `usage` 的**实测差值**：同 session 内 transform 开/关的对照回合，或 `cached_tokens` 的可归因变化 | **只有它能进 gate、能对外报数** |
| `inferred` | 本地 tokenizer 估算，无对照 | 只能用于调试与方向判断，必须标注 |

报告要求：任何"省了多少"的陈述必须说明口径、样本量、时间窗；混用口径视为错误。

## 8. 降级与错误行为

**统一错误体**（所有非 2xx，含桩端点；客户端按此解析，不要依赖各家上游的错误形状）：

```json
{"error": {"type": "quota_exceeded", "message": "monthly quota exhausted for zai/glm-5.3",
           "request_id": "req-7", "details": {"provider": "zai", "over_quota": "block"}}}
```

| `error.type` | HTTP | 触发 |
|---|---|---|
| `invalid_request` | 400 | 请求体不可解析 / 缺 `model` / 字段类型错 |
| `auto_not_supported` | 400 | `model: auto`（v0.1，§3） |
| `capability_unsupported` | 400 | 入站协议 ∉ 该 provider `supports`（未声明的格 = 400，不给"尽力而为"的翻译） |
| `stateful_unsupported` | 400 | stateful 入站且粘性无法保真（ADR-004） |
| `cost_cap_exceeded` | 403 | guard 成本上限命中 |
| `unknown_provider` / `unknown_model` | 404 | `provider/model` 或别名解析不到 |
| `quota_exceeded` | 429 | `quota.over_quota = block` 且额度耗尽 |
| `upstream_error` | 502 | 上游错误且 fallback 链用尽（`details.upstream_status`） |
| `upstream_timeout` | 504 | 上游尝试超时且链用尽 |
| `not_implemented` | 501 | v0.1 三个协议端点的桩（转发在 Round 2 落地） |
| `internal` | 500 | 其它 |

响应头：`X-Router-Request-Id`（恒有）、`X-Router-Session`（解析出 session 时）、`X-Router-Lossy`（发生有损
翻译时）。SSE 路径首个事件前必须已发这三个头。

行为条款：

- transform 失败 → **回退原文**（fail-safe），trace 记 `errors[].kind = transform_error`，请求照常转发。
- 上游 5xx / 429 / 配额耗尽 → 按 config 的 `fallback` 链切换（§4.2；切换会失去缓存，需记 `failover_from`
  与由此产生的 re-prefill 成本）。
- 前缀不连续（cache-guard 检测到）→ 按 `strict_prefix` 处理：默认告警 + 继续；严格模式拒绝该 transform。
- trace 落盘失败 → **不影响请求**，记 `errors[].kind = trace_write_failed`（观测缺失必须显式，不静默）。
- 未知字段：**必须原样透传**（协议演进友好），不得静默丢弃。
