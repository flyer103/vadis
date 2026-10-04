use vadis_core::config::{PlanPolicyCfg, RecoveryMode, VadisConfig};
use vadis_core::cost::Nano;
use vadis_core::plan::{PlanAccount, PlanFirstRule, PlanRequest, PlanStateRow, ProbeBlockedBy};
use vadis_core::store::{Query, QueryRow, Store};
use serde_json::{json, Map, Value};

/// What `serve` actually loaded from one config file (CONF-25): built by
/// `vadis-cli` at startup after validation, read by `/health`.
#[derive(Clone)]
pub struct AppState {
    /// The published revision's handle (ADR-040 D2; DESIGN §12.20):
    /// `/health` captures it **once per call** and reports the revision
    /// **in force** — after an accepted reload, this surface answers for
    /// the configuration actually serving, not the one the process
    /// started with. The per-revision facts (the config itself, its
    /// identity, the provider key facts) live on the `Revision`; the
    /// members below are the process-level facts a revision switch
    /// refuses to move (D5).
    pub revision: crate::revision::SharedRevision,
    /// `<config dir>/<trace.dir>` (spec §4.1 resolution rule).
    pub trace_dir: String,
    /// `<config dir>/state/router.db` (spec §4.5; fixed in v0.1).
    pub state_db: String,
    /// The store `serve` opened: spec §9.1's `plan` section reads the
    /// `plan_state` projection (and the two inputs of the probe gate that
    /// are projections) through it — read-only queries on the writer's
    /// own connection. `None` in assemblies without a store; the section
    /// then reports a family that never switched.
    pub store: Option<std::sync::Arc<dyn Store>>,
}

/// `/health`'s `config` member's data (spec §9.1's five keys): the byte
/// digest identity of the loaded pair, computed once by the loader.
/// `roster_path: None` and `roster_sha16: ""` are the two spellings of
/// one fact — the roster is inline (§9.1: a path that does not exist is
/// null; a hash input that is not there is the empty string).
#[derive(Clone)]
pub struct ConfigIdentity {
    /// The root file's resolved path, spelled the way §4.12 selected it.
    pub root_path: String,
    /// The roster's resolved path, or `None` when the roster is inline.
    pub roster_path: Option<String>,
    /// First 16 hex chars of SHA-256 over the root file's bytes.
    pub root_sha16: String,
    /// The same over the roster file's bytes; "" when the roster is inline.
    pub roster_sha16: String,
    /// `sha16(root_sha16 + ":" + roster_sha16)` — the same value the trace
    /// rows (§6) and the `config.applied` event carry.
    pub config_digest: String,
}

/// One provider entry's operator-facing facts (spec §9.1's provider list,
/// §4.8's two new members): name, key variable, key presence, and the
/// declared region and currency — all reads of the loaded config.
#[derive(Clone)]
pub struct ProviderKeyFacts {
    pub name: String,
    pub api_key_env: String,
    pub present: bool,
    pub region: vadis_core::config::Region,
    pub currency: vadis_core::Currency,
}

