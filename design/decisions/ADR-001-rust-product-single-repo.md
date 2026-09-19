# ADR-001 — Rust 产品 + 单仓（root = 产品，`autowork/` = 循环侧）

- 状态：accepted
- 日期：2026-09-19

## 背景

需要一个日常给 codex / hermes 用的网关：必须字节保真、低开销、长期可扩展（插件化实验），并且要有一个
能持续迭代它的循环侧（autowork）。两个候选形态：单仓（root 是产品，`autowork/` 是循环）或双仓
（产品 + 独立 autowork 仓）。

## 决策

1. **产品用 Rust**（workspace，多 crate）：数据面需要无 GC 抖动、可预测的 p99、以及"绝不改动字节"的
   强类型约束（`serde_json` 的保序/原始字节透传比动态语言更容易做对）。
2. **单仓**：repo root = 产品；`autowork/` 是循环侧。

## 理由

- 产品与循环侧之间存在**强耦合的接口**：trace 字段、插件契约、config schema、replay 子命令。双仓会
  让"改 trace 格式"变成跨仓同步问题，而这恰好是旧项目最痛的一类 bug（多文件/多仓同步漂移）。
- 循环侧的产物（规则 TOML、config、tier-B 插件）必须在同一提交里与产品契约一致演进；单仓让
  "gate PASS → 合入 main"成为一次原子操作。
- 参考形态：旧的 `nexaroute/router`（root 是 Go 产品 + `research/` 循环侧）已验证这个组织方式可长期
  运转。

## 后果

- `autowork/` 的大文件（traces/results/corpus）必须 gitignore，仓库只保留 harness/配置/round 文件。
- 循环侧语言不必与产品相同（见 ADR-005）。
- 单仓意味着 CI 需要区分两个平面：`cargo` 组（阻塞）与 `autowork` 组（报表类，非阻塞）。
