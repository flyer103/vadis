# ADR-028 — the harness-only wire recorder: a byte-preserving peer, so the live wire has an auditable `O`

- Status: accepted
- Date: 2026-09-23
- Related: AGENTS hard constraints **1** (the byte boundary — this ADR exists so that the byte audits have an
  input on a live wire, and it adds exactly one apparatus process to get one), **4** (no unverified savings —
  nothing here mints or proposes a figure), **5** (no fabricated prices — no price enters this decision), **7**
  (English only; the one Chinese string below is a verbatim quote of the owner's ruling, given with an English
  gloss), **9** / **ADR-012 item 2** (the measurement is not part of the search space: the fixed corpus, every
  threshold, `tests/conformance/` and the L1 envelope are **untouched** by this ADR) ; **ADR-009 item 3** (the
  vadis persists no body — the fact that makes a peer the only route to `O`), **ADR-015** (the two permitted
  mutations), **ADR-020** (`urls` names each wire's endpoint in full, so a forward target is a declared value and
  not a composed one); the harness contract's **ARM-3.11** (the new clause), **ARM-3.2**, **ARM-3.3.1/3.3.2/3.3.7**,
  **ARM-3.4**, **ARM-3.8.3/3.8.4**, **PROV-8 §9.1 / §9.1.2 / §9.2 / §9.3 / §9.4 / §9.5**, **REAL-9 §10.1**,
  **VER-4.1.1 / VER-4.4 / VER-4.4.1** and **§16** (the revision's own provenance); the freeze that produced the
  ruling, the loop's verdict record; the owner's ruling in the loop state record row 13 (the same file) and its
  08:25 refresh paragraph (the same file); the round's plan of record (**R30-1** = this ADR + contract rev 6, **R30-2**
  = harness code + one live evidence run, **R30-3** = the independent re-verification).

## Background

AGENTS constraint 1's conformance rule is a *byte* assertion: the upstream-visible request must equal the
client's modulo vadis-owned top-level fields and the resolved model value. The harness audits it in two ways
(ARM-3.8) — the client-visible pair and the **upstream-visible outbound body** — and the second one needs a body
`O` that the vadis actually sent.

On a **mock** run `O` comes from the mock's own record (`<arm dir>/upstream/NNNNN.req`, ARM-3.4.1 item 2). On a
**live** run there is no mock, and there cannot be: a mock cannot answer a real request without fabricating the
provider's response and its `usage`, which ARM-3.4.4 forbids in as many words. So the audit has been
`not_run` on every live arm since the live path existed, and VER-4.4.1 (rev 5) says so out loud and refuses to
let the absence read as a pass: **condition 3 fails on the live path**, and `verified` is therefore unreachable
there. That is finding **L4**, recorded by the first real-upstream run (the loop live-run findings)
and confirmed clause by clause in the loop's verdict record.

The product cannot be asked to help. ADR-009 item 3 means the vadis persists no body; making it persist one, or
log its peer's ciphertext, or accept an injected CA, would put an **observation surface inside the serving path**
whose only purpose is to make its own measurement easier — the exact inversion AGENTS constraint 3 exists to
prevent, and it would ship in `crates/` for every user.

So the question the ruling settled is not *whether* the harness may observe the wire, but *where* the observer
stands and *what* it may do there.

## Decision

**A harness-owned HTTP peer — the wire recorder — stands at the vadis's immediate neighbour on a live run. It
answers nothing: it records the bytes it receives, forwards them to a declared forward target (the real
upstream), and relays that target's own response back. The counterparty of record becomes the forward target.**

The owner's ruling is the authority for this decision, quoted verbatim (English gloss in brackets):

> 按照你的建议，只用 harness
>
> ["As you suggested — harness only."]

Recorded 2026-09-23 08:25 in the loop state record's waiting-on-human **row 13** (the cell is that row;
the same ruling's summary paragraph sits beside it), closing the question the loop's verdict record left open
with **option (a)**. The row's own statement of its effect:

> the **harness moves and only the harness** … the product's serving path is **not** touched (option (b) — keylog /
> trust override / outbound tap — is refused) and the boundary is **not** accepted as permanent (option (c) is
> refused). **ADR-028 is authorised and belongs to the round that implements it.**

What that fixes, and how this ADR disposes of each part:

