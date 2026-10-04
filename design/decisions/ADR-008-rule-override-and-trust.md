# ADR-008 — the three-level rule-file override (the first hit takes effect) and the trust gate (not enabled in v0.1)

- Status: accepted
- Date: 2026-09-19
- Related: ADR-003 (a declarative, revertible, individually accounted transform pipeline); spec §4.4; DESIGN §12.9 Q4; `rules/tool_output.toml`

## Background

Rules are data (ADR-003): compression policy is carried by TOML rule files, and `builtin/transform_rules`
loads and executes them. Two questions must be answered: **when several rule files exist at once, which one
takes effect**; and **do rules need an rtk-style trust gate** (a rule file from an untrusted source must be
explicitly trusted before it executes).

Both questions were "incident-grade" in the old project: when rule sources are heterogeneous, adopting merge
semantics turns "which rule is currently in effect" into an undeducible question, and any cost change loses
its attribution chain; while trusting any source blindly makes a rule file a channel for arbitrarily
rewriting outbound bytes (a direct break of the prefix cache).

## Decision

1. **Lookup order (three levels)**: `.vadis/rules.toml` (project) → `~/.config/vadis/rules.toml` (user) →
   builtin (compiled in). **The first hit takes effect** (first match wins).
2. **No merging, no per-rule override**: the selected file is the whole rule set; a missing rule must either
   be provided explicitly at an earlier position or be added to that earlier file by a PR. Deep merge /
   key-level override is forbidden — the "effective rule set" must be readable by opening one file.
3. **v0.1 does not enable the trust gate**: rule files are treated as "local repository assets" (the same
   level as config and code, in the same trust domain); no signature/origin check before loading, and no
   explicit trust required.
4. **A load failure must be explicit**: if any rule's inline test fails, a regex fails to compile or the DSL
   semantics fail → **that rule is not loaded**, it is reported in the startup log and in `/health`, and the
   remaining rules work as usual; a runtime failure falls back to the original text per spec §8 fail-safe
   and records `errors[].kind = transform_error` (never half-rewrite, never silently).
5. **Trigger conditions for re-evaluating the trust gate** (any one holding means it must be re-evaluated;
   this decision must not be carried over by default):
   - a rule file appears whose source is a **non-local author** (a download, a third-party repository,
     someone else's share);
   - a rule file enters a **network/shared distribution** path (packaged distribution, remote sync, rules
     shipped with a plugin);
   - a concrete case appears where "rules can affect outbound bytes but the source is untrusted" (e.g.
     transform rules in a plugin marketplace).
   The minimal evolution is "an origin marker + rejecting unmarked rules"; it is **not allowed** to degrade
   into silent loading.

## Rationale

- First-hit + no merging makes the "effective rule set" a definite, reproducible, auditable function (it is
  the input of `rules verify`), which is the precondition for §4's attributable accounting.
- v0.1's rules can only come from a local author's repository: at this point the trust gate is a
  zero-benefit complexity (every local edit would go through a trust workflow), while the threat it guards
  against (an untrusted source) does not exist in v0.1's deployment shape. **Deferring is a judgement, not
  an omission** — so the trigger conditions are written into the ADR.
- Consistent with the AGENTS hard constraints: policy artifacts are the **only** injection surface of config
  / rule TOML / tier-B plugins, and the artifacts themselves do not enter the serving path's "runtime
  decision", so origin verification should happen at the load boundary rather than at execution time.

## Consequences

- `vadis rules verify` becomes an executable gate before loading: inline tests are a rule's **only** spec
  (same content → same output) and a failure means no load; this makes "the rule was written wrong" show up
  as "the rule did not take effect + an explicit log" instead of "the bytes were changed quietly".
- The three-level order means a project-level file **completely shadows** user-level and builtin rules; a
  user who wants to "override just one rule" must copy the whole file and then edit it — a deliberate
  trade-off (readability > convenience), documented in the header of `rules/tool_output.toml`.
- Once a rule file's source goes beyond the local trust domain, this ADR must be re-adjudicated; any "just
  add a signature check in passing" before that is an unevaluated scope expansion.
- GAP-Q4 (DESIGN §12.9) is closed by this: the override semantics are settled, and the trust gate is ruled
  not enabled + with trigger conditions.
