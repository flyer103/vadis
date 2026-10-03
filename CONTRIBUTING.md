# Contributing

Thanks for the interest. This repository is small and opinionated: the rules below are
load-bearing invariants, not style preferences. `AGENTS.md` is the authoritative list — this
file is the practical subset for someone opening a pull request.

## 1. Build it, and run all four gates

```bash
cargo build --workspace
cargo test  --workspace --no-fail-fast
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

All four must be green before you open a pull request. CI (`.github/workflows/ci.yml`) runs
exactly these four commands on every push and every pull request, so what you run locally is
what decides.

Requirements:

- **Rust**: the toolchain pinned in `rust-toolchain.toml`. With `rustup` you need to do
  nothing — the first `cargo` invocation in the checkout installs it. The *minimum* supported
  version is lower and is declared as `rust-version` in `Cargo.toml` (`[workspace.package]`),
  inherited by every member crate.
- **A C compiler**: `rusqlite` is built with its `bundled` feature, so the pinned SQLite C
  library is compiled at build time. No system SQLite is needed, but a working `cc` is.

`tests/conformance` is a workspace member and runs with everything else: it holds the 3×3
protocol matrix, the byte-fidelity and prefix-stability invariants and the accounting
convention. To run one case while iterating:

```bash
cargo test -p router-conformance --test conf_20_ordered_write_invariant
```

An `#[ignore = "CONF-NN: depends on <item>"]` case is not a passing case — it is a declared gap,
kept visible on purpose. Do not turn one green by deleting the `#[ignore]` and weakening what
it asserts.

## 2. One logical change per commit

One commit contains one logical change, and only that change's own files — no unrelated
working-copy edits, no drive-by reformatting, no rename bundled into a fix. Commit messages are
in English. If a change spans several files for one reason, that is still one commit; if it
does two things, it is two commits.

## 3. All repository text is English

Docs, code comments, commit messages and file names are English. The one exemption is captured
data — raw recorded client traffic kept on the private side, not in the published tree: that is
traffic, not authored text. (AGENTS.md constraint 7.)

## 4. The measurement is not part of the change space

> The gate definitions, the fixed corpus, the conformance assertions and the L1 envelope are
> outside the mutable scope: a change to any of them is a human decision, never a loop outcome.
> A gate verdict records the evaluator commit and the corpus digest it ran against.
>
> — AGENTS.md, constraint 9 (the reasoning is in ADR-012)

A pull request that turns a red gate green by editing the gate, the corpus, the conformance
expectations or the L1 envelope is rejected, whatever the numbers say. If you believe the
measurement itself is wrong, open an issue that states what changes, the evidence for it and
what the previous values were: the change is then a recorded human decision, not a side effect
of a pull request.

## 5. Docs before code

User-visible behaviour goes into `book/` before it goes into the implementation. `docs/spec.md`
is the WHAT (protocol contracts, config schema, observation and accounting), `design/DESIGN.md`
is the HOW, and `design/decisions/ADR-NNN-*.md` is the WHY. The ADR directory is
**append-only**: when a decision changes, add an ADR that supersedes the old one instead of
rewriting it.

## 6. Secrets and captured data do not get committed

`config.yaml`, `.env`, `target/`, the local state database and the trace corpora are
git-ignored — keep it that way. Provider keys are named in the config as environment variables
and read from the environment only, so they never need to appear in a file. Never paste a key
into an issue, a commit, a test fixture or a log.

## 7. Reporting a security problem

Privately, never as a public issue — see [`SECURITY.md`](SECURITY.md).

## 8. Where things live

| Path | What it is |
|---|---|
| `crates/router-core` | domain: decision pipeline, cost engine, prefix/cache attribution, quota, trace contract |
| `crates/router-protocol` | the three-protocol codec: usage normalization, the SSE block parser |
| `crates/router-providers` | provider adapters: wire_api capability, auth, error classification |
| `crates/router-proxy` | the data plane (axum), byte-faithful forwarding |
| `crates/router-cli` | the binary (`serve`, `stats`, `setup`) |
| `crates/router-store` | SQLite/WAL event log + projections, and the JSONL trace sink |
| `tests/conformance` | the protocol matrix and the byte/prefix/accounting invariants |

Start with `README.md`, then `book/` for the user-facing story.
