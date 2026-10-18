# reduce.jq — the paired run's offline reducer (ADR-059 §7.1 step 7).
#
# Input : the eight DecisionRecords of one run (quartet + null quartet),
#         slurped from the instrument's own trace files, e.g.
#           jq -s -f reduce.jq <trace.dir>/*.jsonl \
#             --arg basis measurement \
#             --arg fixture_path design/paired-run/fixture.json \
#             --arg fixture_sha256 <sha256> \
#             --arg fixture_class composed \
#             --arg rule_file_sha256 <sha256 of rules/tool_output.toml> \
#             --arg plugin_config 'builtin/transform_rules, rules_file=<path>' \
#             --arg commit <git rev-parse HEAD> \
#             --arg binary_sha256 <sha256 of the serving binary> \
#             --argjson gates '{"build":true,"test":true,"clippy":true,"fmt":true}' \
#             --argjson porcelain_empty true \
#             --argjson artefacts_consistent true \
#             --argjson body_hashes '<{"<request_id>":"<sha16>",...}> from events upstream.submitted>' \
#             --argjson row_refs '<[{"request_id","file","line"},...]>' \
#             --argjson price_entry '<the route price block + source URL + read date>'
# Output: report.json — either a verified net with P1–P9 observed values and
#         the six provenance artefacts, or the first failing condition.
#
# Ordering note (ADR-059 §8 + the architect's hand-down): P9 is evaluated
# FIRST — identical bytes must read identically or the ruler is broken and
# nothing at all may print (R7). Artefact disagreement (R8) is checked next.
# Then P1..P8 in table order; R4/R5/R6 refuse the money line only.
#
# The records are ordered by identity.event_id (the store's monotonic row id):
# `ts` is second-granular, so same-second rows order arbitrarily.

# ── helpers ──────────────────────────────────────────────────────────────
def ref_of($refs; $id): ($refs | map(select(.request_id == $id)) | first) // {file: null, line: null};
def hash_of($map; $id): $map[$id] // null;
def tsec($t): $t | sub("\\.[0-9]+Z$"; "Z") | fromdateiso8601;

# ── order and split ─────────────────────────────────────────────────────
sort_by(.identity.event_id)
| if length != 8 then
    {schema: "vadis/paired-run-report@1", basis: $basis, verdict: "refused",
     refusal: {id: "R1", condition: "rowcount",
               observed: {records: length,
                          note: "expected exactly 8 records (quartet + null quartet) ordered by identity.event_id"}},
     figures: null, conditions: null, provenance: null, diagnostics: null}
  else
    . as $all
    | $all[0:4] as $q
    | $all[4:8] as $nq
    | $q[2] as $ctrl          # #3 measured control (passthrough, warm)
    | $q[3] as $trmt          # #4 measured treatment (transform, warm)
    | $nq[2] as $nctrl
    | $nq[3] as $ntrmt

