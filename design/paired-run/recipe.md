# The paired run — recipe (ADR-059 §7.1)

This is the instrument that mints the first `verified` input-token net (spec §7's first
route: a control turn with the transform on/off within one session). Its method, its nine
conditions (P1–P9) and its eight refusals (R1–R8) are ADR-059's and are not restated here;
this file is the copy-pasteable procedure. Everything runs against a `serve` of the
instrument's **own** — never the operator's gateway (port 8290 is production), never
`~/.config/vadis`, never `vadis setup`.

Artefacts in this directory:

| file | what it is |
|---|---|
| `recipe.md` | this file |
| `fixture.json` | the one body, `responses`-wire, non-streaming, fixed `prompt_cache_key`; sha256 in the report |
| `record_proxy.py` | the byte witness: a mock upstream (`--mode mock`, authors `usage` — dry run only) or a recording TLS relay (`--mode forward`) between the instrument's `serve` and the real upstream |
| `off.out.json` / `on.out.json` | the two measured rows' exact outbound bodies, as recorded on the wire (F3's anchor) |
| `reduce.jq` | the offline reducer: eight trace rows → P1–P9 with observed values → `report.json` |
| `report.json` | the first run's report (or its refusal) |

The fixture targets the shipped `bash-log-noise` rule: one `exec_command` call
(`rules/tool_output.toml`'s measured live vocabulary) whose `output` is a constructed
build-log payload — noise lines the rule drops, duration suffixes it normalizes, two lines
past `truncate_lines_at`, kept diagnostics — with a final user message **after** the tool
output so the measured pair's prefix diff has a later block that must hash identically (P6).

## The eight steps

### 1. Build and pin

```sh
CARGO_BUILD_JOBS=4 cargo build --workspace        # one cargo invocation at a time on this host
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace --no-fail-fast
git rev-parse HEAD            # the run's commit
git status --porcelain        # MUST be empty (R8 otherwise)
shasum -a 256 target/debug/vadis
```

If the checkout that will serve is dirty (a shared tree with another card's edits), run from
a **separate clean worktree at the commit you name**: `git worktree add --detach <dir> <commit>`.

### 2. The instrument's own config pair (never `~/.config/vadis`, never `vadis setup`)

Under a scratch dir (`$TMPDIR`): `config.yaml` + `providers.yaml` with

- `server.addr` on an ephemeral loopback port (bind `127.0.0.1:0`, release, use that number);
- `trace.dir` and the state dir inside the same scratch dir (state is fixed at
  `<config dir>/state/vadis.db`, spec §4.5);
- **one metered provider entry** that declares `responses` with a complete `urls.responses`
  pointing at the recording proxy (step 4), its price block transcribed from the official
  pricing page with `source:` URL + read date on the line (constraint 5);
- **no `plan_policies`**, an **empty `fallback: []`**, no inbound auth, exact-match response
  cache off (its default — a cache hit voids the measurement, P3);
- a `plugins:` entry `builtin/transform_rules` whose `rules_file` is the repository's
  `rules/tool_output.toml` **by absolute path**;
- one alias (`paired-run`) for the measured route.

### 3. The fixture

Committed at `design/paired-run/fixture.json`; hash it and never edit it in place:
`shasum -a 256 design/paired-run/fixture.json`. Constructed data only.

### 4. The byte witness + serve

```sh
python3 design/paired-run/record_proxy.py --mode forward --port <proxy_port> \
    --log-dir <scratch>/proxylog --upstream-host api.deepseek.com &
target/debug/vadis serve --config <scratch>/config.yaml &   # record its pid
```

Pin the trace window: the scratch trace dir starts empty, so every line after the first
request is the run's. The provider key must be in `serve`'s env (`api_key_env` names it;
the value never enters any file).

### 5. The four requests (measured quartet)

`--noproxy '*'` is not hygiene — the macOS system proxy intercepts `127.0.0.1` and ignores
its own exclusion list. `--data-binary` keeps the fixture's bytes.

```sh
curl --noproxy '*' -sS --data-binary @design/paired-run/fixture.json \
     -H 'content-type: application/json' -H 'X-Vadis-Transform: passthrough' \
     http://127.0.0.1:<port>/v1/responses    # #1 warm-up control  (never measured)
# … the same line three more times, with: transform (#2 warm-up treatment),
#   passthrough (#3 measured control), transform (#4 measured treatment)
```

The measured pair is **#3/#4** — the first of each arm only puts that arm's prefix in the
provider's cache. Reducing #1/#2 is the one mistake that silently doubles the number.

### 6. The null quartet (P9 — run it before trusting any figure)

The same four requests with the `X-Vadis-Transform` header **absent** (not `passthrough` —
absent is the default path, and P4 checks the record says so). Identical bytes must read
identical `usage.input_total`; a non-zero null prints **no figure at all** (R7).

### 7. The reduction (offline, no network)

Shut the instrument's `serve` down **by pid** first (the store is EXCLUSIVE-locked while it
lives, and reading it needs the process gone):

```sh
sqlite3 -readonly <scratch>/state/vadis.db \
  "select request_id, body_hash from events where kind='upstream.submitted' and body_hash is not null;"
jq -s -f design/paired-run/reduce.jq <scratch>/state/traces/*.jsonl \
   --arg basis measurement \
   --arg fixture_path design/paired-run/fixture.json \
   --arg fixture_sha256 <sha256> --arg fixture_class composed \
   --arg rule_file_sha256 <sha256 of rules/tool_output.toml> \
   --arg plugin_config 'id=tool-output-rules, kind=builtin/transform_rules, rules_file=rules/tool_output.toml' \
   --arg commit <rev-parse HEAD> --arg binary_sha256 <sha256 of the binary> \
   --argjson gates '{"build":true,"test":true,"clippy":true,"fmt":true}' \
   --argjson porcelain_empty true --argjson artefacts_consistent true \
   --argjson body_hashes '{...from the sqlite step...}' \
   --argjson row_refs '[{"request_id":…,"file":"….jsonl","line":n}, …]' \
   --argjson price_entry '{… the route, the per-1K tiers, the source URL + read date …}' \
   --argjson response_receipt '[{"seq":1,"req_sha16":"…","status":200,"len":…,"sha256":"…"}, …]' \
   > design/paired-run/report.json
```

`response_receipt` is one entry per proxy `meta.jsonl` line of `kind: resp` — the recorded
upstream response's `status`, `len` and `sha256`, keyed by the request's own `req_sha16` —
so the report carries the independent copy of every `usage` figure it quotes.

`body_hash` here is the `upstream.submitted` event's — the sha16 of the **byte-final**
outbound body (post transform **and** post model rewrite), unlike `request.received`'s
(pre model-rewrite). F3 verification, before trusting the report:

1. `sha256(off.on.wire.bodies)[:16]` (from `proxylog/NNN.body`, requests #3 and #4) must
   equal the two measured rows' `upstream.submitted` hashes — and those two files are
   `off.out.json` / `on.out.json`, committed;
2. the two must differ in exactly the payload span `transforms[].edited_paths[]` names
   (diff them: the `model` value, and the `output` string of the tool result);
3. the null quartet's own #3/#4 wire bodies must be byte-identical;
4. the control's wire body must equal the fixture modulo mutation (b) (the `model` value
   replacement) — the fixture carries no vadis-owned top-level field, so mutation (a) is
   empty here;
5. the recorded upstream responses (`<seq>.resp.body`, digests in `meta.jsonl`'s `kind: resp`
   lines and in the report's `response_receipt`) must contain the provider's own `usage`
   objects — this is the independent copy that makes "reproduce without trusting the author"
   true at the last step: recompute both `usage.input_total` figures from these bodies, not
   from the router's parse.

### 8. Shut down by pid

Kill only the pids this recipe started (the proxy and the instrument's `serve`). Never a
pattern match — `pkill vadis` would take the production gateway with it.

## The zero-cost dry run (`basis: fixture-arithmetic`)

Same driver, `--mode mock`: the proxy answers every request itself with a deterministic
responses body whose `usage` **the mock authors** (`input_tokens = len(body)//4`). It
exercises steps 4–7 — pairing, ordering by `event_id`, the admission table, every refusal
branch, the reducer — at $0.00 and proves **plumbing only**: a delta computed from authored
counters is the fixture's arithmetic, never the provider's count. The report it prints
carries `basis: fixture-arithmetic`, and its figures are labelled `fixture-arithmetic
(plumbing only)`, never `verified`. It cannot prove a saving.

## What this instrument does not licence

ADR-059 §9's ten limits bind every report it prints: no output-side claim, no session
total (n × delta is a multiplication without evidence), no other-regime money as spend, no
generalisation beyond the fixture's shape (`composed` ≠ client traffic), no `tee`-loss
cost, no quality claim, no latency claim, no plan-account dollars, no later-turn cache
dynamics, no traffic-distribution claim.
