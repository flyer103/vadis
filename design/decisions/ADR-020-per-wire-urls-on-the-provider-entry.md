# ADR-020 — the provider entry names each wire's endpoint in full: `urls` replaces `base_url`, and the router stops composing paths

- Status: accepted
- Date: 2026-09-21
- Related: AGENTS hard constraints 1 (the byte boundary — a URL is not body bytes, so this decision moves
  nothing on the passthrough path), 5 (no fabricated prices — the reason this repository demands a cited
  line per value), 8 (docs before code); ADR-011 (the provider layer makes **no decisions** — it surfaces
  raw material and hands it up), ADR-018 ("a deployment is an entry": this ADR extends what the entry
  owns, from its region and unit to its endpoints); spec §4 (+ the new §4.9) / §4.6 / §4.8; DESIGN §12.5
  (load-time validations), §12.10.1 (`UpstreamPlan` and URL assembly), §12.10.2 (the config landing);
  `config.example.yaml` (the 2026-09-21 re-read that surfaced the defect);
  `tests/conformance/tests/conf_03_native_anthropic.rs` (the assertion that encoded the old rule)

## Background

### What shipped

The provider entry carried one `base_url`, and `router-providers::build_request` composed the outbound URL
from it plus one fixed tail per protocol:

```rust
let path = match plan.protocol_out {
    WireApi::Chat      => "/chat/completions",
    WireApi::Responses => "/responses",
    WireApi::Anthropic => "/v1/messages",
};
let url = format!("{}{path}", plan.base_url.trim_end_matches('/'));
```

The rule that went with it — *"base_url must already contain the version segment"* (spec §4, DESIGN §12.5
and §12.10.1) — holds only when a vendor puts its version segment in the base. That is not a fact about
vendors; it is a fact about one of them.

### The three readings that break it

The 2026-09-21 re-read of all eight provider entries (every `base_url`, `context` and price line checked
against the page it cites) made the mismatch visible:

| vendor | OpenAI-form endpoint | Anthropic-form endpoint | what the old rule produced |
|---|---|---|---|
| Moonshot (intl) | `https://api.moonshot.ai/v1/chat/completions` | `https://api.moonshot.ai/anthropic/v1/messages` | `…/v1` + `/v1/messages` = `…/v1/v1/messages` |
| z.ai (intl) | `https://api.z.ai/api/paas/v4/chat/completions` | `https://api.z.ai/api/anthropic/v1/messages` | `…/api/paas/v4` + `/v1/messages` |
| DeepSeek | `https://api.deepseek.com/chat/completions` | `https://api.deepseek.com/anthropic/v1/messages` | `https://api.deepseek.com` + `/v1/messages` |
| Zhipu CN | `https://open.bigmodel.cn/api/paas/v4/chat/completions` | `https://open.bigmodel.cn/api/anthropic/v1/messages` | `…/api/paas/v4` + `/v1/messages` |

Three entries (`deepseek`, `zai`, `kimi`) declared an `anthropic` cell, and for all three the composed URL
was wrong: the vendor serves that form at a **different** base, and the entry has one field. Note also the
DeepSeek row: its documented OpenAI base carries **no** version segment at all while the Anthropic tail
carries `/v1` itself — so the "version segment lives in the base" convention was never a convention, only a
coincidence that held while one wire per entry was reachable.

### Why the failure is worse than a 404

The wrong URL answers `404`, and `router-core::error_class` classifies `404` as `ErrorClass::ModelNotFound`
— a class whose own doc comment reads *"a roster defect. Fail over; the roster entry is surfaced."* The
router therefore burns a second attempt (losing the prefix cache, which is the first-order cost lever),
and the trace attributes the failure to the model rather than to the roster. A configuration defect is
silently re-labelled as a model defect.

## Decision

### 1. `urls`: one complete URL per wire protocol, and no assembly

A provider entry states, for each inbound protocol it declares, the **entire** URL the router will POST to:

```yaml
  - name: kimi
    wire_api: chat
    supports: [chat, anthropic]
    urls:
      chat:      https://api.moonshot.ai/v1/chat/completions
      anthropic: https://api.moonshot.ai/anthropic/v1/messages
```

`base_url` is **deleted**, not deprecated: two keys that can disagree about the same fact is exactly the
defect this ADR exists to remove. The value is used verbatim — no append, no trailing-slash trimming, no
normalization — so a reader of the config can predict the outbound request without knowing router's rules.

### 2. The declaration and the reachability cannot disagree

`validate` refuses a config in which the two sets differ, naming the field path:

- a wire in `supports` with no `urls` entry → load error (`providers[i].urls`);
- a `urls` key the entry does not declare in `supports` → load error;
- `wire_api ∈ supports` stays as it was (spec §4).

