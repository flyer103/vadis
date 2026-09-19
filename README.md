# router

多协议 LLM 网关：**字节保真的数据面 + 可逆的插件运行时 + 可实测的成本引擎**。

给日常的 agent（codex / hermes / claude code）用：客户端把 base_url 指过来，router 决定请求走哪个
(provider, model)、在**不破坏上游前缀缓存**的前提下省 token，并把每一次决定与每一分钱记进可回放的
trace。

## 现状

| | |
|---|---|
| 阶段 | v0.1 契约已定，实现未开始（见 `docs/spec.md`、`design/DESIGN.md`） |
| 选模型 | **显式指定** (provider, model) 或别名；自动选择是插件槽，v0.1 不启用 |
| 协议 | 入站/出站均支持 OpenAI chat completions / OpenAI responses / Anthropic messages |
| 成本 | P0 缓存保真 → P1 输入侧载荷压缩 → P2 输出侧纪律 → P3 provider 套利 |
| 迭代 | `autowork/` 循环侧持续迭代产品（多 profile + kanban，见 `autowork/program.md`） |

## Quick Start

```bash
cp config.example.yaml config.yaml     # 编辑 roster：provider / model / 价格 / 配额
set -a && source .env && set +a        # provider keys 只从 env 读，绝不写进 yaml
cargo run -p router-cli -- serve --config config.yaml
```

客户端接入（**必须**先做，否则 macOS 系统代理会截走发往 localhost 的请求）：

```bash
export NO_PROXY=127.0.0.1,localhost     # codex(reqwest) / hermes(httpx) 都读这个变量
```

codex 侧：

```toml
[model_providers.router]
base_url = "http://127.0.0.1:8790/v1"
wire_api = "responses"                  # 或 "chat"
env_key  = "ROUTER_TOKEN"
```

## CLI

```bash
router serve      --config config.yaml          # 启动网关
router stats      --window 24h                  # 成本 / 缓存命中 / stateful 占比 / 每 transform 实测收益
router replay     --trace traces/x.jsonl --config config.yaml   # 离线回放：同一份真实代码路径算成本
router trace tail                               # 跟随查看决定与计账流水
```

## API

入站端点（三者等价，按协议镜像上游语义）：

| Method | Path | 协议 |
|---|---|---|
| POST | `/v1/chat/completions` | OpenAI chat completions |
| POST | `/v1/responses` | OpenAI responses |
| POST | `/v1/messages` | Anthropic messages |
| GET | `/health` | 存活 + 已装载插件/服务 |
| GET | `/metrics` | Prometheus |

`model` 字段接受：`provider/model`、配置中的别名、或 `auto`（v0.1 返回 400 并提示由插件接管）。
响应中附带 `router_meta`：命中的插件链、每步 transform 的计账、session 与缓存状态。

## Build & Test

```bash
cargo build --workspace
cargo test  --workspace          # 含 tests/conformance 的 3×3 协议矩阵
cargo clippy --workspace -- -D warnings
```

## 目录

```
crates/router-core        领域模型、决定管线、成本引擎、cache 账本、插件 trait
crates/router-protocol    3 协议编解码、翻译矩阵、usage 归一化
crates/router-providers   provider 适配（wire_api 能力、鉴权、重试、SSE）
crates/router-runtime     Cordis 语义运行时（effect / coeffect / fiber / 声明式 loader）
crates/router-plugins     内置 tier-A 插件
crates/router-proxy       数据面（axum），字节保真转发
crates/router-cli         serve / stats / replay / trace
crates/router-plugin-sdk  tier-B 进程外插件协议
tests/conformance         协议保真、前缀稳定、计账口径
autowork/                 迭代循环侧（Python 编排 + router replay 做策略模拟）
```

## 设计文档

- [Spec (WHAT)](docs/spec.md) — 协议契约、配置 schema、观测与计账口径
- [Design (HOW)](design/DESIGN.md) — crate 布局、插件运行时、成本引擎、缓存策略
- [Decisions (WHY)](design/decisions/) — ADR-001…005
- [Autowork](autowork/program.md) — 迭代章程、gate、方向池

## Ops

- 不做服务端会话状态：入站 `store:true` / 非空 `previous_response_id` → 粘性路由 + trace 打标。
- 缓存是一阶成本杠杆：任何改写都必须是**内容确定性**的（同一内容 → 同一上游字节）。
- 观测指标见 `docs/spec.md` §观测契约；离线回放是唯一算钱的权威口径。
