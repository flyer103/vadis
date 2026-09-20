# ADR-007 — span-faithful forwarding: the request body travels as raw bytes, no parse → reserialize round trip

- Status: accepted
- Date: 2026-09-19
- Related: AGENTS hard constraints 1/2; spec §2 (byte boundary) / §6 (prefix blocks); DESIGN §12.3/§12.3.1; implementation `crates/router-core/src/body.rs`

## Background

The first-order lever of prefix caching is that **the bytes the upstream sees stay unchanged across turns**.
Measurement (2026-09-19, codex → ZAI) shows the client resends the whole body every round (3 → 6 items),
with `store:false` and `previous_response_id` absent, so the stability of the prefix is decided entirely by
router's outbound bytes: in the second round of the same session `cached_tokens` reaches 14400/14520
(99.2%), while any single "the order changed / the whitespace changed / an unknown field is gone" pushes the
hit rate back to 0 (the whole prefix recomputed at the miss price).

The most common way it breaks is "parse into a struct and serialize back" (parse → reserialize): it looks
like the identity transform, but in practice field order, unknown fields, numeric literals
(`1e-9` → `0.000000001`), whitespace and escape forms can all change, and even JSON semantics can change
(duplicate keys, large integers). Such rewrites are **undetectable and irreversible**: the cache silently
fails and the cost actually rises.

## Decision

1. **The request body is carried as raw bytes through the pipeline**: `RawBody(Vec<u8>)` (DESIGN §12.3.1).
   It does **not** implement `DerefMut`/`AsMut`, nor does it expose a mutable reference to
   `serde_json::Value` — that blocks "just tweak it in passing" at compile time.
2. **The only permitted rewrite = deleting top-level router-owned fields**: the whitelist constant
   `ROUTER_OWNED_TOP_LEVEL_KEYS` (currently only `router_meta`; legitimate unknown client fields are never
   in it). Adding an owned key must change that constant; a second list scattered at call sites is not
   allowed.
3. **The parse → reserialize round trip is forbidden**: `remove_top_level_keys` uses a **single-pass span
   scanner** (tracking strings/escapes/bracket depth) to locate only the byte ranges of the members to be
   deleted and then remove those intervals; `serde_json` serves **only as a validator of raw value
   fragments** (number/true/false/null) and never produces any outbound byte.
4. **The encoder signature returns a borrow**: the native cell's encoder returns `Cow<'_, [u8]>`, and when
   the bytes are identical it must be `Borrowed` (zero rebuild); only the translated cell may return
   `Owned`, and its mapper must be a pure function of `(content, stable config)` (same content → same
   upstream bytes, spec §2 / ADR-004).
5. **The domain of prefix blocks and hashes = the bytes the upstream sees**: a block = a structural unit in
   the prefix region (one message / one tool definition / one input item), and
   `hash = the first 16 hex chars of sha256(block raw bytes)` (spec §6). Therefore "deleting router-owned
   fields" changes no block hash — CONF-10 asserts exactly that.
6. **The separator semantics of deletion (pinned 2026-09-19)**: members that consecutively hit the whitelist
   form a "run"; the run is removed as a whole and swallows the comma between the **run's end** and its
   successor member (including the whitespace in between), while the comma at the run's start is left to the
   previous retained member; only when the first member is itself the start of the run does it swallow the
   trailing comma instead. Invariant: **any `Ok` output must be valid JSON**, and retained members are
   byte-for-byte equal to their input span.
7. **Boundary-behaviour trade-offs (each pinned by a unit test, never "fixed in passing")**:
   - A BOM prefix → `Err(NotTopLevelObject)`: stripping the BOM is a rewrite outside the whitelist, left to
     the caller to handle as a 400.
   - A leading-zero number (`01`) → `Err(Malformed)`: not part of the RFC 8259 number grammar, so no
     ambiguous number is passed through.
   - Invalid UTF-8 inside a string **value** → `Ok` and passed through byte for byte (upstream decides);
     invalid UTF-8 inside a **key** → `Err(Malformed)` — a key must be decodable to be compared
     semantically with the whitelist; the asymmetry is intentional.
8. **Container deviation recorded honestly**: the blueprint wrote `RawBody(Bytes)`, but the `bytes` crate is
   not on `router-core`'s dependency allowlist, so `Vec<u8>` is used in practice; outbound zero-copy
   (`Bytes::from(vec)`) happens in the proxy layer and the byte semantics are unaffected.

## Rationale

- It turns "no byte may change" from **discipline** into a **type fact**: with no mutable view you cannot
  write an accidental rewrite, and code review only has to review the whitelist constant.
- The span scan is a single O(n) pass with no allocating re-encode: fast, and free of normalization side
  effects; it uses the same scanning semantics as "splitting prefix blocks by structural unit", so the
  metrics and the data plane never tell different stories.
- `Cow<'_, [u8]>` makes "the native path must borrow" a signature-level constraint: once someone writes the
  native encoder as a byte rebuild, the type signature and CONF-01/02/03's byte-equivalence assertions fail
  together, instead of degrading quietly.

## Consequences

- "native passthrough's upstream-visible bytes == the client's bytes (minus the whitelist fields)" becomes an
  assertable contract (CONF-10 already has real executing cases; the link-level assertion comes with the data-plane landing). The
  `deletion_position_matrix*` test matrix and the adversarial input table are a **permanent regression**; new
  deletion semantics must extend them.
- The parse view the decision relies on (`JsonDoc`, order-preserving) is used for **decisions only** and is
  never allowed to flow back into the outbound path.
- Any "just normalize it in passing" request (trim, reorder keys, unify number notation) counts as a
  violation of hard constraint 1: either abandon that transform, or register it explicitly as a transform at
  the plugin layer and bear the cache cost (spec §6's `cache_impact`).
- This decision makes the `prefix_continuity` metric trustworthy: when it drops, the cause can only be a
  transform we explicitly performed.