This is the part that makes the class of bug unreachable rather than merely absent: "declared but
unreachable" and "reachable but undeclared" both stop the process.

### 3. A URL must be a URL

Each value must start with `http://` or `https://` and contain no whitespace; anything else is a load
error naming the path and the bad value. The router still does **not** check a URL against the entry's
`region` (ADR-018 §3's stance, unchanged: vendors own their host lists, and a built-in table of them rots).

### 4. The plan carries the resolved string

`UpstreamPlan.base_url: &str` becomes `UpstreamPlan.url: &str` (DESIGN §12.10.1) — the plan carries the
URL resolved for this attempt's `protocol_out`, and `build_request` keeps only what it never should have
shared with URL composition: the auth headers. The provider layer continues to decide nothing (ADR-011);
the resolution is a lookup, and the lookup's precondition is enforced at load time by item 2.

### 5. One migration, in this change

The contract, the parser, the assembly, `config.example.yaml`, the operator's live `config.yaml`, the
conformance fixtures and the autowork harness's generated configs move together. A half-migrated tree is
not a state this repository should ever be in.

## Alternatives considered

- **Keep `base_url`, add a per-wire override (`base_url_anthropic`).** Same expressive power as `urls`,
  but it leaves the hole open for the next wire: the default silently applies to every cell nobody
  overrode, which is precisely the failure mode being fixed. It also encodes one wire as privileged.
- **Keep one base (the host) and put the per-wire *path* in config (`endpoint_paths`).** Equally
  expressible. Rejected on reviewability: the config would no longer contain a string that appears
  verbatim on the vendor's page, so checking it means mentally splitting `https://api.moonshot.ai/anthropic`
  into prefix + path — and the enforced loop for this file is *click the cited URL and compare the line*.
  It also has to be re-read whenever a vendor moves a product segment (`/paas/v4` → `/coding/paas/v4`),
  whereas a full URL is copied from the page the citation names.
- **Keep the assembly, shrink `supports` to the reachable cells.** Zero schema change, and it was the
  temporary mitigation in the config review. Rejected as the destination: the vendored capability is real
  (all three vendors document the Anthropic form), so this would delete a true declaration to protect a
  schema gap — the config would be describing router's limitation instead of the vendor's surface.
- **A vendor table in code (path templates per vendor).** This is what general-purpose gateways do.
  Rejected for the reason ADR-018 already recorded for host lists: it puts a table that rots inside the
  router, and it makes "add a provider" a code change.

## Rationale

The config file's job is to be **checkable**: every value a human may have to verify carries the URL it was
read from, and the check is a comparison, not a deduction. The old shape forced a deduction (base + tail
per protocol, with an unstated convention about where the version segment lives), and a deduction the
reader gets wrong produces a plausible-looking wrong URL. Ambiguity in a config is paid for twice: once in
review, and once in production, where the bill arrives as a misclassified 404 and a lost prefix cache.
Trading one config line per declared cell for "there is no rule to remember" is a trade this repository's
constraints (AGENTS 1, 5) are already written to make.

## Consequences

- The "already contains the version segment" convention disappears from the contract; a new entry pastes
  the vendor's own endpoint line, and `book/getting-started.md` shows the shape.
- `deny_unknown_fields` cannot police a map's keys, so the map's key type does the work: `WireApi`'s
  deserializer refuses an unknown protocol (`unknown wire protocol 'chta'`), and item 2 refuses a known one
  that the entry does not declare.
- The passthrough byte promise, the prefix hash, the cache ledger and the transform modes are **untouched**
  by construction: a URL is not body bytes, and this ADR changes no code on the body path.
- `conf_03_native_anthropic.rs`'s assertion changes meaning: it asserted "base without `/v1` + `/v1/messages`"
  and now asserts the literal the config wrote — a strictly stronger statement (the router sends exactly
  what it was given).
- The file gets longer by one line per declared cell. That is the whole cost.
- Existing installs must migrate; the migration is mechanical and is part of this change (item 5).

## Honest boundaries and verification owed

- **The router does not verify that a URL is the vendor's URL.** A wrong-but-absolute URL is a
  documentation error, and the only defence is the citation rule (AGENTS 5) plus the human re-read — the
  same boundary ADR-018 accepted for `region` vs host.
- **No test can prove a vendor accepts the URL** without live traffic. What the test suite proves is
  narrower and is the part router owns: the bytes it sends are the bytes it was configured with
  (`conf_03`'s equality assertion, plus the load-time refusals of item 2 and item 3).

## Reversibility

The change is a delete-and-add of one key plus a lookup, so a rollback is the inverse commit: re-add
`base_url`, restore the three literals, and re-migrate the configs. Nothing is written to traces or to the
store that depends on the shape — the outbound URL is not a trace field, and the prefix hash covers body
bytes only.
