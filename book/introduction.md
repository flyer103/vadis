# Introduction

Status: outline only. This chapter describes what the user sees; the numbers behind it
are never copied here — they live in the engineering contracts the links point at.

vadis is a local-first, multi-protocol LLM gateway. You point an existing agent client
(codex, hermes, claude code) at `http://127.0.0.1:8790/v1` and vadis decides which
(provider, model) the request goes to, saves tokens without breaking the upstream prefix
cache, and records every decision and every cent into a replayable trace.

## Outline

- **What vadis is** and what it deliberately is not in v0.1 — the non-goal list is part
  of the contract, not marketing.
- **How to run it**: build, configure the roster, start `serve`, point a client at it.
- **How to connect real clients** — including the local-proxy prerequisite that, when
  missed, makes requests never arrive at all.
- **How it saves money**: prefix stability first, payload discipline second, provider
  arbitrage third.
- **How to read the reports**: the `plan` section of `GET /health` and `vadis stats` (both
  served), and the difference between a measured (`verified`) and an estimated (`inferred`)
  saving. `vadis replay` and `vadis trace tail` are planned, not served — the list is
  `docs/spec.md` §9.
- **How to extend it** with plugins and rule files, and what is stable enough to rely on.
- **What is coming next** and where the authoritative current state lives.

## Authoritative sources

- [`README.md`](../README.md) — the one-screen summary of the project and its status.
- [`docs/spec.md` §1](../docs/spec.md) — goals and non-goals (WHAT is contractual).
- [`design/DESIGN.md` §1](../design/DESIGN.md) — the system panorama (HOW it is built).
- [`AGENTS.md`](../AGENTS.md) — the hard constraints that bind every change, including
  the rule that this book links rather than duplicates.
- [`docs/spec.md` §1](../docs/spec.md) — goals and non-goals (WHAT is contractual).
- [`docs/spec.md` §9.3`](../docs/spec.md) — what is served and what is not.