# ── observed values, P1..P9 (ADR-059 §5) ────────────────────────────────
    | ([$all[].config_digest] | unique | length) as $n_digests
    | ([$all[].decision.provider] | unique) as $providers
    | ([$all[].decision.model] | unique) as $models
    | ([$all[].decision.key_index] | unique) as $key_indexes
    | [$all[] | select(.result.failover_from != null) | .identity.request_id] as $failovers
    | [$all[] | {r: .identity.request_id, s: .result.status,
                 m: .usage_missing, e: (.errors | length), c: (has("cache"))}] as $measured_ok_rows
    | ([$q[].transform_mode]) as $q_modes
    | ([$nq[].transform_mode]) as $n_modes
    | [$trmt.transforms[]? | .edited_paths[]?] as $edits
    | ($edits | length) as $n_edits
    | ([$ctrl.prefix.blocks[].hash, $trmt.prefix.blocks[].hash]) as $two_block_lists
    | [range(0; ($ctrl.prefix.blocks | length))
       | select($ctrl.prefix.blocks[.].hash != $trmt.prefix.blocks[.].hash)] as $diff_idx
    | ([$q[].cost.currency] | unique) as $currencies
    | ([$q[].cost.peak_applied_pct] | unique) as $peaks
    | [$q[] | select(.cost.quota_after != null or .result.plan_switch != null)
             | .identity.request_id] as $plan_rows
    | ($ctrl.usage.input_cached > 0 and $trmt.usage.input_cached > 0) as $warm
    | ($ctrl.usage.input_cached == 0 and $trmt.usage.input_cached == 0) as $cold
    | ($nq[2].usage.input_total - $nq[3].usage.input_total) as $null_delta
    | (hash_of($body_hashes; $nctrl.identity.request_id) != null
       and hash_of($body_hashes; $nctrl.identity.request_id)
            == hash_of($body_hashes; $ntrmt.identity.request_id)) as $null_bytes_equal

    | {P1: {observed: {config_digests: ($all | map(.config_digest) | unique),
                       one_serve: ($n_digests == 1),
                       porcelain_empty: $porcelain_empty, commit: $commit},
            pass: ($n_digests == 1 and $porcelain_empty and ($commit != "" and $commit != null))},
       P2: {observed: {providers: $providers, models: $models, key_indexes: $key_indexes,
                       failover_rows: $failovers},
            pass: (($providers | length) == 1 and ($models | length) == 1
                   and ($key_indexes | length) == 1 and ($failovers | length) == 0)},
       P3: {observed: {rows: $measured_ok_rows},
            pass: ($measured_ok_rows | all(.s >= 200 and .s < 300 and .m == false
                                           and .e == 0 and .c == false))},
       P4: {observed: {quartet_modes: $q_modes, null_quartet_modes: $n_modes},
            pass: ($q_modes == ["passthrough", "transform", "passthrough", "transform"]
                   and ($n_modes | all(. == "passthrough")))},
       P5: {observed: {edited_nodes: $n_edits, edits: $edits},
            pass: ($n_edits >= 1)},
       P6: {observed: {block_counts: [($ctrl.prefix.blocks | length), ($trmt.prefix.blocks | length)],
                       differing_block_indices: $diff_idx,
                       edited_nodes: $n_edits},
            pass: (($ctrl.prefix.blocks | length) == ($trmt.prefix.blocks | length)
                   and ($diff_idx | length) >= 1 and ($diff_idx | length) == $n_edits)},
       P7: {observed: {currencies: $currencies, peak_applied_pct: $peaks,
                       plan_rows: $plan_rows, entry_account: "api"},
            plan_ok: (($plan_rows | length) == 0),
            money_ok: (($currencies | length) == 1 and ($peaks | length) == 1)},
       P8: {observed: {regime: (if $warm then "warm" elif $cold then "cold" else "mixed" end),
                       control: {input_cached: $ctrl.usage.input_cached,
                                 input_total: $ctrl.usage.input_total},
                       treatment: {input_cached: $trmt.usage.input_cached,
                                   input_total: $trmt.usage.input_total}},
            pass: ($warm or $cold)},
       P9: {observed: {null_control_input_total: $nctrl.usage.input_total,
                       null_treatment_input_total: $ntrmt.usage.input_total,
                       delta: $null_delta,
                       body_hash_pair_equal: $null_bytes_equal,
                       rows: [$nctrl.identity.request_id, $ntrmt.identity.request_id]},
            pass: ($null_delta == 0 and $null_bytes_equal)}}
    as $P

# ── first-fail → refusal (ADR-059 §5.1) ─────────────────────────────────
# P9 first (the ruler), then artefacts (R8), then table order.
    | (if ($P.P9.pass | not) then {id: "R7", condition: "P9", observed: $P.P9.observed}
       elif ($artefacts_consistent | not) then
         {id: "R8", condition: "artefacts",
          observed: {fixture_sha256: $fixture_sha256, note: "artefact digests disagree with the run — see recipe step 7 verification"}}
       elif ($P.P1.pass | not) then {id: "R1", condition: "P1", observed: $P.P1.observed}
       elif ($P.P2.pass | not) then {id: "R1", condition: "P2", observed: $P.P2.observed}
       elif ($P.P3.pass | not) then {id: "R1", condition: "P3", observed: $P.P3.observed}
       elif ($P.P4.pass | not) then {id: "R2", condition: "P4", observed: $P.P4.observed}
       elif ($P.P5.pass | not) then {id: "R2", condition: "P5", observed: $P.P5.observed}
       elif ($P.P6.pass | not) then {id: "R3", condition: "P6", observed: $P.P6.observed}
       else null end) as $hard_refusal