/// `/health` reports what was actually loaded (DESIGN §12.10.2): the plugin set with `disabled` shown as disabled, each
/// provider's key presence, the resolved trace/state paths, and the store
/// status — `open` (the store is a startup prerequisite: a process that
/// could not open it refuses to serve, CONF-23). Spec §9.1 adds the `plan`
/// member.
pub fn health_json(state: &AppState) -> Value {
    // One capture per call (the capture-once rule, ADR-040 D2): the whole
    // body below answers for the revision in force at this request's
    // arrival — the config member and the plan section cannot disagree
    // about which revision they describe.
    let rev = state.revision.capture();
    let config = &rev.forwarder.config;
    let mut plugins = Vec::new();
    for p in &config.plugins {
        let mut entry = Map::new();
        entry.insert("id".into(), json!(p.id));
        entry.insert("kind".into(), json!(p.kind));
        if p.disabled {
            entry.insert("disabled".into(), Value::Bool(true));
        }
        plugins.push(Value::Object(entry));
    }

    let providers: Vec<Value> = rev
        .provider_keys
        .iter()
        .map(|f| {
            json!({
                "name": f.name,
                "api_key_env": f.api_key_env,
                "api_key_present": f.present,
                "available": f.present,
                // spec §4.8: the two facts an operator needs when one
                // vendor appears in the roster twice — declared, read
                // from the loaded config, never inferred from the host
                // or from each other.
                "region": f.region.as_str(),
                "currency": f.currency.as_code(),
            })
        })
        .collect();

    json!({
        "status": "ok",
        "addr": config.server.addr,
        "plugins": plugins,
        "providers": providers,
        "trace_dir": state.trace_dir,
        "state_db": state.state_db,
        // Spec §9.1 / ADR-037 D6: which configuration this process loaded
        // — the two resolved paths and the three digests, so "which
        // revision is this process serving?" is answered by the surface.
        // The two spellings of one fact: `roster_path: null` and
        // `roster_sha16: ""` both mean the roster is inline.
        "config": {
            "root_path": rev.config_identity.root_path,
            "roster_path": rev.config_identity.roster_path,
            "root_sha16": rev.config_identity.root_sha16,
            "roster_sha16": rev.config_identity.roster_sha16,
            "config_digest": rev.config_identity.config_digest,
        },
        // Spec §9.1 / §4.7: `{"required": true, "env": "<name>"}` when the
        // key is written, `{"required": false}` — and no other key — when
        // it is not. Derived from the loaded config (the one source; no
        // second copy of the resolution logic to drift). The variable's
        // NAME may be reported; its value is printed nowhere, ever.
        "auth": auth_member(&config.server),
        // The store is a startup prerequisite: if it could not open, serve
        // would have exited non-zero (CONF-23), so a running process reports
        // "open" — the refusal reason never reaches /health.
        "store": "open",
        // Spec §9.1. `{"configured": false}` — and nothing else — when the
        // loaded config declares no `plan_policy`: with no policy there is
        // no family to name, and inventing one would be the "a state nobody
        // can see" error in reverse.
        "plan": plan_section_value(state, &rev),
    })
}

// ---------------------------------------------------------------------------
// Spec §9.1: `GET /health`'s `plan` section
// ---------------------------------------------------------------------------

/// The inputs the section needs beyond the config itself — every field is a
/// projection read or a clock read, none is computed here (the same purity
/// stance as `PlanFirstRule`, AGENTS constraint 2).
pub(crate) struct PlanHealthInputs {
    pub account: PlanAccount,
    /// The last transition's instant; 0 for a family that never switched.
    pub since_us: i64,
    pub now_us: i64,
    /// ADR-011's answer for the primary route right now.
    pub primary_allowed: bool,
    /// The local counter's only influence (spec §4.6 rule 3).
    pub deferred_by_window: bool,
}

fn plan_section_value(state: &AppState, rev: &crate::revision::Revision) -> Value {
    let Some(policy) = rev.forwarder.config.plan_policy.as_ref() else {
        return json!({ "configured": false });
    };
    let inputs = gather_plan_inputs(state, &rev.forwarder.config, policy);
    plan_section(policy, &inputs)
}

/// Read the projections the section reports (spec §9.1's per-key semantics):
/// the `plan_state` row (absent ⇒ `primary`, §4.6), ADR-011's cooldown for
/// the primary provider, and the quota counter's window verdict. The last
/// two are `availability`'s single-owner reads (ADR-016 §13.3 L1c/L1d,
/// fixed R10) — this surface consumes them, it does not re-derive them —
/// both evaluated against the section's one clock read below.
fn gather_plan_inputs(
    state: &AppState,
    config: &VadisConfig,
    policy: &PlanPolicyCfg,
) -> PlanHealthInputs {
    let now = now_us();
    let (account, since_us) = match state.store.as_ref().map(|s| {
        s.query(Query::PlanState {
            family: &policy.family,
        })
    }) {
        Some(Ok(QueryRow::PlanState(Some(row)))) => (
            if row.account == "overflow" {
                PlanAccount::Overflow
            } else {
                PlanAccount::Primary
            },
            row.since_us,
        ),
        _ => (PlanAccount::Primary, 0),
    };
    PlanHealthInputs {
        account,
        since_us,
        now_us: now,
        primary_allowed: !crate::availability::provider_in_cooldown(
            state.store.as_ref(),
            &policy.primary.provider,
            now,
        ),
        deferred_by_window: crate::availability::probe_deferred_by_window(
            state.store.as_ref(),
            config,
            policy,
            (now.max(0) as u64) / 1_000_000,
        ),
    }
}