1. **The five movements the ruling names** are the contract's rev-6 amendments: PROV-8's derivation gains a
   **derived and printed** recorder witness — its listen address, its declared forward target, its record count and
   its sha256 (§9.1 **W4**, §9.4 property 1); **the counterparty of record becomes the forward target**
   (ARM-3.8.4); **W2's registry learns mock-vs-recorder** without either reader changing shape (ARM-3.11.3, §9.1.2);
   **§9.2's sentence** is re-written (§9.2 rev 6); **VER-4.4's condition 0/1 sentences** are re-read (VER-4.4 rev 6);
   and **ARM-3.4 rev 3's "no listener on a live run"** is re-worded so that a byte-preserving recorder is **not** a
   mock while a fixture-answering listener stays forbidden (ARM-3.4 rev 6, the rev-3 sentence quoted verbatim).
2. **The two guards the ruling makes part of the freeze** are ARM-3.11.4: byte preservation is **asserted by
   construction** (one buffer: the bytes written to the record are the bytes handed to the socket; the response is
   relayed streaming with both stored files recomputable by a stranger) **and cross-checked** (the vadis's own
   `usage` against the usage parsed from the recorded response bytes — the independent half); and the
   **declaration and the digest are printed on every row** (VER-4.1.1's `wire_recorder` field: listen, forward map,
   record count, per-request digests, one set digest, and the target's TLS peer certificate).
3. **The honesty cost binds every artefact that uses the recorder.** With it in place, `external` means
   *non-loopback address, no mock answered, and a declared byte-preserving recorder answered for the bytes* — and any
   `verified` figure minted on such a run must say so **in the same breath as R23-F6's delegated signature** and must
   state that its budget confirmation was the **standing authorisation**, not a same-minute "go". §9.2 rev 6 carries
   the re-written sentence, and §16.3 records that rev 6 mints no `verified` figure and makes none reachable.
4. **What the ADR adds, out loud, because a reviewer must weigh it:** the instrument **no longer rests on the
   subject's cooperation** — the product is not modified, not configured and not even aware — while the *bytes* are
   now witnessed by **harness code** rather than by the product. The observer moved from "inside the serving path,
   trusted" to "outside it, in the same repository as the judge". That is a real reduction in independence, and
   §9.2 rev 6's limit plus §16.4 item 9's obligation package are how it is bounded rather than waved away.

## Alternatives considered, and why each was refused

| Option | What it would have bought | Why it is refused |
|---|---|---|
| **(b) observe from inside the product** — a keylog, a CA/trust override, or an outbound byte tap in `crates/` | `O` obtained directly, at the exact socket the conformance rule talks about; no second process, no forward target to declare | Puts an observation surface in the serving path for the sole purpose of easing its own measurement (AGENTS constraint 3), ships it to every user, and in the TLS case requires the harness to hold a key the product trusts. The owner refused it explicitly (the loop state record), and ADR-015's two permitted mutations are the only product-side changes this repository allows. |
| **(c) declare the live byte audit permanently unrunnable** | Zero new apparatus; the honest blank stays honest | The owner refused it explicitly. It also has a cost this repository has already paid for four rounds: a condition that *cannot* run is a gate that silently stops being read, and VER-4.4.1's own rule (an absence never supplies a verdict) then makes the whole live path unpublishable rather than merely unverified. |
| **packet capture** (a passive tap beside the wire) | No peer in the path at all; nothing to forward, nothing to declare | It sees **ciphertext**. With `http2` enabled there is not even a request line to read — CAP-1.7's own judgement of 2026-09-22, made when the first capture path was rejected. A capture is evidence about *volume*, never about *body bytes*, which is what ARM-3.8 audits. |
| **a recorder behind an IPC/plugin schema** (a second process, harness and recorder in different images) | The appearance of process separation | No added independence: the same repository writes both sides, so the trust boundary is identical while the surface to freeze grows (a schema, a protocol version, a lifecycle). ARM-3.11.7 records the refusal. |
| **let a mock stand at the peer on a live run** (a "live-shaped" arm answered by fixtures) | The byte audit runs unchanged | It is exactly what ARM-3.4.4 forbids: a mock number presented as a provider's report. A run with a fixture at the peer is a mock run and must say so — §9.1.2 rev 6 makes that precedence mechanical (`mock` outranks every other witness). |

## Trade-offs

**Gained.**

- The live path's byte audit becomes **runnable**: condition 3 can carry a real verdict on a live wire, from the
  bytes the vadis actually sent, instead of `not_run` forever.
- `O` gains a **defensible provenance**: it is what a declared target received, recorded by the process that read
  it off the socket, with a digest pair printed on the row.
- One witness in the chain is **not harness-authored**: the forward target's TLS peer certificate
  (ARM-3.11.4 item 4) names an identity the harness had to verify against the ordinary trust store — no override,
  no injected CA.
- The layout keeps the two peers **mechanically disjoint** (ARM-3.11.3: different directory, different extension,
  plus an asserted `zero` for W2's own glob), so the derivation's most load-bearing distinction — a mock answered vs
  a recorder recorded — is not a matter of interpretation.
- Failures **fail closed and loud**: a recorder that cannot bind is a preflight refusal (exit `2`); a forward that
  refuses, times out or truncates is an item-level non-measurement with a named class (exit `4`); a recorder with
  any failed forward reads `mock` (§9.1.2 rev 6) — so a failing recorder can never be mistaken for an external
  counterparty, and no label can rise on a failure.

**Sacrificed.**

- **Independence, as stated above.** The bytes are witnessed by the same repository that judges them. R30-3's
  obligation package (§16.4 item 9: offline digest recomputation, the certificate check, the vendor's own usage
  relation, and negative controls) is the mitigation, not a cure.
- **A new apparatus surface**: one declaration block, one process per (arm, repeat), one artifact directory, one
  field on the row, and four check ids — all of which a reader must now know. ARM-3.11 is written so a reader who
  does not know them still reads every older row correctly (an absent `wire_recorder` field means "no recorder",
  never `external` by itself).
- **The forwarded evidence is a digest pair, not a file**: because the forwarded request bytes carry the
  credential, no `NNNNN.fwd.req` exists (ARM-3.11.4 item 6). The audit reads the **body**, which is exact; a reader
  who wanted the forwarded request *as bytes on disk* cannot have it, and that is deliberate.
- **The records are gitignored** (`.gitignore:13`, since they live under the run root). Only the printed witness
  survives a clone — which is why ARM-3.11.1 makes the printed witness load-bearing rather than decorative.
- **A residual hole, named and not closed**: a declared forward target that is itself harness-owned apparatus at a
  non-loopback address is not excluded by this contract. The certificate witness (§9.2 rev 6's limit) is what a
  reader checks it against, and nothing here closes the gap by construction.

## Consequences

- **What becomes possible:** a live arm whose condition 3 carries three real verdicts. That is a *condition*, not a
  measurement: the frozen corpus's own `exclusion_note` still bounds any delta, D3 stays unmet, and this round mints
  **no** `verified` figure.
- **What remains not established:** the recorder's byte preservation is established about the **body**, not about
  the HTTP envelope (ARM-3.11.4 item 5 discloses the re-addressing); the counterparty's identity rests on the
  certificate witness; and the independence of the whole chain rests on R30-3's packet, not on this ADR.
- **What this ADR does not do:** it does not touch `crates/`, `tests/`, `docs/`, `book/`, `README.md`, the corpus,
  the loop charter, the loop execution model, any threshold, the L1 envelope or `tests/conformance/`. It does not make the
  recorder reachable from the product, and it does not widen the mock path in any particular.
- **Reversibility:** **high, and cheap in the direction that matters.** The recorder is optional by construction —
  `[wire_recorder]` absent means every pre-rev-6 behaviour holds unchanged (ARM-3.11.1) — so removing it, or
  refusing it for a given round, restores the live path's previous (blank, honest) state with no migration, no
  artifact to retire and no contract clause to unwind beyond the rev-6 markers themselves. What is **not**
  reversible by a later round's own decision is the direction of travel: once a `verified` figure is minted on a
  recorder-mediated run, the honesty cost of item 3 above is permanent in that figure's sentence — which is why no
  figure is minted here.

## Publication note (2026-10-03, R62-2)

The `R<n>` labels and finding ids in this document name iterations of the project's own
private analysis loop — a loop that is not part of this repository, so no label here is
resolvable by a reader of it; they are kept as the provenance of the decision. This
publication pass removed only the dead-pointer class: every reference into that loop's
working tree (its file paths and round-record names, its state record, charter, execution
model and replay contract, its scripts and module names, and the kanban card ids), each
replaced by the neutral phrase the sentence needs. Nothing else moved — no figure,
threshold, `§`/`ADR`/`CONF` id, code sample or contract sentence.
