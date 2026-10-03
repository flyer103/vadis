# AGENTS.md — router

Multi-protocol LLM gateway. One local process speaks the chat-completions, Responses and Anthropic
wire shapes, decides which configured provider entry serves each request, forwards the **client's own
bytes** upstream, and writes a decision + cost record for every terminal outcome.

`crates/`, `tests/`, `docs/`, `design/`, `book/`, `rules/`, the two example configuration files and
this file are the product. `CONTRIBUTING.md` is the human-facing overview; `docs/spec.md` is the WHAT,
`design/DESIGN.md` the HOW, `design/decisions/ADR-NNN-*.md` the WHY (append-only).

## Hard constraints (bind every change)

1. **Byte boundary.** The proxy forwards the original request bytes. Exactly two mutations are
   permitted, both byte-level, both scoped, both auditable: **(a)** removing router-owned
   top-level fields (e.g. `router_meta` echo, routing hints); **(b)** replacing the *value* of the
   top-level `model` field with the resolved route's provider-native model id — routing is
   resolved first, and the upstream is called with the native model id alone, while the client's
   own string is recorded in the trace as `decision.requested_model`. Never touch message content,
   order, whitespace, or tool schemas on the passthrough path. Conformance tests assert the
   upstream-visible prefix hash equals the client's modulo those two mutations.
2. **Content determinism.** Every transform must be a pure function of (content, stable
   config) — never of turn number, wall clock, or RNG. The prefix on turn N must be a
   prefix of turn N+1. Violating this silently destroys upstream prompt cache and *raises*
   cost.
3. **Observation boundary.** The serving path's only output channel is the trace JSONL and the
   event log; its only inputs are the client's bytes and its own configuration. Off-line analysis
   never enters the serving path, and the serving path reads nothing off-line analysis wrote except
   through configuration (the config file, a rule TOML, a plugin).
4. **No unverified savings.** Every transform reports its token delta and labels it
   `verified` (measured usage difference) or `inferred` (local estimate). Only `verified` numbers
   may be counted as savings. Never present an inferred number as measured.
5. **No fabricated prices.** Prices/quotas enter config only after reading the provider's
   official pricing page; the config comment records source URL + date. Never estimate.
6. **Tests must not depend on current data, and must not read outside this repository.** No
   change-detector tests (model counts, catalog snapshots) — assert relations and invariants
   instead. A test that reads a file outside this tree passes only on the machine that has that
   file; a suite that needs it is not a suite.
7. **English only.** All repository text is English — docs, code comments, commit messages, file
   names. Raw captured client traffic is not authored text and is exempt.
8. **Docs before code.** A user-facing change updates the relevant `book/` chapter in the same
   change. The book is user-facing (what it is / how to connect / how to save money / how to read
   the reports); `docs/spec.md` + `design/` stay the engineering contracts. The book never
   duplicates price numbers or type sketches — it links to the authoritative source (single-source
   price policy, constraint 5).
9. **The measurement is not part of the search space.** The gate definitions, the fixed corpus, the
   conformance assertions and the L1 envelope are outside the mutable scope: a change to any of them
   is a human decision. A gate verdict records the evaluator commit and the corpus digest it ran
   against (ADR-012).

## Environment gotchas (learned the hard way)

- **macOS system proxy intercepts localhost.** `reqwest` (codex) and `httpx` (hermes) read
  the macOS system proxy and do **not** honor its exclusion list for `127.0.0.1`. A router
  bound to a local port receives *no connection* and the client reports
  `503 Service Unavailable`. Verified 2026-09-19: codex log line
  `reqwest::connect: proxy(http://127.0.0.1:8080/) intercepts 'http://127.0.0.1:8899/'`.
  Fix/onboarding step: `NO_PROXY=127.0.0.1,localhost` (documented in README + spec).
- **Agent clients are stateless.** codex sends `store: false`, no `previous_response_id`, and
  resends the full `input` every turn (3 items → 6 items), with a stable `prompt_cache_key` = the
  session id. This is why v0.1 needs no server-side state (ADR-004).
- **`prompt_cache_key` is the session identity.** Client-generated, echoed by the upstream
  (observed on ZAI responses). Use it — do not invent a session hash when it is present.
- **Cache is real and fast.** Same-session measurement on ZAI responses:
  `cached_tokens` 960/14409 (turn 1) → 14400/14520 (**99.2%**, turn 2). Prefix stability is
  the first-order cost lever; model choice is second-order.
- **`tools` are part of the prompt.** 16 tool schemas ship in every request; changing tool
  schema ordering/formatting invalidates cache for the whole conversation.
- **The fast-fail trap is a missing TLS backend.** `reqwest` with `default-features = false`
  builds with no TLS, so every `https` upstream dies in milliseconds with a transport error the
  classifier will mislabel. Any new HTTP client here needs the same check, and an invariant test
  (`CONF-26`); offline tests cannot see it, only a real upstream smoke can.

## Commands

```bash
cargo build --workspace
cargo test  --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

These four are the repository's own gates, run verbatim by CI (`.github/workflows/ci.yml`) and by
`CONTRIBUTING.md`'s review checklist. `tests/conformance/` is a separate workspace member whose
cases are each their own target.

## Repo layout

- `crates/` — the product. `router-core` holds the domain (decision pipeline, cost engine, cache
  ledger, plugin traits); `router-runtime` holds the Cordis-style runtime; nothing outside
  `router-cli` wires them together; `router-protocol` the three wire codecs, `router-providers` the
  upstream adapters, `router-proxy` the axum data plane, `router-plugins` the tier-A built-ins,
  `router-store` the one SQLite/WAL store, `router-plugin-sdk` the tier-B protocol types.
- `tests/conformance/` — the 3×3 protocol matrix and the byte/cache invariants. A change that
  breaks these is not landed regardless of downstream wins.
- `rules/tool_output.toml` — the shipped rule file of the `builtin/transform_rules` engine; the
  example plugin entry names it, and `router setup` lands it beside the config it writes.
- `config.example.yaml` + `providers.example.yaml` — the shipped pair a first configuration starts
  from; `router setup` writes both (and the rule file) from the copies embedded in the binary.

## Version control

Plain git. One logical change per commit, containing only its own files (exclude unrelated
working-copy edits). Do not rewrite published history. Do not push unless asked: a change that is
not part of a reviewed landing stays local until the owner asks for it. Negative results commit
documentation only — never broken code.

## Docs map

- `book/` — user-facing guide (what router is, how to connect a client, the cost levers, how to
  read the reports). Written for users, not implementers: it links to the contracts and never copies
  their price numbers or type sketches.
- `docs/spec.md` — WHAT: protocol contracts, config schema, observation + accounting.
- `design/DESIGN.md` — HOW: crates, plugin runtime, cost engine, cache policy.
- `design/decisions/ADR-NNN-*.md` — WHY, append-only.
- `CONTRIBUTING.md` — how to build, test and send a change; the review checklist.
- `README.md` — what it is, the quick start (`router setup` first), the API at a glance.