/// The section itself, pure over (config, inputs) so the shape is unit-testable
/// without a store. Every key is a read or an arithmetic on two known instants
/// (`since + cooldown`); no figure here is an estimate (spec §9.1's closing
/// rule, AGENTS constraint 5).
pub(crate) fn plan_section(policy: &PlanPolicyCfg, i: &PlanHealthInputs) -> Value {
    let rule = PlanFirstRule::new(policy.clone());
    let recover = if policy.recover == RecoveryMode::None {
        "none"
    } else {
        "probe"
    };
    let common = |account: &str, since: Value, probe: Value| {
        json!({
            "configured": true,
            "family": policy.family,
            "primary": route_str(&policy.primary),
            "overflow": route_str(&policy.overflow),
            "recover": recover,
            "account": account,
            "since": since,
            "probe": probe,
        })
    };
    if i.account == PlanAccount::Primary {
        // A family on its primary has nothing to probe back to (§4.6 rule 2:
        // the probe IS the way back from `overflow`). `since` is the ts of
        // the `plan.switched` row that produced the CURRENT state (§9.1's
        // letter): null only for a family that never switched (the absent
        // `plan_state` row above) — a recovered family keeps the recovery
        // row's ts, not a reset to null.
        let since = if i.since_us > 0 {
            rfc3339_millis(i.since_us)
                .map(Value::String)
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        return common("primary", since, Value::Null);
    }
    // Overflow: `probe.deadline` is recomputed from the **currently loaded**
    // cooldown against `since_us` — the same recomputation the serving path
    // does (ADR-014 item 10's mid-flight clause) — never the stored
    // informational `until_us`, which a rebuild may have derived from a
    // cooldown that has since changed. Always computable when `probe` is
    // present, so no unprintable number exists here.
    let deadline_us = i.since_us.saturating_add(policy.cooldown_us());
    // `blocked_by` is the guard's own answer, not a re-derivation here
    // (ADR-016 §13.3 L1a: `PlanFirstRule::probe_admitted` is the single
    // owner of the evaluation order). The request the section renders is
    // the surface's reduced one: a fresh session at `turn_index == 1` —
    // the only request shape the probe gate could still admit — so the
    // two request-shaped arms (`NoSession`, `NotSessionBoundary`) never
    // fire and get no word (§9.1's table), and `blocked_by_surface_word`
    // maps every arm that does fire to the surface's vocabulary.
    let blocked_by = rule
        .probe_admitted(&surface_request(i))
        .err()
        .and_then(ProbeBlockedBy::blocked_by_surface_word);
    let probe = json!({
        "deadline": rfc3339_millis(deadline_us),
        "admitted": blocked_by.is_none(),
        "blocked_by": blocked_by,
    });
    common(
        "overflow",
        rfc3339_millis(i.since_us)
            .map(Value::String)
            .unwrap_or(Value::Null),
        probe,
    )
}

fn route_str(r: &vadis_core::config::RouteSpec) -> String {
    format!("{}/{}", r.provider, r.model)
}

/// The reduced request the section renders the guard's answer for: the
/// surface describes a family, not a request, so the request is the
/// shape the probe gate could still admit — a fresh session at
/// `turn_index == 1` on the overflow account (the section's `probe`
/// member exists only there). With that choice the two request-shaped
/// arms of `probe_admitted` never fire, and every arm that does fire is
/// one §9.1 names — the guard's own evaluation order, consumed, not
/// re-derived (ADR-016 §13.3 L1a). The guard reads the cooldown through
/// the same `PlanPolicyCfg::cooldown_us` the deadline above used (L1b).
fn surface_request(i: &PlanHealthInputs) -> PlanRequest<'_> {
    PlanRequest {
        session: Some(""),
        turn_index: 1,
        state: PlanStateRow {
            account: i.account,
            since_us: i.since_us,
        },
        now_us: i.now_us,
        primary_allowed: i.primary_allowed,
        deferred_by_window: i.deferred_by_window,
        overflow_spend: Nano(0),
    }
}

/// `/health`'s `auth` member (spec §9.1, DESIGN §12.11).
fn auth_member(server: &vadis_core::config::ServerCfg) -> Value {
    match server.auth_token_env.as_deref() {
        Some(name) => json!({ "required": true, "env": name }),
        None => json!({ "required": false }),
    }
}