# ── the figures (§4.1): net, input-side only; money per §4.4 ────────────
    | ($ctrl.usage.input_total) as $t_off
    | ($trmt.usage.input_total) as $t_on
    | ($t_off - $t_on) as $net
    | ([$trmt.transforms[]? | (.saved_input_tokens - .added_input_tokens)] | add // 0) as $pred
    | ($net - $pred) as $est_err
    | ($ctrl.cost.input_miss + $ctrl.cost.input_hit + $ctrl.cost.cache_write) as $m_off
    | ($trmt.cost.input_miss + $trmt.cost.input_hit + $trmt.cost.cache_write) as $m_on
    | ($m_off - $m_on) as $m_net
    | (if $P.P7.plan_ok | not then "R4"
       elif ($P.P7.money_ok | not) then "R5"
       elif ($P.P8.pass | not) then "R6"
       else null end) as $money_refusal
    | (if $basis == "measurement" then "verified" else "fixture-arithmetic (plumbing only)" end) as $fig_label

    | {schema: "vadis/paired-run-report@1",
       basis: $basis,
       verdict: (if $hard_refusal != null then "refused" else "figure" end),
       refusal: $hard_refusal,

       figures: (if $hard_refusal != null then null
                 else
                 {net_input_tokens:
                    {value: $net, label: $fig_label, unit: "tokens",
                     scope: "input-side only; net = saved − added; one fixture body, one rule set",
                     from: {request_id: $ctrl.identity.request_id, role: "measured control #3 (passthrough, warm)",
                            usage_input_total: $t_off},
                     to:   {request_id: $trmt.identity.request_id, role: "measured treatment #4 (transform, warm)",
                            usage_input_total: $t_on}},
                  predicted_net_tokens:
                    {value: $pred, label: "inferred",
                     note: "Σ(saved_input_tokens − added_input_tokens) of #4's transforms[] — printed beside, never summed with the net (ADR-031 D4.3)"},
                  estimator_error_tokens:
                    {value: $est_err, label: "diagnostic",
                     note: "net − predicted_net; the first measurement of GAP-Q14's estimator error"},
                  net_input_cost:
                    (if $money_refusal == "R4" then
                       {refused: "R4",
                        reason: "marginal price 0 on this account: a money saving cannot be stated",
                        tokens_still_printed: true}
                     elif $money_refusal == "R5" then
                       {refused: "R5",
                        reason: "cost.currency or cost.peak_applied_pct differ across the pair",
                        observed: {currencies: $currencies, peak_applied_pct: $peaks},
                        tokens_still_printed: true}
                     elif $money_refusal == "R6" then
                       {refused: "R6",
                        reason: "mixed cache regime: the money delta would be mostly the cache",
                        counterfactual_note: "the other regime's money is arithmetic over published prices, printed as a counterfactual only (ADR-031 D3)",
                        tokens_still_printed: true}
                     else
                       {value_nano: $m_net, label: $fig_label, currency: $currencies[0],
                        peak_applied_pct: $peaks[0],
                        scope: "pre-peak tier delta over cost.{input_miss,input_hit,cache_write}; output excluded (§4.3)",
                        m_off_nano: $m_off, m_on_nano: $m_on,
                        from: {request_id: $ctrl.identity.request_id},
                        to: {request_id: $trmt.identity.request_id}}
                     end)}
                 end),

       conditions: $P,

       run: {quartet:
               [$q[0], $q[1], $ctrl, $trmt
                | {request_id: .identity.request_id, event_id: .identity.event_id,
                   ts: .ts, transform_mode: .transform_mode,
                   usage: {input_total: .usage.input_total, input_cached: .usage.input_cached},
                   body_hash: hash_of($body_hashes; .identity.request_id),
                   row_ref: ref_of($row_refs; .identity.request_id)}],
             null_quartet:
               [$nq[0], $nq[1], $nctrl, $ntrmt
                | {request_id: .identity.request_id, event_id: .identity.event_id,
                   ts: .ts, transform_mode: .transform_mode,
                   usage: {input_total: .usage.input_total, input_cached: .usage.input_cached},
                   body_hash: hash_of($body_hashes; .identity.request_id),
                   row_ref: ref_of($row_refs; .identity.request_id)}]},

       provenance:
         {rows: [$all[] | {request_id: .identity.request_id, event_id: .identity.event_id,
                           ts: .ts, body_hash: hash_of($body_hashes; .identity.request_id),
                           file: ref_of($row_refs; .identity.request_id).file,
                           line: ref_of($row_refs; .identity.request_id).line}],
          fixture: {path: $fixture_path, sha256: $fixture_sha256, source_class: $fixture_class},
          rule_set: {rules_file: "rules/tool_output.toml", sha256: $rule_file_sha256,
                     plugin_config: $plugin_config},
          commit_gates: {commit: $commit, porcelain_empty: $porcelain_empty,
                         binary_sha256: $binary_sha256, gates: $gates},
          price_entry: $price_entry},

       diagnostics:
         {cache_regime: $P.P8.observed.regime,
          edited_paths: $edits,
          block_diff: {differing_block_indices: $diff_idx,
                       control_hashes: [$ctrl.prefix.blocks[].hash],
                       treatment_hashes: [$trmt.prefix.blocks[].hash]},
          ts_seconds: [$all[].ts],
          ts_gaps_seconds: ([$all[].ts | tsec(.)] as $ts
                             | [range(1; $ts | length) | ($ts[.] - $ts[. - 1])]),
          note: "ts is second-granular; gaps of 0 mean same-second, not simultaneous"}
    }
  end
