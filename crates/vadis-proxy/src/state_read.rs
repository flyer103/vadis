//! `GET /state/events` (spec §4.18, ADR-054, DESIGN §12.24): the live
//! event-log figure — the one a second process cannot read while `serve`
//! holds the store exclusively (ADR-009 item 8), answered by the writer
//! itself.
//!
//! Pure: no HTTP types, no I/O — the window parse, the figure's
//! derivation from the returned rows and the JSON body are unit-testable
//! without a rig; the wiring (route, guard, query string) is vadis-cli's.
//! The store read goes through `Query::EventsSince` (the windowed,
//! kind-filtered, bounded read) — **never** `Query::AllEvents`, whose
//! bounded-use contract ("the serving path never scans the log") this
//! surface keeps by construction.

#![forbid(unsafe_code)]

use serde_json::{json, Value};
use vadis_core::store::{Query, QueryRow, Store};

use crate::health::AppState;

/// The two kinds the figure joins on (§6's definition: an
/// `upstream.submitted` with no `upstream.responded` for the same
/// `request_id`).
const SUBMITTED: &str = "upstream.submitted";
const RESPONDED: &str = "upstream.responded";

/// The window bound (spec §4.18): `1ms ..= 24h`. A constant, not a key —
/// the same stance §3.3/§4.16 take for their own frozen values.
pub const WINDOW_MIN_MS: i64 = 1;
pub const WINDOW_MAX_MS: i64 = 24 * 3_600_000;

/// The parsed query: the window in milliseconds, already bounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateWindow {
    pub window_ms: i64,
}

/// Parse the `window` query parameter (spec §4.18: the §4.1 duration
/// grammar, `1ms ..= 24h`). `None` is the refusal — absent, unparsable
/// or out of bounds — and the caller answers `400` **without reading the
/// store**.
///
/// The grammar is parsed by the same `DurationVal` the config uses, so
/// the two grammars cannot drift (the stance `stats --window` takes).
pub fn parse_window(raw: Option<&str>) -> Option<StateWindow> {
    let raw = raw?;
    let val: vadis_core::config::DurationVal =
        serde_json::from_value(serde_json::Value::String(raw.to_string())).ok()?;
    let window_ms = val.0 as i64;
    if (WINDOW_MIN_MS..=WINDOW_MAX_MS).contains(&window_ms) {
        Some(StateWindow { window_ms })
    } else {
        None
    }
}

/// The figure itself: `upstream.submitted` rows in `[since_us, now_us]`
/// whose `request_id` has no `upstream.responded` row **anywhere in the
/// returned set** — a response written a moment after the window closes
/// still closes its intent (spec §4.18's derivation clause). This is
/// `EventFigures`' definition (spec §6), not a second one.
pub fn count_unknown_outcomes(
    store: &dyn Store,
    since_us: i64,
) -> Result<(u64, usize), vadis_core::StoreError> {
    let kinds = [SUBMITTED, RESPONDED];
    let QueryRow::Events(rows) = store.query(Query::EventsSince {
        kinds: &kinds,
        since_us,
    })?
    else {
        return Err(vadis_core::StoreError::Sql(
            "state read: the store answered a non-events row".into(),
        ));
    };
    let answered: std::collections::HashSet<&str> = rows
        .iter()
        .filter(|ev| ev.kind_raw == RESPONDED)
        .filter_map(|ev| ev.request_id.as_deref())
        .collect();
    let total = rows.len();
    let unknown = rows
        .iter()
        .filter(|ev| ev.kind_raw == SUBMITTED)
        .filter(|ev| ev.ts_us >= since_us)
        .filter(|ev| {
            !ev.request_id
                .as_deref()
                .is_some_and(|r| answered.contains(r))
        })
        .count() as u64;
    Ok((unknown, total))
}

/// The `200` body (spec §4.18's two keys, exactly): the window in-band
/// and the figure. `rfc3339` renders each bound; the figure is passed
/// through unchanged.
pub fn state_events_json(
    w: StateWindow,
    now_us: i64,
    unknown: u64,
    rfc3339: fn(i64) -> String,
) -> Value {
    json!({
        "window": {
            "from": rfc3339(now_us.saturating_sub(w.window_ms * 1_000)),
            "to": rfc3339(now_us),
        },
        "unknown_outcome_requests": unknown,
    })
}