fn now_us() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

/// RFC3339 UTC with millisecond precision (spec §9.1's `since` format).
/// `None` for non-positive instants (a family that never switched).
fn rfc3339_millis(us: i64) -> Option<String> {
    if us <= 0 {
        return None;
    }
    let s = (us / 1_000_000) as u64;
    let ms = ((us % 1_000_000) / 1_000) as u64;
    let (y, mo, d, minute, _) = vadis_core::peak::timestamp_parts(s, vadis_core::peak::Tz::Utc);
    let (h, mi) = (minute / 60, minute % 60);
    let sec = s % 60;
    Some(format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{sec:02}.{ms:03}Z"
    ))
}

// ADR-016 §13.3 L1c/L1d, fixed R10: the route-availability read and the
// local counter's window verdict each had a second copy here, mirroring
// `Forwarder`'s private helpers. Both are deleted — the section (and the
// request path's `plan_guard`) calls `availability`'s single-owner
// functions; CONF-75..78 witness the two consumers agreeing on the live
// path.

pub(crate) fn not_implemented_body(
    request_id: String,
    endpoint: &'static str,
) -> vadis_core::error::ErrorBody {
    vadis_core::error::ErrorBody::new(
        vadis_core::error::ErrorCode::NotImplemented,
        format!("endpoint {endpoint} is not implemented in v0.1"),
        request_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use vadis_core::config::{CapUsdVal, DurationVal, OnPrimaryExhausted, RouteSpec};

    fn policy(cooldown_ms: u64, recover: RecoveryMode) -> PlanPolicyCfg {
        PlanPolicyCfg {
            family: "m1".into(),
            primary: RouteSpec {
                provider: "p-plan".into(),
                model: "m1".into(),
            },
            overflow: RouteSpec {
                provider: "p-api".into(),
                model: "m1".into(),
            },
            on_primary_exhausted: OnPrimaryExhausted::Spill,
            recover,
            cooldown: DurationVal(cooldown_ms),
            overflow_monthly_cap_usd: Some(CapUsdVal(20.0)),
        }
    }

    fn inputs(account: PlanAccount, since_us: i64) -> PlanHealthInputs {
        PlanHealthInputs {
            account,
            since_us,
            now_us: since_us + 60 * 1_000_000, // a minute past the transition
            primary_allowed: true,
            deferred_by_window: false,
        }
    }

    /// An AppState fixture around a `RevisionCell` — the shape `serve`
    /// builds: the config and its identity inside the published revision.
    fn app_state(config: VadisConfig, identity: ConfigIdentity) -> AppState {
        AppState {
            revision: crate::revision::RevisionCell::new(crate::revision::Revision {
                forwarder: crate::forward::Forwarder {
                    config,
                    transports: Default::default(),
                    api_keys: Default::default(),
                    store: None,
                    trace: None,
                    transform_engine: None,
                    response_cache: None,
                    session_ttl_us: 0,
                },
                config_identity: identity,
                provider_keys: Vec::new(),
                auth_gate: None,
            }),
            trace_dir: String::new(),
            state_db: String::new(),
            store: None,
        }
    }

    // §9.1's `configured: false` row: NO other key is present.
    #[test]
    fn no_policy_is_exactly_configured_false() {
        let state = app_state(no_policy_config(), test_identity(None));
        let v = plan_section_value(&state, &state.revision.capture());
        assert_eq!(v, json!({ "configured": false }));
    }

    /// A `ConfigIdentity` fixture: the inline shape when `roster` is None,
    /// the split shape when it names a roster path.
    fn test_identity(roster: Option<&str>) -> ConfigIdentity {
        ConfigIdentity {
            root_path: "/cfg/config.yaml".into(),
            roster_path: roster.map(str::to_string),
            root_sha16: "0123456789abcdef".into(),
            roster_sha16: roster
                .map(|_| "fedcba9876543210".into())
                .unwrap_or_default(),
            config_digest: "0011223344556677".into(),
        }
    }

    // Spec §9.1's `config` member: the five keys, with the inline shape's
    // two spellings of one fact — `roster_path: null` and
    // `roster_sha16: ""` — and the split shape's named roster.
    #[test]
    fn the_config_member_reports_what_was_loaded() {
        let state = |roster: Option<&str>| app_state(no_policy_config(), test_identity(roster));
        let inline = health_json(&state(None));
        assert_eq!(
            inline["config"],
            json!({
                "root_path": "/cfg/config.yaml",
                "roster_path": Value::Null,
                "root_sha16": "0123456789abcdef",
                "roster_sha16": "",
                "config_digest": "0011223344556677",
            }),
            "the inline shape: null path, empty roster half"
        );
        let split = health_json(&state(Some("/cfg/providers.yaml")));
        assert_eq!(split["config"]["roster_path"], "/cfg/providers.yaml");
        assert_eq!(split["config"]["roster_sha16"], "fedcba9876543210");
        assert_eq!(
            split["config"]["config_digest"], inline["config"]["config_digest"],
            "the member reports the value it is handed, unchanged"
        );
    }

    #[test]
    fn primary_account_has_null_probe_and_null_since() {
        let p = policy(900_000, RecoveryMode::Probe);
        let v = plan_section(&p, &inputs(PlanAccount::Primary, 0));
        assert_eq!(v["configured"], true);
        assert_eq!(v["family"], "m1");
        assert_eq!(v["primary"], "p-plan/m1");
        assert_eq!(v["overflow"], "p-api/m1");
        assert_eq!(v["recover"], "probe");
        assert_eq!(v["account"], "primary");
        assert_eq!(v["since"], Value::Null, "never switched ⇒ null");
        assert_eq!(v["probe"], Value::Null, "nothing to probe back to");
    }

    // §9.1's letter: `since` is "the ts of the plan.switched row that
    // produced the current state", null ONLY for a family that never
    // switched. A recovered family's current state was produced by the
    // recovery row — so `since` is that row's ts (plan_state keeps it),
    // and `probe` is still null (nothing to probe back to from primary).
    #[test]
    fn recovered_primary_keeps_the_recovery_row_ts_and_null_probe() {
        let p = policy(900_000, RecoveryMode::Probe);
        let recovery_ts_us = 1_760_000_000_000_000i64;
        let v = plan_section(&p, &inputs(PlanAccount::Primary, recovery_ts_us));
        assert_eq!(v["account"], "primary");
        let since = v["since"].as_str().expect("since is the recovery row's ts");
        assert_eq!(
            since, "2025-10-09T08:53:20.000Z",
            "the stored since_us, formatted — not a reset to null"
        );
        assert_eq!(
            v["probe"],
            Value::Null,
            "primary still has nothing to probe"
        );
    }

    // The card's own acceptance shape: deadline == since + cooldown, parsed
    // back from the two RFC3339 strings rather than trusted.
    #[test]
    fn deadline_is_since_plus_current_cooldown() {
        let since_us = 1_760_000_000_000_000i64; // arbitrary instant
        let p = policy(15 * 60 * 1_000, RecoveryMode::Probe);
        let v = plan_section(&p, &inputs(PlanAccount::Overflow, since_us));
        assert_eq!(v["account"], "overflow");
        let since = v["since"].as_str().expect("since is a string on overflow");
        let deadline = v["probe"]["deadline"].as_str().expect("deadline");
        assert_eq!(
            parse_ms(deadline) - parse_ms(since),
            15 * 60 * 1_000,
            "deadline == since + the currently loaded cooldown"
        );
        // A minute past the transition is inside the 15m cooldown.
        assert_eq!(v["probe"]["admitted"], false);
        assert_eq!(v["probe"]["blocked_by"], "cooldown");
    }

    #[test]
    fn blocked_by_follows_the_guard_order() {
        let since_us = 1_760_000_000_000_000i64;
        // Cooldown elapsed, but ADR-011 refuses the primary.
        let p = policy(0, RecoveryMode::Probe);
        let mut i = inputs(PlanAccount::Overflow, since_us);
        i.primary_allowed = false;
        let v = plan_section(&p, &i);
        assert_eq!(v["probe"]["blocked_by"], "primary_cooling_down");
        // Primary healthy, but the plan's window has not reset.
        let mut i = inputs(PlanAccount::Overflow, since_us);
        i.deferred_by_window = true;
        let v = plan_section(&p, &i);
        assert_eq!(v["probe"]["blocked_by"], "window_not_reset");
        // Everything holds: admitted, blocked_by null.
        let v = plan_section(&p, &inputs(PlanAccount::Overflow, since_us));
        assert_eq!(v["probe"]["admitted"], true);
        assert_eq!(v["probe"]["blocked_by"], Value::Null);
        // recover: none wins over everything (evaluation order's first arm).
        let p_none = policy(0, RecoveryMode::None);
        let v = plan_section(&p_none, &inputs(PlanAccount::Overflow, since_us));
        assert_eq!(v["recover"], "none");
        assert_eq!(v["probe"]["blocked_by"], "recovery_disabled");
        // And cooldown precedes primary_cooling_down: a demoted primary
        // inside the cooldown still reports `cooldown` (the first failing
        // condition is the honest answer).
        let p15 = policy(15 * 60 * 1_000, RecoveryMode::Probe);
        let mut i = inputs(PlanAccount::Overflow, since_us);
        i.primary_allowed = false;
        let v = plan_section(&p15, &i);
        assert_eq!(v["probe"]["blocked_by"], "cooldown");
    }

    #[test]
    fn rfc3339_anchors() {
        // 2026-09-19T12:34:56Z is 1_789_821_296 (the store crate's own anchor).
        let base_s: i64 = 1_789_821_296;
        assert_eq!(
            rfc3339_millis(base_s * 1_000_000).as_deref(),
            Some("2026-09-19T12:34:56.000Z")
        );
        assert_eq!(
            rfc3339_millis(base_s * 1_000_000 + 789_000).as_deref(),
            Some("2026-09-19T12:34:56.789Z")
        );
        // Leap day, at whole milliseconds.
        assert_eq!(
            rfc3339_millis(951_868_799_000_000).as_deref(),
            Some("2000-02-29T23:59:59.000Z")
        );
        assert_eq!(rfc3339_millis(0), None, "never-switched sentinel");
    }

    /// "YYYY-MM-DDTHH:MM:SS.mmmZ" → epoch milliseconds (test-side parser,
    /// independent of the formatter under test).
    fn parse_ms(ts: &str) -> i64 {
        let (date, rest) = ts.split_once('T').expect("T");
        let (time, _) = rest.split_once('.').expect("millis");
        let mut d = date.split('-');
        let y: i64 = d.next().unwrap().parse().unwrap();
        let mo: u32 = d.next().unwrap().parse().unwrap();
        let day: u32 = d.next().unwrap().parse().unwrap();
        let mut t = time.split(':');
        let h: i64 = t.next().unwrap().parse().unwrap();
        let mi: i64 = t.next().unwrap().parse().unwrap();
        let s: i64 = t.next().unwrap().parse().unwrap();
        let ms: i64 = rest
            .split_once('.')
            .unwrap()
            .1
            .trim_end_matches('Z')
            .parse()
            .unwrap();
        vadis_core::peak::utc_midnight_epoch(y, mo, day) as i64 * 1_000
            + h * 3_600_000
            + mi * 60_000
            + s * 1_000
            + ms
    }

    /// A config with no `plan_policy` — only the fields the section reads;
    /// loader-level validation is vadis-cli's concern.
    fn no_policy_config() -> VadisConfig {
        use vadis_core::config::{
            BreakevenCfg, CacheCfg, PluginCfg, VadisConfig, ServerCfg, SessionCfg, TraceCfg,
        };
        use std::collections::BTreeMap;
        VadisConfig {
            server: ServerCfg {
                addr: "127.0.0.1:0".into(),
                upstream_attempt_timeout: DurationVal(60_000),
                request_timeout: DurationVal(600_000),
                auth_token_env: None,
                max_body_bytes: 2_097_152,
            },
            session: SessionCfg {
                key_sources: vec!["prompt_cache_key".into()],
                ttl: DurationVal(43_200_000),
            },
            cache: CacheCfg {
                sticky: true,
                breakeven: BreakevenCfg {
                    enabled: true,
                    min_remaining_turns: 2,
                    safety_factor: vadis_core::config::MultiplierVal(1.1),
                },
            },
            trace: TraceCfg {
                dir: "./state/traces".into(),
                rollover: vadis_core::config::Rollover::Hourly,
            },
            providers: Vec::new(),
            aliases: BTreeMap::new(),
            plugins: Vec::<PluginCfg>::new(),
            fallback: Vec::new(),
            plan_policy: None,
            state: None,
        }
    }
}