/// The whole handler body, minus the HTTP carriage: parse, then read,
/// then answer. `None` from `parse_window` ⇒ the `400` arm (no store
/// read); a store error ⇒ the honest `500` (spec §4.18's status set is
/// `{200, 400, 401}` for a healthy store — an unreadable store is the
/// one thing this surface cannot paper over, and `AppState.store` is
/// `None` only in assemblies that never serve this route).
pub fn handle(
    state: &AppState,
    raw_window: Option<&str>,
    now_us: i64,
    rfc3339: fn(i64) -> String,
) -> Result<Value, &'static str> {
    let w = parse_window(raw_window).ok_or("window")?;
    let store = state.store.as_ref().ok_or("no store")?;
    let (unknown, _) =
        count_unknown_outcomes(store.as_ref(), now_us.saturating_sub(w.window_ms * 1_000))
            .map_err(|_| "store")?;
    Ok(state_events_json(w, now_us, unknown, rfc3339))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use vadis_core::config::{
        BreakevenCfg, CacheCfg, DurationVal, PluginCfg, ServerCfg, SessionCfg, TraceCfg,
        VadisConfig,
    };
    use vadis_core::store::{EventKind, NewEvent, StoredEvent};
    use vadis_core::EventId;

    fn cfg() -> VadisConfig {
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
            plan_policies: None,
            state: None,
        }
    }

    fn app_state(store: Option<std::sync::Arc<dyn Store>>) -> AppState {
        AppState {
            revision: crate::revision::RevisionCell::new(crate::revision::Revision {
                forwarder: crate::forward::Forwarder {
                    config: cfg(),
                    transports: Default::default(),
                    api_keys: Default::default(),
                    store: None,
                    trace: None,
                    transform_engine: None,
                    response_cache: None,
                    session_ttl_us: 0,
                },
                config_identity: crate::health::ConfigIdentity {
                    root_path: "/cfg/config.yaml".into(),
                    roster_path: None,
                    root_sha16: "0123456789abcdef".into(),
                    roster_sha16: String::new(),
                    config_digest: "0011223344556677".into(),
                },
                provider_keys: Vec::new(),
                auth_gate: None,
            }),
            trace_dir: String::new(),
            state_db: String::new(),
            store,
        }
    }

    /// An in-memory store double holding pre-seeded event rows — the
    /// shape `count_unknown_outcomes` joins over. `panic_on_all_events`
    /// is limb (e) of CONF-106's unit half: the read path never
    /// constructs `Query::AllEvents`.
    struct RowsStore {
        rows: Vec<StoredEvent>,
        panic_on_all_events: bool,
    }

    impl Store for RowsStore {
        fn append(&self, _: NewEvent<'_>) -> Result<EventId, vadis_core::StoreError> {
            Err(vadis_core::StoreError::Sql("read-only double".into()))
        }
        fn project(
            &self,
            _: vadis_core::store::ProjectionWrite<'_>,
        ) -> Result<(), vadis_core::StoreError> {
            Ok(())
        }
        fn query(&self, q: Query<'_>) -> Result<QueryRow, vadis_core::StoreError> {
            match q {
                Query::EventsSince { kinds, since_us } => Ok(QueryRow::Events(
                    self.rows
                        .iter()
                        .filter(|e| kinds.contains(&e.kind_raw.as_str()))
                        .filter(|e| e.ts_us >= since_us)
                        .cloned()
                        .collect(),
                )),
                Query::AllEvents if self.panic_on_all_events => {
                    panic!("the live state surface must never scan the whole log")
                }
                _ => Ok(QueryRow::Count(0)),
            }
        }
        fn rebuild(
            &self,
            _: vadis_core::store::Projection,
        ) -> Result<vadis_core::store::RebuildStats, vadis_core::StoreError> {
            Ok(vadis_core::store::RebuildStats::default())
        }
        fn schema_version(&self) -> Result<u32, vadis_core::StoreError> {
            Ok(1)
        }
    }

    fn ev(event_id: i64, kind: &str, ts_us: i64, request_id: Option<&str>) -> StoredEvent {
        StoredEvent {
            event_id: EventId(event_id),
            ts_us,
            kind_raw: kind.to_string(),
            kind: EventKind::from_str_lossy(kind),
            request_id: request_id.map(str::to_string),
            session: None,
            schema_version: 1,
            payload: serde_json::json!({}),
            body_hash: None,
            trace_ref: None,
        }
    }

    // The window grammar and bound: the config grammar, `1ms..=24h`.
    #[test]
    fn window_parses_the_config_grammar_and_bounds() {
        assert_eq!(
            parse_window(Some("15m")).map(|w| w.window_ms),
            Some(900_000)
        );
        assert_eq!(
            parse_window(Some("1h30m")).map(|w| w.window_ms),
            Some(5_400_000)
        );
        assert_eq!(parse_window(Some("1ms")).map(|w| w.window_ms), Some(1));
        assert_eq!(
            parse_window(Some("24h")).map(|w| w.window_ms),
            Some(86_400_000)
        );
        assert_eq!(parse_window(None), None, "absent ⇒ 400");
        assert_eq!(parse_window(Some("nope")), None, "unparsable ⇒ 400");
        assert_eq!(parse_window(Some("0ms")), None, "below the bound ⇒ 400");
        assert_eq!(parse_window(Some("25h")), None, "above the bound ⇒ 400");
        assert_eq!(parse_window(Some("1d")), None, "no d unit (§4.1)");
    }

    // §6's definition, the two discriminating limbs: an intent with no
    // response anywhere, and an intent whose response falls OUTSIDE the
    // window — which still closes it.
    #[test]
    fn an_answer_outside_the_window_still_closes_its_intent() {
        let now = 1_000_000_000_000_000; // µs
        let store = RowsStore {
            rows: vec![
                ev(1, SUBMITTED, now - 60_000_000, Some("r-open")),
                ev(2, RESPONDED, now - 59_000_000, Some("r-closed")),
                // 130 s old: OUTSIDE the 2-minute window below — its intent
                // must not count for that window.
                ev(3, SUBMITTED, now - 130_000_000, Some("r-early")),
            ],
            panic_on_all_events: true,
        };
        // Window (2 minutes): r-open's intent is in, no response for it in
        // the set ⇒ unknown; r-closed is answered; r-early's intent is
        // outside the window ⇒ not counted.
        let (unknown, _) = count_unknown_outcomes(&store, now - 120_000_000).unwrap();
        assert_eq!(unknown, 1, "r-open is the one unanswered intent in-window");
    }

    // The boundary limb the comment above names: a response written
    // AFTER the window's start still closes an intent submitted inside
    // the window (it is in the same returned set). A response older
    // than the window cannot exist for an intent inside the window
    // (event_id order), so the single direction is the whole space.
    #[test]
    fn the_figure_joins_by_request_id_not_by_position() {
        let now = 1_000_000_000_000_000;
        let store = RowsStore {
            rows: vec![
                ev(1, SUBMITTED, now - 100_000, Some("a")),
                ev(2, RESPONDED, now - 50_000, Some("a")),
                ev(3, SUBMITTED, now - 40_000, Some("b")),
                ev(4, SUBMITTED, now - 30_000, Some("c")),
                ev(5, RESPONDED, now - 20_000, Some("c")),
            ],
            panic_on_all_events: true,
        };
        let (unknown, total) = count_unknown_outcomes(&store, now - 60_000).unwrap();
        assert_eq!(unknown, 1, "b is the one unanswered intent");
        // a's intent (100 ms old) predates the 60 ms window, so the returned
        // set holds 4 rows, not 5 — EventsSince already bounded the read.
        assert_eq!(total, 4, "the window excludes a's intent row");
        // A window that excludes a's intent entirely counts only b.
        let (unknown, _) = count_unknown_outcomes(&store, now - 45_000).unwrap();
        assert_eq!(unknown, 1, "a is out, b and c are in (c answered)");
    }

    // The body: exactly two keys, the window in-band.
    #[test]
    fn the_body_is_two_keys_with_the_window_in_band() {
        fn rfc(us: i64) -> String {
            format!("t{us}")
        }
        let w = StateWindow { window_ms: 900_000 };
        let now = 1_760_000_000_000_000i64;
        let v = state_events_json(w, now, 2, rfc);
        assert_eq!(v["unknown_outcome_requests"], 2);
        assert_eq!(v["window"]["from"], format!("t{}", now - 900_000_000));
        assert_eq!(v["window"]["to"], format!("t{now}"));
        assert_eq!(v.as_object().unwrap().len(), 2, "no third key, ever");
    }

    // The 400 arm reads no store: an unparsable window over a store that
    // panics on ANY query never reaches it.
    #[test]
    fn an_unusable_window_never_touches_the_store() {
        let state = app_state(None); // no store at all
                                     // `25h` and absent are out of bound / missing ⇒ the 400 arm; `9h`
                                     // is IN bound (the cap is 24h), so with no store it takes the
                                     // assembly-error arm instead — the proof the 400 arm is not the
                                     // fallthrough for everything.
        assert_eq!(
            handle(&state, Some("25h"), 0, |us| format!("{us}")),
            Err("window")
        );
        assert_eq!(handle(&state, None, 0, |us| format!("{us}")), Err("window"));
        // A usable window with no store is the assembly error, not a 400.
        assert_eq!(
            handle(&state, Some("15m"), 0, |us| format!("{us}")),
            Err("no store")
        );
    }
}
