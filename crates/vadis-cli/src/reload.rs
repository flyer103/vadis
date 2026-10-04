//! The reload's watcher plumbing and publish (ADR-039's mechanism; ADR-040
//! D1/D2/D5/D10/D11/D12; DESIGN §12.20): **event → coalesced look → gate
//! verdict → publish**.
//!
//! The module decides *when to look* (the watcher), *whether anything
//! changed* (the digest, via the same [`config_load::load`] `serve` starts
//! with), *what a refused candidate reports* (one line, RV-9) and — R47-2's
//! half — *what an accepted candidate becomes*: the [`Publisher`] runs
//! DESIGN §12.20's steps 3–5 (the `config.applied` row with D10's value
//! diff, the delta-only plugin edges, the publish of one immutable
//! revision), on the watcher's own thread, off the request path.
//!
//! The three watcher decisions this module lands: **D12.2** — the window is
//! ours and it is a constant ([`COALESCE_WINDOW`], leading edge, never a
//! config key; the pure policy is [`Coalescer`], a function of (event times,
//! window) with no filesystem and no clock of its own, so a unit test pins
//! it); **D12.3** — no second dependency (`notify-debouncer-full` is
//! refused; the coalescing is the few lines below, not a crate); **D12.4** —
//! the registration is on the directories with an exact-path filter. The
//! landing replaces the target's inode (spec §4.11: `temp` + `rename`), so
//! a registration bound to the target's inode could go silent; a
//! directory's inode is not the thing being replaced. The filter is
//! required, not cosmetic: the pair's own directory is *hot* (the store is
//! `<config dir>/state/vadis.db`, the shipped `trace.dir` is
//! `./state/traces`, and the macOS backend delivers sub-directory events),
//! so without exact-path equality the reload would re-read the pair once
//! per request — the per-request check ADR-039 refused outright.
//!
//! One honest boundary, stated rather than discovered: the **registration
//! follows the startup pair's paths**. A revision whose root names a
//! *different* roster file is applied (the loader reads what the root
//! names), and the refusal report's roster path is re-armed to the serving
//! pair — but edits to the newly-named roster file alone produce no event
//! (its directory was never registered). The next event on the registration
//! — any edit of the root — reads it correctly (D12.6), and the operator's
//! remedy for the gap is the same fallback: a restart.
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher as _};

use crate::config_load::{self, ConfigIdentity, ResolvedConfig};

/// The coalescing window (ADR-040 D12.2): a leading-edge constant in this
/// module — never a config key, never a flag, never a signal (D8's last
/// paragraph; the owner's standing no-new-parameter ruling).
///
/// The value's lower bound is the landing's own write gap: spec §4.11's
/// writer sequence (`temp` create + write + fsync + `rename`-over) measured
/// min 0.128 ms / median 0.138 ms / p95 0.182 ms / max 0.258 ms (N = 200;
/// a tracked probe run, re-runnable by its `gap.py`). 200 ms sits ~3
/// orders of magnitude above that gap
/// (776× the run's max) — deliberately conservative, and safe in both
/// directions by D12.2's own argument: the window is a bound on how often
/// two small files are read, never a correctness mechanism, because a look
/// that finds the same digest is a no-op (D1).
pub const COALESCE_WINDOW: Duration = Duration::from_millis(200);

// ---------------------------------------------------------------------
// The pure policy (no filesystem, no clock): the coalescer and the
// reporting rule. Both are unit-tested without a watcher.
// ---------------------------------------------------------------------

/// What the coalescer decides about one event (ADR-040 D12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventDecision {
    /// The first event of a burst: run a look immediately (the leading
    /// edge — a purely trailing window would make a reload's latency the
    /// window plus the load, and the p99 belongs to the change, not to a
    /// timer, ADR-039 D2 criterion 1).
    LookNow,
    /// Inside an open burst's window: folded; the trailing edge will look.
    Folded,
}

/// The coalescing policy as a **pure function of (event times, window)**:
/// the first event of a burst starts a look immediately, events arriving
/// inside the window that follows are folded into it, and one further look
/// runs when the burst's last event is older than the window (ADR-040
/// D12.2). No filesystem, no clock — the caller feeds monotonic
/// millisecond readings; the unit test pins the behaviour with no watcher.
///
/// The window is anchored at the burst's **first** event ("events arriving
/// inside the window that follows are folded into it"): an event at or
/// past `burst_start + window` opens a new burst with its own leading
/// look. The trailing look exists because the leading look can race the
/// rest of a burst (the landing's two events, an editor's write sequence);
/// a burst of one event needs none — its leading look already read the
/// state that event announced.
#[derive(Debug, Clone)]
pub struct Coalescer {
    window_ms: u64,
    burst_start_ms: Option<u64>,
    last_event_ms: Option<u64>,
}

impl Coalescer {
    pub const fn new(window_ms: u64) -> Self {
        Self {
            window_ms,
            burst_start_ms: None,
            last_event_ms: None,
        }
    }

    /// One matching event at `t_ms` (monotonic milliseconds, any epoch).
    pub fn on_event(&mut self, t_ms: u64) -> EventDecision {
        match self.burst_start_ms {
            Some(start) if t_ms < start + self.window_ms => {
                self.last_event_ms = Some(t_ms);
                EventDecision::Folded
            }
            _ => {
                self.burst_start_ms = Some(t_ms);
                self.last_event_ms = Some(t_ms);
                EventDecision::LookNow
            }
        }
    }

    /// When the pending trailing look is due (monotonic ms), if a burst is
    /// open and it saw more than its leading event.
    pub fn trailing_due_at_ms(&self) -> Option<u64> {
        match (self.burst_start_ms, self.last_event_ms) {
            (Some(start), Some(last)) if last > start => Some(last + self.window_ms),
            _ => None,
        }
    }

    /// The trailing edge has fired: the burst is closed. The caller runs
    /// the one further look itself.
    pub fn burst_closed(&mut self) {
        self.burst_start_ms = None;
        self.last_event_ms = None;
    }
}

/// The gate verdict of one look: the pair as read, judged by the same
/// loader `serve` starts with (spec §4.15 rule 1; DESIGN §12.20 step 2).
#[derive(Debug)]
pub enum Verdict {
    /// D1: the digest is unchanged — no revision, no row, no line,
    /// nothing observable at all.
    NoChange,
    /// A different digest that loads. **Unapplied on purpose**: the
    /// publish of an accepted candidate (the `config.applied` row, the
    /// handle swap, the capture-once seam) is R47-2's scope.
    Accepted(Box<ResolvedConfig>),
    /// D4/D11: the loader refused — keep serving, report one line, write
    /// nothing.
    Refused(Refusal),
}

/// A refused candidate, carried to the report (ADR-040 D11's four facts).
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The revision still being served (D11 item 2) — the one fact
    /// "which configuration am I talking to?" needs.
    pub serving_digest: String,
    /// The resolved root path the look read (D11 item 4).
    pub root_path: PathBuf,
    /// The resolved roster path of the watched pair, when there is one.
    /// Note the honesty boundary: on the refusal path the loader's
    /// *verbatim reason* is what names the exact path a roster-side
    /// failure occurred at (`config_load` embeds the resolved roster path
    /// in its refusals); this member names the pair the watcher was
    /// registered on.
    pub roster_path: Option<PathBuf>,
    /// The loader's refusal reason, verbatim (D11 item 3).
    pub reason: String,
}

/// The refusal's one line (ADR-040 D11): a fixed marker that this came
/// from a **reload** (not a start), the revision still served, the
/// resolved path(s) the candidate was read from, and the loader's reason
/// verbatim. The wording is this module's; the content is the contract's.
pub fn refusal_line(r: &Refusal) -> String {
    let read = match &r.roster_path {
        Some(roster) => format!("read {} and {}", r.root_path.display(), roster.display()),
        None => format!("read {}", r.root_path.display()),
    };
    format!(
        "vadis: reload refused ({read}, still serving revision {}): {}",
        r.serving_digest, r.reason
    )
}

/// The reporting rule (ADR-040 D11): **one line per refused candidate —
/// one landing, one line — no de-duplication and no rate limit across
/// landings.** The one refinement: a burst's trailing look can re-read the
/// *same* refused state its leading look already reported (one landing is
/// two events; the coalescer may look twice at what is byte-wise one
/// candidate), and that second look is the same landing, not a new one —
/// so a refusal line identical to the one already emitted *within the
/// same burst* is not repeated. The state resets at every burst's leading
/// edge, so an operator who lands the same broken pair twice (two bursts)
/// gets two lines, exactly as D11 requires. Pure: no I/O, unit-tested.
#[derive(Debug, Default)]
pub struct Reporter {
    burst_last_line: Option<String>,
}

impl Reporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new burst opened (its leading look is about to run): the
    /// per-burst suppression resets.
    pub fn burst_started(&mut self) {
        self.burst_last_line = None;
    }

    /// Fold one look's verdict into the report stream. Returns the line
    /// to emit, or `None` when the contract says nothing is observable.
    pub fn report(&mut self, verdict: &Verdict) -> Option<String> {
        match verdict {
            // D1: a look that finds no change ends with nothing observable.
            Verdict::NoChange => None,
            // R47-2's publish owns the accepted half's surface (the
            // `config.applied` row); nothing here claims a change.
            Verdict::Accepted(_) => None,
            Verdict::Refused(refusal) => {
                let line = refusal_line(refusal);
                if self.burst_last_line.as_deref() == Some(line.as_str()) {
                    None
                } else {
                    self.burst_last_line = Some(line.clone());
                    Some(line)
                }
            }
        }
    }
}

/// One look (DESIGN §12.20 step 2): read + gate the pair through the same
/// [`config_load::load`] `serve` starts with, then let the digest decide
/// (D1). `watched_roster` is the serving pair's resolved roster path — it
/// is what the refusal names beside the root (see [`Refusal::roster_path`]).
pub fn look(root_path: &Path, serving_digest: &str, watched_roster: Option<&Path>) -> Verdict {
    match config_load::load(root_path) {
        Ok(rc) => {
            if rc.identity.config_digest == serving_digest {
                Verdict::NoChange
            } else {
                Verdict::Accepted(Box::new(rc))
            }
        }
        Err(reason) => Verdict::Refused(Refusal {
            serving_digest: serving_digest.to_string(),
            root_path: root_path.to_path_buf(),
            roster_path: watched_roster.map(Path::to_path_buf),
            reason,
        }),
    }
}

// ---------------------------------------------------------------------
// The publish (R47-2; ADR-040 D2/D5/D10; DESIGN §12.20 steps 3–5): an
// accepted candidate becomes the revision the request path serves.
// ---------------------------------------------------------------------

/// D5's refused set (spec §4.15's "keys the reload refuses"): every key
/// whose only consumer is an object the **process** builds once and holds.
/// Each entry is the diff's exact path beside the reason the refusal
/// names; these are scalar leaves, so the value diff always reports them
/// at exactly these paths. The direction of travel is one way (D5): a
/// later card may promote a key by rebuilding its object with the
/// revision; nothing here tightens.
const REFUSED_KEYS: &[(&str, &str)] = &[
    ("server.addr", "the listener is bound once at startup"),
    (
        "server.upstream_attempt_timeout",
        "the provider transports are built once at startup",
    ),
    (
        "server.max_body_bytes",
        "the inbound body bound is built once for the process (spec §4.13)",
    ),
    (
        "trace.dir",
        "the resolved trace directory is held by the trace writer",
    ),
];

/// The serving head: the facts a look judges against (the digest decides,
/// D1) and a refusal names (D11). Re-armed by every accepted publish — the
/// watcher's "serving digest" is this value, never one fixed at start.
struct ServingHead {
    digest: String,
    roster_path: Option<PathBuf>,
}

/// The publish half of the reload (R47-2; DESIGN §12.20's steps 3–5, run
/// on the watcher's thread — off the request path by construction). One
/// accepted candidate, in order:
///
/// 1. **the value diff** against the revision in force (D10), computed
///    here and never by a request;
/// 2. **D5's refused set** — a candidate touching a process-held key is
///    refused with the key named, before any row exists (D10's "a refused
///    key can never appear in a `changed_keys` list");
/// 3. **the auth resolution** (D5's honesty note): an unchanged
///    `auth_token_env` name reuses the startup's one read (spec §4.7); a
///    new name is read now, and an unset one refuses the switch with the
///    startup refusal's own reason;
/// 4. **the build** — the same assembly `serve` starts with, off the
///    request path, so the published revision is complete before it is
///    visible (D2: all-or-nothing);
/// 5. **one `config.applied` row, committed before the publish**
///    (ADR-010's order; RV-1): if the intent row cannot commit, the
///    revision never serves — the refusal keeps the process honest at
///    exactly the point RV-1 names a defect;
/// 6. **row 12 at the crossed plugin edges only** (D5's delta: unchanged
///    entries stay mounted; a roster-only change writes none);
/// 7. **the publish and the re-arm**: the cell's readers see the whole
///    new revision or the whole old one, and the next look judges against
///    the new digest.
///
/// One ownership rule, learned from R47-1's two regressions: the
/// publisher is owned by the watcher's **detached** thread (its `Drop` is
/// deliberately non-blocking — the backend's teardown latency must never
/// sit in `serve`'s drop), so it must not own the serving assembly's
/// *lifetimes*. The cell, the store and the trace sink are held **weak**:
/// their strong owners are `serve`'s own frame (and the axum state), so
/// the store's writer lock is released exactly when `serve` is torn down,
/// never on the watcher thread's schedule. An upgrade that fails means
/// the process is exiting — a reload into a dying process is a no-op,
/// not a refusal.
pub struct Publisher {
    /// The published handle the request path captures (D2 step 5). Weak:
    /// see the struct's ownership rule.
    cell: std::sync::Weak<vadis_proxy::RevisionCell>,
    /// The one store handle (D6: one ledger — the switch's rows land in
    /// the same log the startup row lives in). Weak: this handle carries
    /// the writer lock, whose lifetime is `serve`'s, not this thread's.
    store: std::sync::Weak<dyn vadis_core::store::Store>,
    /// The process's one trace sink (`trace.dir` is refused, D5); every
    /// revision's writer shares it, stamped with that revision's digest.
    /// Weak, same rule.
    trace_sink: std::sync::Weak<vadis_store::TraceSink>,
    /// The watched root's resolved path — fixed: it is where the pair is
    /// re-read from, and no revision can move it (a different root is a
    /// different process).
    root_path: PathBuf,
    /// The startup auth facts (spec §4.7's "read once"): the env var the
    /// startup config named and the token read from it. A revision naming
    /// the SAME variable reuses this read; one naming a different
    /// variable is read at the switch (ADR-040 D5's honesty note).
    startup_auth: Option<(String, String)>,
    /// The reload's own report channel (the watcher's sink): the plugin
    /// assembly's notes print here, exactly as startup's print to stderr.
    sink: Arc<dyn Fn(String) + Send + Sync>,
    /// The serving head, re-armed by each accepted publish.
    head: std::sync::Mutex<ServingHead>,
}

impl Publisher {
    /// The publisher is born knowing the startup revision — the cell
    /// already holds it and its `config.applied` row has already
    /// committed (the startup row, `previous_config_digest: null`). The
    /// three assembly handles are borrowed and held **weak** (the
    /// struct's ownership rule): the caller keeps the strong refs.
    pub fn new(
        cell: &vadis_proxy::SharedRevision,
        store: &Arc<dyn vadis_core::store::Store>,
        trace_sink: &Arc<vadis_store::TraceSink>,
        identity: &ConfigIdentity,
        startup_auth: Option<(String, String)>,
        sink: Arc<dyn Fn(String) + Send + Sync>,
    ) -> Self {
        Self {
            cell: Arc::downgrade(cell),
            store: Arc::downgrade(store),
            trace_sink: Arc::downgrade(trace_sink),
            root_path: identity.root_path.clone(),
            startup_auth,
            sink,
            head: std::sync::Mutex::new(ServingHead {
                digest: identity.config_digest.clone(),
                roster_path: identity.roster_path.clone(),
            }),
        }
    }

    /// The facts one look judges against (D1) and a refusal names (D11).
    fn head(&self) -> (String, Option<PathBuf>) {
        let h = self.head.lock().expect("the serving head is poisoned");
        (h.digest.clone(), h.roster_path.clone())
    }

    /// Steps 3–5 for one accepted candidate. `Err` is a refusal: the
    /// revision in force keeps serving and the caller reports D11's one
    /// line, indistinguishable in kind from the loader's own refusals.
    pub fn apply(&self, candidate: ResolvedConfig) -> Result<(), Refusal> {
        // The serving assembly, upgraded from the weak holds. A failed
        // upgrade is possible only while `serve`'s frame is being torn
        // down — the process is exiting, the switch is meaningless, and
        // there is no refusal to report (D11's surface is for a process
        // that keeps serving). This is a no-op, not a defect.
        let (Some(cell), Some(store), Some(trace_sink)) = (
            self.cell.upgrade(),
            self.store.upgrade(),
            self.trace_sink.upgrade(),
        ) else {
            return Ok(());
        };
        let serving = cell.capture();
        let serving_digest = serving.config_identity.config_digest.clone();
        let refuse = |reason: String| {
            let (_, roster_path) = self.head();
            Refusal {
                serving_digest: serving_digest.clone(),
                root_path: self.root_path.clone(),
                roster_path,
                reason,
            }
        };

        // 1. The value diff (D10). A serialization failure is a refusal,
        // not a panic: the row cannot be written honestly, so the
        // revision is not served.
        let changes = match vadis_core::changed_keys(&serving.forwarder.config, &candidate.vadis) {
            Ok(c) => c,
            Err(e) => {
                return Err(refuse(format!(
                    "the changed_keys diff could not be computed: {e}"
                )))
            }
        };
        // 2. D5's refused set — named per key, before any row exists.
        for change in &changes {
            for (key, why) in REFUSED_KEYS {
                if change.path == *key {
                    return Err(refuse(format!(
                        "a reload may not change {key} ({why}); the remedy is a restart"
                    )));
                }
            }
        }
        // 3. The auth resolution (D5's honesty note).
        let gate = match self.resolve_gate(&candidate) {
            Ok(g) => g,
            Err(reason) => return Err(refuse(reason)),
        };

        // 4. The build — the same assembly `serve` starts with.
        let note = |line: String| (self.sink)(format!("vadis: {line}"));
        let revision = crate::build_revision(&candidate, &store, &trace_sink, gate, &note);

        // 5. Row 13, committed BEFORE the publish (ADR-010's order, the
        // startup path's own order). The payload is one flat object on
        // every row (DESIGN §12.10.5 note R10): the digest half plus
        // `previous_config_digest` and `changed_keys` — on a switch both
        // are present and non-null (a comment-only revision's `[]` is the
        // sharpest case: the digest moved, the values did not).
        let payload = serde_json::json!({
            "config_path": candidate.identity.root_path.to_string_lossy(),
            "schema_version": store.schema_version().unwrap_or(0),
            "root_path": candidate.identity.root_path.to_string_lossy(),
            "roster_path": candidate.identity.roster_path.as_ref().map(|p| p.to_string_lossy()),
            "root_sha16": candidate.identity.root_sha16,
            "roster_sha16": candidate.identity.roster_sha16,
            "config_digest": candidate.identity.config_digest,
            "previous_config_digest": serving_digest,
            "changed_keys": changes,
        });
        if let Err(e) = store.append(vadis_core::NewEvent::store_level(
            vadis_core::EventKind::ConfigApplied,
            payload,
        )) {
            return Err(refuse(format!(
                "the config.applied row could not be committed: {e}"
            )));
        }

        // 6. Row 12's edges — only the edges actually crossed (D5; a
        // roster-only change writes none). NORMAL class: a write failure
        // here is observation loss, never a publish blocker (RV-1
        // constrains row 13 alone).
        for ev in plugin_edges(
            &serving.forwarder.config.plugins,
            &candidate.vadis.plugins,
            &candidate.identity.config_digest,
        ) {
            let _ = store.append(ev);
        }

        // 7. The publish — and the watcher's re-arm: the next look judges
        // against the revision now serving (D1), and a refusal names the
        // serving pair's own paths.
        let head = ServingHead {
            digest: candidate.identity.config_digest.clone(),
            roster_path: candidate.identity.roster_path.clone(),
        };
        cell.publish(revision);
        *self.head.lock().expect("the serving head is poisoned") = head;
        Ok(())
    }

    /// D5's `auth_token_env` honesty note: the *name* is per-revision.
    /// An unchanged name reuses the startup's one read (spec §4.7's
    /// contract is untouched — the value is not re-read); a changed or
    /// newly-added name is read now, and an unset or empty variable
    /// refuses the switch with the startup refusal's own reason rather
    /// than quietly serving without a gate.
    fn resolve_gate(
        &self,
        candidate: &ResolvedConfig,
    ) -> Result<Option<vadis_proxy::AuthGate>, String> {
        let name = candidate.vadis.server.auth_token_env.as_deref();
        match name {
            None => Ok(None),
            Some(n) if Some(n) == self.startup_auth.as_ref().map(|(name, _)| name.as_str()) => {
                Ok(self
                    .startup_auth
                    .as_ref()
                    .map(|(_, token)| vadis_proxy::AuthGate::new(token.clone())))
            }
            Some(n) => match std::env::var(n) {
                Ok(v) if !v.is_empty() => Ok(Some(vadis_proxy::AuthGate::new(v))),
                other => {
                    let state = match other {
                        Ok(_) => "empty",
                        Err(_) => "unset",
                    };
                    Err(format!(
                        "server.auth_token_env names {n}, which is {state}: refusing the switch \
                         (a token-less revision would serve unauthenticated)"
                    ))
                }
            },
        }
    }
}

/// D5's mount-the-delta as row 12's edges, keyed by the plugin entry's
/// own identity (`id`, which the loader guarantees unique). An entry
/// unchanged under that identity is left mounted and writes nothing; a
/// `disabled` entry is not mounted, so only its *transitions* across the
/// mount line are edges. A changed mounted entry is a rebuild — the
/// `unloaded` row then the `loaded` row, in that order. The list is
/// deterministic (sorted by id) so two processes applying the same pair
/// write the same rows (AGENTS 2).
fn plugin_edges(
    old: &[vadis_core::config::PluginCfg],
    new: &[vadis_core::config::PluginCfg],
    digest: &str,
) -> Vec<vadis_core::NewEvent<'static>> {
    use std::collections::BTreeMap;
    use vadis_core::config::PluginCfg;

    fn keyed(list: &[PluginCfg]) -> BTreeMap<&str, &PluginCfg> {
        list.iter().map(|p| (p.id.as_str(), p)).collect()
    }
    fn row(
        kind: vadis_core::EventKind,
        p: &PluginCfg,
        digest: &str,
    ) -> vadis_core::NewEvent<'static> {
        vadis_core::NewEvent::store_level(
            kind,
            serde_json::json!({
                "id": p.id,
                "kind": p.kind,
                // DESIGN §12.10.5 row 12's "tier": derivable from the kind
                // (`builtin/<name>` is tier-A, `process` is tier-B) — the
                // loader has already refused anything else.
                "tier": if p.kind.starts_with("builtin/") { "A" } else { "B" },
                // The *effective config digest* member (note R10's table):
                // on a switch, the new revision's.
                "config_digest": digest,
            }),
        )
    }
    let differs = |a: &PluginCfg, b: &PluginCfg| {
        // Both entries are mounted here (`disabled` transitions are the
        // mount-line arms below), so the whole serialized entry is the
        // comparison — kind, config, url, inject, isolate, intercept.
        match (serde_json::to_value(a), serde_json::to_value(b)) {
            (Ok(a), Ok(b)) => a != b,
            // A value that cannot serialize cannot be compared: treat it
            // as changed (rebuild) rather than silently left mounted.
            _ => true,
        }
    };

    let old_map = keyed(old);
    let new_map = keyed(new);
    let ids: std::collections::BTreeSet<&str> =
        old_map.keys().chain(new_map.keys()).copied().collect();
    let mut rows = Vec::new();
    for id in ids {
        match (old_map.get(id), new_map.get(id)) {
            (None, Some(n)) if !n.disabled => {
                rows.push(row(vadis_core::EventKind::PluginLoaded, n, digest));
            }
            (Some(o), None) if !o.disabled => {
                rows.push(row(vadis_core::EventKind::PluginUnloaded, o, digest));
            }
            (Some(o), Some(n)) => match (o.disabled, n.disabled) {
                (false, true) => {
                    rows.push(row(vadis_core::EventKind::PluginUnloaded, o, digest));
                }
                (true, false) => {
                    rows.push(row(vadis_core::EventKind::PluginLoaded, n, digest));
                }
                (false, false) if differs(o, n) => {
                    rows.push(row(vadis_core::EventKind::PluginUnloaded, o, digest));
                    rows.push(row(vadis_core::EventKind::PluginLoaded, n, digest));
                }
                _ => {}
            },
            _ => {}
        }
    }
    rows
}

// ---------------------------------------------------------------------
// The registration (ADR-040 D12.4): directories, exact-path filter.
// ---------------------------------------------------------------------

/// The registration's filter: a look is triggered only by an event whose
/// path is **exactly one of the two resolved paths** (the root, and the
/// roster when the pair names one). Each target is held in two spellings —
/// the loader's lexical-absolute one and the canonical one — because the
/// platform backends do not promise either: inotify reports paths under
/// the directory as it was registered, while FSEvents resolves symlinks
/// (a config under `/var/…` arrives as `/private/var/…`). The comparison
/// itself is string equality only: no syscall per event, which matters
/// because the directory is hot (store and trace writes; see the module
/// docs) and every one of those events passes through [`Self::matches`].
#[derive(Debug)]
struct WatchFilter {
    /// The directories to register on (deduped): the pair's own. The
    /// landing's `rename` replaces the target's inode, never the
    /// directory's, so the registration survives it structurally.
    dirs: Vec<PathBuf>,
    /// The exact paths a matching event may name, in both spellings.
    targets: Vec<PathBuf>,
}

impl WatchFilter {
    fn for_identity(identity: &ConfigIdentity) -> Self {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut targets: Vec<PathBuf> = Vec::new();
        let mut add = |path: &Path| {
            // The lexical spelling the loader resolved (the fact /health
            // reports; a symlink stays a symlink — the digest, not the
            // path, identifies the bytes).
            if !targets.iter().any(|t| t == path) {
                targets.push(path.to_path_buf());
            }
            // The canonical spelling, when it differs (symlinked
            // ancestors): the spelling FSEvents reports.
            if let Ok(canonical) = std::fs::canonicalize(path) {
                if canonical != path && !targets.iter().any(|t| t == &canonical) {
                    targets.push(canonical);
                }
            }
            if let Some(dir) = path.parent() {
                if !dirs.iter().any(|d| d == dir) {
                    dirs.push(dir.to_path_buf());
                }
            }
        };
        add(&identity.root_path);
        if let Some(roster) = &identity.roster_path {
            add(roster);
        }
        Self { dirs, targets }
    }

    /// An event is relevant iff one of its paths is exactly a target
    /// (D12.4). A `temp` file of the landing, a store write and a trace
    /// line all live in the watched directories and are none of the
    /// targets — the hot-directory noise never reaches a look.
    fn matches(&self, event: &notify::Event) -> bool {
        event
            .paths
            .iter()
            .any(|p| self.targets.iter().any(|t| t == p))
    }
}

// ---------------------------------------------------------------------
// The watcher itself: notify behind one dedicated thread, the policy
// above inside.
// ---------------------------------------------------------------------

/// The watcher's own channel: a platform event, or the shutdown sentinel.
/// One channel (rather than a flag polled beside `recv`) keeps the loop
/// asleep until there is something to do — no wake-up cadence of its own.
enum Msg {
    Event(notify::Result<notify::Event>),
    Shutdown,
}

/// The running registration. `serve` holds it for the process's lifetime;
/// dropping it stops the loop.
///
/// Two placements are deliberate, and both are about *whose* time the
/// backend spends:
///
/// - **Everything lives on one dedicated thread, not on the tokio
///   runtime and not on the caller.** The macOS backend's stream start
///   goes through fseventsd and is slow — measured ~0.6 s per
///   registration on this machine (first in a process: ~1.7 s) — and its
///   teardown joins the runloop thread in `Drop`. If creation sat on
///   `serve`'s startup path, every boot would pay it before the listener
///   answers (a live-config fixture with a sub-second timing budget
///   trips over exactly this); if the handle's teardown sat in `serve`'s
///   own drop, every teardown would pay the join while the store's
///   writer lock is still held by that future's locals. On the dedicated
///   thread, both costs are off both paths.
/// - **Dropping is non-blocking and the thread is detached.** `Drop`
///   sends the sentinel and returns; the backend's teardown then runs on
///   the watcher thread itself. There is nothing to join — and nothing
///   joining means a caller's drop never inherits the backend's latency.
pub struct Watcher {
    looks: Arc<AtomicU64>,
    ready: Arc<AtomicBool>,
    control: std::sync::mpsc::Sender<Msg>,
}

impl Watcher {
    /// Spawn the watcher thread and return immediately; the thread builds
    /// the backend, registers on the pair's directories, sets
    /// [`Self::is_ready`] and runs the loop. The `sink` receives this
    /// module's only outputs — the refusal line (ADR-040 D11) and, when
    /// the mechanism cannot be set up at all, one line saying so (the
    /// process serves on; a change then takes effect on restart, D12.6's
    /// named fallback). `serve` wires the sink to the process's own
    /// stderr, test rigs to a capture. The `publisher` owns the accepted
    /// half (DESIGN §12.20 steps 3–5) and the serving head the looks
    /// judge against — the digest a look compares is re-armed by every
    /// accepted publish (D1).
    ///
    /// An `Err` here means the thread itself could not be spawned; the
    /// caller reports and serves on, same fallback.
    pub fn start(
        identity: &ConfigIdentity,
        sink: Arc<dyn Fn(String) + Send + Sync>,
        publisher: Arc<Publisher>,
    ) -> Result<Self, String> {
        let filter = WatchFilter::for_identity(identity);
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let looks = Arc::new(AtomicU64::new(0));
        let ready = Arc::new(AtomicBool::new(false));
        let unavailable = |reason: String| {
            format!(
                "vadis: the config reload watcher is unavailable: {reason} \
                 (serving on; a config change takes effect on restart)"
            )
        };
        std::thread::Builder::new()
            .name("vadis-reload-watch".to_string())
            .spawn({
                let looks = looks.clone();
                let ready = ready.clone();
                let events = tx.clone();
                let root_path = identity.root_path.clone();
                let roster_path = identity.roster_path.clone();
                move || {
                    let mut imp = match notify::recommended_watcher(move |res| {
                        let _ = events.send(Msg::Event(res));
                    }) {
                        Ok(imp) => imp,
                        Err(e) => {
                            sink(unavailable(format!("cannot create the file watcher: {e}")));
                            return;
                        }
                    };
                    for dir in &filter.dirs {
                        if let Err(e) = imp.watch(dir, RecursiveMode::NonRecursive) {
                            sink(unavailable(format!("cannot watch {}: {e}", dir.display())));
                            return;
                        }
                    }
                    ready.store(true, Ordering::Relaxed);
                    let target = LookTarget {
                        root_path,
                        roster_path,
                    };
                    run(rx, imp, filter, target, publisher, sink, looks);
                }
            })
            .map_err(|e| unavailable(format!("cannot spawn the watcher thread: {e}")))?;
        Ok(Self {
            looks,
            ready,
            control: tx,
        })
    }

    /// How many looks the loop has run. A diagnostic for the rigs (the
    /// hot-directory arm asserts it stays at zero); not a config surface,
    /// not a gate input.
    pub fn look_count(&self) -> u64 {
        self.looks.load(Ordering::Relaxed)
    }

    /// The registration is live. A diagnostic for the rigs (they wait
    /// for it before performing a landing — an event that arrives before
    /// registration is one the backend cannot deliver); production never
    /// reads it, because a missed event's next look is the next event
    /// (D12.6).
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Non-blocking by construction (see the struct's doc): the
        // backend's teardown runs on the watcher thread after it drains.
        let _ = self.control.send(Msg::Shutdown);
    }
}

/// What one look reads and names: the watched pair's resolved paths,
/// fixed at the watcher's construction (the registration follows the
/// startup pair — see the module doc's boundary note). The *serving
/// digest* is deliberately NOT here: it moves with every accepted
/// publish, so the look reads it off the [`Publisher`] instead.
struct LookTarget {
    root_path: PathBuf,
    roster_path: Option<PathBuf>,
}

/// The loop: events in, looks out, per the pure policy. Timing is the
/// only impure part — the coalescer is fed monotonic milliseconds
/// relative to the loop's start, and the trailing edge is `recv_timeout`
/// at the due instant (a folded event re-arms it on the next pass).
///
/// The `notify` handle is owned here (see [`Watcher`]'s doc): the
/// registration lives exactly as long as the loop, and the backend's
/// teardown runs in this thread's own exit, never in the caller's.
fn run(
    rx: std::sync::mpsc::Receiver<Msg>,
    _imp: notify::RecommendedWatcher,
    filter: WatchFilter,
    target: LookTarget,
    publisher: Arc<Publisher>,
    sink: Arc<dyn Fn(String) + Send + Sync>,
    looks: Arc<AtomicU64>,
) {
    let base = Instant::now();
    let rel_ms = || base.elapsed().as_millis() as u64;
    let window_ms = COALESCE_WINDOW.as_millis() as u64;
    let mut coalescer = Coalescer::new(window_ms);
    let mut reporter = Reporter::new();
    loop {
        let msg = match coalescer.trailing_due_at_ms() {
            // A burst is open and saw more than its leading event: wait
            // for the next event or the trailing edge, whichever first.
            Some(due) => match rx.recv_timeout(Duration::from_millis(due.saturating_sub(rel_ms())))
            {
                Ok(msg) => Some(msg),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            },
            // No trailing look pending: sleep until something arrives.
            None => match rx.recv() {
                Ok(msg) => Some(msg),
                Err(_) => break,
            },
        };
        match msg {
            // The trailing edge: the burst's last event is older than
            // the window, so one further look runs and the burst closes.
            None => {
                coalescer.burst_closed();
                one_look(&target, &publisher, &mut reporter, &sink, &looks);
            }
            Some(Msg::Shutdown) => break,
            // A notify error names no path fact worth a look; the next
            // look is the next event on the registration — D12.6's
            // stated boundary, no reconciliation pass.
            Some(Msg::Event(Err(_))) => continue,
            Some(Msg::Event(Ok(event))) => {
                if !filter.matches(&event) {
                    continue;
                }
                if coalescer.on_event(rel_ms()) == EventDecision::LookNow {
                    reporter.burst_started();
                    one_look(&target, &publisher, &mut reporter, &sink, &looks);
                }
            }
        }
    }
}

/// One look through the gate, then — for an accepted candidate — through
/// the publish, and finally through the reporting rule. The load reads
/// two small files and the publish diffs two in-memory values — all of it
/// on the watcher's own thread, off the request path by construction.
/// A refusal at EITHER stage (the loader's, or the publish's D5/auth
/// gates) lands in the same report: one line, the revision still served,
/// the reason verbatim (D11; RV-9).
fn one_look(
    target: &LookTarget,
    publisher: &Arc<Publisher>,
    reporter: &mut Reporter,
    sink: &Arc<dyn Fn(String) + Send + Sync>,
    looks: &AtomicU64,
) {
    looks.fetch_add(1, Ordering::Relaxed);
    let (serving_digest, _) = publisher.head();
    let verdict = look(
        &target.root_path,
        &serving_digest,
        target.roster_path.as_deref(),
    );
    match verdict {
        // The accepted half: apply, and route an apply-stage refusal
        // through the same reporting rule as a loader refusal.
        Verdict::Accepted(rc) => {
            if let Err(refusal) = publisher.apply(*rc) {
                if let Some(line) = reporter.report(&Verdict::Refused(refusal)) {
                    sink(line);
                }
            }
        }
        other => {
            if let Some(line) = reporter.report(&other) {
                sink(line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // -------------------------------------------------------------
    // Fixtures: a minimal valid config and a tiny tempdir helper (the
    // loader's own is private to it).
    // -------------------------------------------------------------

    const MINIMAL: &str = r#"
server:   { addr: "127.0.0.1:8790", upstream_attempt_timeout: 60s, request_timeout: 10m }
session:  { key_sources: ["prompt_cache_key"], ttl: 12h }
cache:    { sticky: true, breakeven: { enabled: true, min_remaining_turns: 3, safety_factor: 1.2 } }
trace:    { dir: "./state/traces", rollover: hourly }
providers:
  - name: p
    urls: { chat: https://x.example/v1/chat/completions }
    api_key_env: P_KEY
    wire_api: chat
    supports: [chat]
    models:
      - id: m1
        context: 200k
        price:
          input_miss: 0.00066
          input_hit: 0.000022
          cache_write: 0.0
          output: 0.00198
          peak: { multiplier: 1.0, windows: [] }
        source: "https://x.example/pricing @2026-09-19"
aliases:  {}
plugins: []
fallback: []
"#;

    struct TempDirGuard(PathBuf);
    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir(tag: &str) -> (TempDirGuard, PathBuf) {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "vadis-reload-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        (TempDirGuard(dir), path)
    }

    /// The landing the rig performs — spec §4.11's writer sequence
    /// (`temp` in the target's directory, flush, `rename` over the
    /// target), never a hand edit (ADR-039's consequence for the
    /// platform backends).
    fn land(target: &Path, bytes: &str) {
        let tmp = target.with_extension("landing-tmp");
        std::fs::write(&tmp, bytes).unwrap();
        // flush+fsync before the rename, as the writer does.
        let f = std::fs::File::open(&tmp).unwrap();
        f.sync_all().unwrap();
        drop(f);
        std::fs::rename(&tmp, target).unwrap();
    }

    fn load_identity(root: &Path) -> ConfigIdentity {
        config_load::load(root).expect("the fixture loads").identity
    }

    // -------------------------------------------------------------
    // The pure coalescer (the card's named unit test): no filesystem,
    // no watcher — (event times, window) in, decisions out.
    // -------------------------------------------------------------

    #[test]
    fn coalescer_is_a_pure_function_of_event_times_and_window() {
        // Window 200 (ms). Each case: the decision sequence, then the
        // trailing-look due time after the last event.
        //
        // A single event: one leading look, no trailing — the leading
        // look already read the state that event announced.
        let mut c = Coalescer::new(200);
        assert_eq!(c.on_event(1_000), EventDecision::LookNow);
        assert_eq!(c.trailing_due_at_ms(), None);

        // A burst folded inside the window: one leading look at the
        // first event, everything else folded, one trailing look due at
        // last_event + window. (The landing's own shape: temp create at
        // t, rename at t+ε.)
        let mut c = Coalescer::new(200);
        assert_eq!(c.on_event(1_000), EventDecision::LookNow);
        assert_eq!(c.on_event(1_050), EventDecision::Folded);
        assert_eq!(c.on_event(1_120), EventDecision::Folded);
        assert_eq!(c.trailing_due_at_ms(), Some(1_320));
        c.burst_closed();
        assert_eq!(c.trailing_due_at_ms(), None);

        // Inside the window is strict: an event at exactly
        // burst_start + window is a NEW burst with its own leading
        // look ("events arriving inside the window that follows are
        // folded into it" — at the boundary it no longer follows).
        let mut c = Coalescer::new(200);
        assert_eq!(c.on_event(1_000), EventDecision::LookNow);
        assert_eq!(c.on_event(1_200), EventDecision::LookNow);
        assert_eq!(c.trailing_due_at_ms(), None);

        // A later burst after a closed one leads again, and the window
        // is anchored at the burst's FIRST event (an event inside the
        // last-event's window but past the first event's window starts
        // a new burst).
        let mut c = Coalescer::new(200);
        assert_eq!(c.on_event(0), EventDecision::LookNow);
        assert_eq!(c.on_event(150), EventDecision::Folded);
        assert_eq!(c.on_event(250), EventDecision::LookNow); // 250 >= 0+200
        assert_eq!(c.trailing_due_at_ms(), None);

        // The same inputs produce the same outputs — the function is of
        // (event times, window) alone: re-run the first case on a fresh
        // coalescer and get the identical sequence.
        let mut c2 = Coalescer::new(200);
        let seq: Vec<_> = [1_000u64, 1_050, 1_120]
            .into_iter()
            .map(|t| c2.on_event(t))
            .collect();
        assert_eq!(
            seq,
            vec![
                EventDecision::LookNow,
                EventDecision::Folded,
                EventDecision::Folded
            ]
        );
        assert_eq!(c2.trailing_due_at_ms(), Some(1_320));

        // The window is an input, not a baked-in value.
        let mut narrow = Coalescer::new(50);
        assert_eq!(narrow.on_event(1_000), EventDecision::LookNow);
        assert_eq!(narrow.on_event(1_050), EventDecision::LookNow);
    }

    // -------------------------------------------------------------
    // The reporting rule: one landing one line, two landings two lines.
    // -------------------------------------------------------------

    fn refusal(digest: &str, reason: &str) -> Refusal {
        Refusal {
            serving_digest: digest.to_string(),
            root_path: PathBuf::from("/cfg/config.yaml"),
            roster_path: Some(PathBuf::from("/cfg/providers.yaml")),
            reason: reason.to_string(),
        }
    }

    #[test]
    fn reporter_emits_one_line_per_landing_and_one_per_new_landing() {
        let mut r = Reporter::new();
        let v = || Verdict::Refused(refusal("aaaa1111bbbb2222", "boom"));

        // One landing (one burst): the leading look's line, then the
        // trailing look re-read the same refused state — the SAME
        // landing, not a new one — and is not repeated.
        r.burst_started();
        let first = r.report(&v()).expect("the landing's line");
        assert!(r.report(&v()).is_none(), "same burst, same refusal: silent");

        // Two landings (two bursts): the operator who lands the same
        // broken pair twice gets two lines (D11: no de-duplication
        // across landings).
        r.burst_started();
        assert_eq!(r.report(&v()).as_deref(), Some(first.as_str()));

        // A no-change look and an accepted candidate are both silent
        // (D1; the accepted half's surface is R47-2's publish).
        assert!(r.report(&Verdict::NoChange).is_none());
        r.burst_started();
        assert!(r.report(&Verdict::NoChange).is_none());
    }

    #[test]
    fn refusal_line_carries_the_marker_the_digest_the_paths_and_the_reason() {
        let line = refusal_line(&refusal(
            "0123456789abcdef",
            "config file /cfg/config.yaml: does not parse: ...",
        ));
        // D11 item 1: the fixed marker distinguishing a reload's refusal
        // from a start's (a start's lines carry no `reload` word).
        assert!(line.starts_with("vadis: reload refused ("), "got: {line}");
        // item 2: the revision still served.
        assert!(
            line.contains("still serving revision 0123456789abcdef"),
            "got: {line}"
        );
        // item 4: the resolved paths the candidate was read from.
        assert!(
            line.contains("read /cfg/config.yaml and /cfg/providers.yaml"),
            "got: {line}"
        );
        // item 3: the loader's reason, verbatim (the whole string,
        // unedited — the assertion is on containment and position).
        assert!(
            line.ends_with("config file /cfg/config.yaml: does not parse: ..."),
            "got: {line}"
        );
        // One line is one line.
        assert!(!line.contains('\n'), "got: {line}");
        // The inline shape names the root alone.
        let line = refusal_line(&Refusal {
            roster_path: None,
            ..refusal("0123456789abcdef", "boom")
        });
        assert!(line.contains("read /cfg/config.yaml"), "got: {line}");
        assert!(!line.contains(" and "), "got: {line}");
    }

    // -------------------------------------------------------------
    // The gate: look() through the real loader.
    // -------------------------------------------------------------

    #[test]
    fn look_is_a_no_op_when_the_digest_matches() {
        let (_g, root) = tempdir("noop");
        std::fs::write(&root, MINIMAL).unwrap();
        let identity = load_identity(&root);
        match look(&root, &identity.config_digest, None) {
            Verdict::NoChange => {}
            other => panic!("same bytes, same digest: a no-op (D1), got {other:?}"),
        }
    }

    #[test]
    fn look_accepts_a_valid_change_as_a_candidate_unapplied() {
        let (_g, root) = tempdir("accept");
        std::fs::write(&root, MINIMAL).unwrap();
        let identity = load_identity(&root);
        // A comment-only edit moves the byte digest and no value
        // (ADR-037 D6) — still a candidate (the value-diff half of the
        // story is R47-2's `changed_keys`, not this card's).
        land(&root, &format!("# a note\n{MINIMAL}"));
        match look(&root, &identity.config_digest, None) {
            Verdict::Accepted(rc) => {
                assert_ne!(rc.identity.config_digest, identity.config_digest);
            }
            other => panic!("a changed pair that loads is an accepted candidate, got {other:?}"),
        }
    }

    #[test]
    fn look_refuses_an_illegal_candidate_with_the_loaders_reason_and_paths() {
        let (_g, root) = tempdir("refuse");
        std::fs::write(&root, MINIMAL).unwrap();
        let identity = load_identity(&root);
        land(&root, "server: [oops");
        match look(&root, &identity.config_digest, None) {
            Verdict::Refused(r) => {
                assert_eq!(r.serving_digest, identity.config_digest);
                assert_eq!(r.root_path, identity.root_path);
                assert!(r.reason.contains("does not parse"), "got: {}", r.reason);
                // The loader's reason names the file itself (the
                // loader's path-prefixed wrapper, verbatim).
                assert!(r.reason.contains("config.yaml"), "got: {}", r.reason);
            }
            other => panic!("an illegal candidate is refused, got {other:?}"),
        }
    }

    // -------------------------------------------------------------
    // The publish (R47-2): the accepted half's contract, on a real
    // cell and a real store (RV-1's order, RV-8's payload, D5's
    // refused set, the auth note, row 12's edges).
    // -------------------------------------------------------------

    /// Land `content` over the fixture and hand the publisher the
    /// candidate, exactly as `one_look` does for a digest that moved.
    fn apply_landing(publisher: &Publisher, root: &Path, content: &str) -> Result<(), Refusal> {
        land(root, content);
        let candidate = config_load::load(root).expect("the candidate loads");
        publisher.apply(candidate)
    }

    /// RV-1's order and RV-8's one-key case: the row commits before the
    /// publish, names its predecessor, and lists exactly the moved path;
    /// the cell then serves the new revision and the head is re-armed.
    #[test]
    fn an_accepted_switch_commits_the_row_then_publishes_then_re_arms() {
        let (_g, root) = tempdir("apply");
        std::fs::write(&root, MINIMAL).unwrap();
        let (sink, _lines) = capture();
        let (publisher, store, cell, _sink_handle) = publisher_for(&root, &sink);
        let startup_digest = publisher.head().0;

        apply_landing(&publisher, &root, &MINIMAL.replace("ttl: 12h", "ttl: 6h"))
            .expect("a one-key change applies");

        // The row: one config.applied, its payload the value diff against
        // the named predecessor (D10 / note R10).
        let rows = committed(&store, vadis_core::EventKind::ConfigApplied);
        assert_eq!(rows.len(), 1, "one row per accepted application");
        let payload = &rows[0];
        assert_eq!(
            payload["previous_config_digest"],
            serde_json::json!(startup_digest),
            "the row names the revision it replaced"
        );
        assert_eq!(
            payload["changed_keys"],
            serde_json::json!([{ "path": "session.ttl", "change": "changed" }]),
            "a one-key revision lists exactly that path"
        );
        let new_digest = payload["config_digest"].as_str().unwrap().to_string();
        assert_ne!(new_digest, startup_digest);

        // The publish: the cell serves the new revision, and the head is
        // re-armed — a second look at the same bytes finds no change (D1).
        assert_eq!(cell.capture().config_identity.config_digest, new_digest);
        assert_eq!(publisher.head().0, new_digest);
        match look(&root, &publisher.head().0, None) {
            Verdict::NoChange => {}
            other => panic!("the re-armed digest makes the same bytes a no-op, got {other:?}"),
        }
        // And a roster-only change crossed no plugin edge (D5).
        assert!(
            committed(&store, vadis_core::EventKind::PluginLoaded).is_empty(),
            "a revision with no plugin edge writes no row 12"
        );
    }

    /// RV-8's sharpest case: a comment-only revision — the digest moves
    /// (the identity hashes bytes), `changed_keys` is `[]`, and the
    /// predecessor is named. The two halves of row 13 measure different
    /// things, and this is the case that separates them.
    #[test]
    fn a_comment_only_revision_reports_no_changed_keys_beside_a_moved_digest() {
        let (_g, root) = tempdir("comment");
        std::fs::write(&root, MINIMAL).unwrap();
        let (sink, _lines) = capture();
        let (publisher, store, _cell, _sink_handle) = publisher_for(&root, &sink);
        let startup_digest = publisher.head().0;

        apply_landing(
            &publisher,
            &root,
            &format!("# a price citation note\n{MINIMAL}"),
        )
        .expect("a comment-only revision applies");

        let rows = committed(&store, vadis_core::EventKind::ConfigApplied);
        assert_eq!(rows.len(), 1);
        let payload = &rows[0];
        assert_eq!(
            payload["changed_keys"],
            serde_json::json!([]),
            "no VALUE moved"
        );
        assert_eq!(
            payload["previous_config_digest"],
            serde_json::json!(startup_digest)
        );
        assert_ne!(
            payload["config_digest"].as_str().unwrap(),
            startup_digest,
            "the digest is a byte measurement — it moved"
        );
    }

    /// D5 / RV-9's store half: a candidate touching a process-held key is
    /// refused with the key named, the revision in force keeps serving,
    /// and the store gains NO row (a refused key can never appear in a
    /// `changed_keys` list — the refusal names it instead).
    #[test]
    fn a_refused_key_is_named_and_nothing_is_committed_or_published() {
        let (_g, root) = tempdir("refused-key");
        std::fs::write(&root, MINIMAL).unwrap();
        let (sink, _lines) = capture();
        let (publisher, store, cell, _sink_handle) = publisher_for(&root, &sink);
        let startup_digest = publisher.head().0;

        let refusal = apply_landing(
            &publisher,
            &root,
            &MINIMAL.replace("127.0.0.1:8790", "127.0.0.1:8791"),
        )
        .expect_err("server.addr is refused");
        assert!(
            refusal.reason.contains("server.addr"),
            "the refusal names the key: {}",
            refusal.reason
        );
        assert!(
            refusal.reason.contains("restart"),
            "the remedy is named: {}",
            refusal.reason
        );
        assert_eq!(
            refusal.serving_digest, startup_digest,
            "the refusal names the revision still served"
        );
        assert!(
            committed(&store, vadis_core::EventKind::ConfigApplied).is_empty(),
            "a refused candidate writes no row"
        );
        assert_eq!(
            cell.capture().config_identity.config_digest,
            startup_digest,
            "the revision in force keeps serving"
        );
        // The refusal did not re-arm: the same landing, looked at again,
        // is refused again (not mistaken for already-applied).
        assert_eq!(publisher.head().0, startup_digest);
    }

    /// D5's honesty note: a revision that renames `server.auth_token_env`
    /// to a variable that is unset refuses the switch with the startup
    /// refusal's own reason — never quietly serving without a gate.
    #[test]
    fn a_revision_naming_an_unset_auth_var_is_refused_with_the_startup_reason() {
        std::env::remove_var("RELOAD_TEST_TOKEN_NEVER_SET");
        let (_g, root) = tempdir("auth-unset");
        std::fs::write(&root, MINIMAL).unwrap();
        let (sink, _lines) = capture();
        let (publisher, store, cell, _sink_handle) = publisher_for(&root, &sink);

        let with_auth = MINIMAL.replace(
            "upstream_attempt_timeout: 60s",
            "upstream_attempt_timeout: 60s, auth_token_env: RELOAD_TEST_TOKEN_NEVER_SET",
        );
        let refusal = apply_landing(&publisher, &root, &with_auth)
            .expect_err("an unset newly-named variable refuses the switch");
        assert!(
            refusal.reason.contains(
                "server.auth_token_env names RELOAD_TEST_TOKEN_NEVER_SET, which is unset"
            ),
            "the startup refusal's own reason: {}",
            refusal.reason
        );
        assert!(
            refusal.reason.contains("refusing"),
            "got: {}",
            refusal.reason
        );
        assert!(
            committed(&store, vadis_core::EventKind::ConfigApplied).is_empty(),
            "a refused candidate writes no row"
        );
        assert!(cell.capture().auth_gate.is_none());
    }

    /// Row 12's delta (D5): edges fire only where an entry's mount state
    /// or mounted value actually moved — add, remove, disable, enable,
    /// rebuild — and an unchanged entry writes nothing.
    #[test]
    fn plugin_edges_fire_only_at_crossed_edges() {
        use vadis_core::config::PluginCfg;
        let entry = |id: &str, kind: &str, disabled: bool| PluginCfg {
            id: id.into(),
            kind: kind.into(),
            config: None,
            url: None,
            inject: vec![],
            isolate: false,
            intercept: None,
            disabled,
        };
        let kinds: Vec<vadis_core::EventKind> = plugin_edges(
            &[
                entry("keep", "builtin/transform_rules", false),
                entry("gone", "builtin/transform_rules", false),
                entry("off", "builtin/transform_rules", false),
                entry("rebuilt", "builtin/transform_rules", false),
            ],
            &[
                entry("keep", "builtin/transform_rules", false),
                entry("off", "builtin/transform_rules", true), // disabled: an unload edge
                entry("rebuilt", "builtin/transform_rules", false), // same value: no edge
                entry("new", "process", false),
            ],
            "dddd5555eeee6666",
        )
        .into_iter()
        .map(|e| e.kind)
        .collect();
        // Sorted by id: gone (unloaded), new (loaded), off (unloaded);
        // `keep` crossed nothing. `rebuilt` is value-identical: nothing.
        assert_eq!(
            kinds,
            vec![
                vadis_core::EventKind::PluginUnloaded, // gone
                vadis_core::EventKind::PluginLoaded,   // new
                vadis_core::EventKind::PluginUnloaded, // off
            ]
        );
        // A changed mounted entry is a rebuild: unloaded then loaded.
        let mut changed = entry("rebuilt", "builtin/transform_rules", false);
        changed.config = Some(serde_json::json!({"rules": "other.toml"}));
        let kinds: Vec<vadis_core::EventKind> = plugin_edges(
            &[entry("rebuilt", "builtin/transform_rules", false)],
            &[changed],
            "d",
        )
        .into_iter()
        .map(|e| e.kind)
        .collect();
        assert_eq!(
            kinds,
            vec![
                vadis_core::EventKind::PluginUnloaded,
                vadis_core::EventKind::PluginLoaded
            ],
            "a changed entry is unloaded, then loaded"
        );
    }

    /// The in-flight half of the capture-once rule (ADR-040 D2/D3, and
    /// the digest-honesty note): a request that captured revision A at
    /// receive finishes AFTER the publish of B — and its record is still
    /// stamped with A's digest, because the digest travels WITH the
    /// captured revision's own writer. A writer shared one level up and
    /// stamped at commit time would stamp B: that shape fails this test.
    #[test]
    fn a_request_finishing_after_a_switch_is_stamped_by_the_revision_that_served_it() {
        use vadis_core::TraceWriter as _;

        let (_g, root) = tempdir("in-flight");
        std::fs::write(&root, MINIMAL).unwrap();
        let (sink, _lines) = capture();
        let (publisher, _store, cell, _sink_handle) = publisher_for(&root, &sink);

        // The request captures at receive (D2's one rule)…
        let in_flight = cell.capture();
        let served_digest = in_flight.config_identity.config_digest.clone();

        // …the reload publishes mid-flight…
        apply_landing(&publisher, &root, &MINIMAL.replace("ttl: 12h", "ttl: 6h"))
            .expect("the switch applies");
        let in_force = cell.capture().config_identity.config_digest.clone();
        assert_ne!(in_force, served_digest, "the publish happened");

        // …and the request finishes on the revision it captured: the
        // digest its record carries is read off THAT revision's writer.
        let writer = in_flight
            .forwarder
            .trace
            .as_ref()
            .expect("the rig wires a writer");
        let stamped = writer.config_digest().to_string();
        assert_eq!(stamped, served_digest, "stamped by the serving revision");
        assert_ne!(stamped, in_force, "…not by the one now in force");

        // The record itself — the guard's own constructor, the same write
        // call the request path makes — lands in the trace stamped with A
        // even though the cell already serves B.
        let verdict = match vadis_proxy::AuthGate::new("t".to_string()).admits(&[]) {
            v @ vadis_proxy::AuthVerdict::Refused { .. } => v,
            vadis_proxy::AuthVerdict::Admitted => panic!("a presented-less request is refused"),
        };
        let record = vadis_proxy::refused_record(
            "req-in-flight",
            vadis_core::config::WireApi::Chat.as_str(),
            &verdict,
            1_800_000_000,
            0,
            &stamped,
        );
        writer.write(&record).expect("the write lands");
        let dir = root.parent().unwrap().join("state/traces");
        let mut found = false;
        for entry in std::fs::read_dir(&dir).expect("the trace dir exists") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            for line in std::fs::read_to_string(&path).unwrap().lines() {
                let v: serde_json::Value = serde_json::from_str(line).unwrap();
                if v["identity"]["request_id"] == "req-in-flight" {
                    assert_eq!(
                        v["config_digest"],
                        serde_json::json!(served_digest),
                        "the record is stamped with the digest that served it"
                    );
                    found = true;
                }
            }
        }
        assert!(found, "the in-flight record is in the trace");
    }

    // -------------------------------------------------------------
    // The rigs: the real watcher on real directories (the platform
    // backend included), the landing performed by temp+rename.
    // -------------------------------------------------------------

    fn capture() -> (Arc<dyn Fn(String) + Send + Sync>, Arc<Mutex<Vec<String>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let lines2 = lines.clone();
        (
            Arc::new(move |line| lines2.lock().unwrap().push(line)),
            lines,
        )
    }

    /// A publisher over a real cell, store and sink — the shape `serve`
    /// wires (the startup revision is built by the same `build_revision`,
    /// its row committed by the caller where the contract wants one).
    /// The strong refs come back with the rig: the publisher holds them
    /// weak (its ownership rule), so the fixture must keep them alive for
    /// the upgrades a `apply` performs.
    #[allow(clippy::type_complexity)]
    fn publisher_for(
        root: &Path,
        sink: &Arc<dyn Fn(String) + Send + Sync>,
    ) -> (
        Arc<Publisher>,
        Arc<dyn vadis_core::store::Store>,
        vadis_proxy::SharedRevision,
        Arc<vadis_store::TraceSink>,
    ) {
        let rc = config_load::load(root).expect("the fixture loads");
        let store: Arc<dyn vadis_core::store::Store> =
            Arc::new(vadis_store::SqliteStore::open(&rc.state_db).expect("the store opens"));
        let trace_sink =
            Arc::new(vadis_store::TraceSink::open(&rc.trace_dir).expect("the trace sink opens"));
        let note = |_: String| {};
        let revision = crate::build_revision(&rc, &store, &trace_sink, None, &note);
        let cell = vadis_proxy::RevisionCell::new(revision);
        let publisher = Arc::new(Publisher::new(
            &cell,
            &store,
            &trace_sink,
            &rc.identity,
            None,
            sink.clone(),
        ));
        (publisher, store, cell, trace_sink)
    }

    /// The committed rows of one kind, in log order (the CONF-85 pattern:
    /// the payload is read back out of the store, never inferred).
    fn committed(
        store: &Arc<dyn vadis_core::store::Store>,
        kind: vadis_core::EventKind,
    ) -> Vec<serde_json::Value> {
        use vadis_core::store::{Query, QueryRow, Store as _};
        match store.query(Query::AllEvents).expect("the log reads") {
            QueryRow::Events(rows) => rows
                .into_iter()
                .filter(|r| r.kind == Some(kind))
                .map(|r| r.payload)
                .collect(),
            other => panic!("AllEvents reads events, got {other:?}"),
        }
    }

    /// Poll until `cond` holds or the budget runs out. The platform
    /// backends deliver asynchronously; a poll is honest about that in a
    /// way a fixed sleep is not.
    fn until(cond: impl Fn() -> bool, budget: Duration, what: &str) {
        let start = Instant::now();
        while !cond() {
            assert!(
                start.elapsed() < budget,
                "timed out waiting for {what} (budget {budget:?})"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// The card's hot-directory rig arm: store and trace writes in the
    /// watched directory produce **no look and no line** (ADR-040 D12.4's
    /// measured point — the macOS backend delivers sub-directory events,
    /// so the exact-path filter is what stands between the reload and a
    /// re-read of the pair per request).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hot_directory_noise_produces_no_look_and_no_line() {
        let (_g, root) = tempdir("hot");
        std::fs::write(&root, MINIMAL).unwrap();
        let identity = load_identity(&root);
        let dir = root.parent().unwrap().to_path_buf();

        let (sink, lines) = capture();
        let (publisher, store, _cell, _sink_handle) = publisher_for(&root, &sink);
        let watcher = Watcher::start(&identity, sink, publisher).expect("watcher starts");
        // Registration is asynchronous (the backend's stream start is
        // slow; see Watcher's doc) — wait it out so the noise below is
        // genuinely delivered to the filter, not lost before it.
        until(
            || watcher.is_ready(),
            Duration::from_secs(15),
            "the watcher registration",
        );

        // The noise, in the real shapes: the state store (with its WAL)
        // at <dir>/state/vadis.db, trace files under <dir>/state/
        // traces/, and a stray temp file directly in the watched
        // directory (the landing's own first half, filtered by name).
        // The store write goes through the publisher's own connection —
        // the file (and its WAL) moves exactly as a serving process's
        // write moves it.
        store
            .append(vadis_core::NewEvent {
                kind: vadis_core::EventKind::ConfigApplied,
                request_id: None,
                session: None,
                body_hash: None,
                trace_ref: None,
                payload: serde_json::json!({"note": "hot-directory noise"}),
            })
            .unwrap();
        std::fs::create_dir_all(dir.join("state/traces")).unwrap();
        std::fs::write(
            dir.join("state/traces").join("2026-09-26T00.jsonl"),
            b"{}\n",
        )
        .unwrap();
        std::fs::write(dir.join("config.landing-tmp"), b"not a landing").unwrap();

        // Give the backend ample time to deliver every event (latency
        // 0 on this backend, but delivery is still asynchronous), then
        // sit through more than one full window: any event that slipped
        // the filter would have produced a look by then.
        std::thread::sleep(Duration::from_millis(
            3 * COALESCE_WINDOW.as_millis() as u64 + 300,
        ));
        assert_eq!(
            watcher.look_count(),
            0,
            "hot-directory noise must not trigger a look (D12.4)"
        );
        assert!(lines.lock().unwrap().is_empty(), "and no line: {lines:?}");
        drop(store);
        drop(watcher);
    }

    /// The card's cheapest honest test, driven through the real watcher:
    /// an illegal candidate lands (temp + rename, the real landing) and
    /// the process's report is **exactly one line** naming the reload,
    /// the revision still served, the loader's reason and the path read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_landing_reports_exactly_one_line() {
        let (_g, root) = tempdir("refused-landing");
        std::fs::write(&root, MINIMAL).unwrap();
        let identity = load_identity(&root);

        let (sink, lines) = capture();
        let (publisher, _store, _cell, _sink_handle) = publisher_for(&root, &sink);
        let watcher = Watcher::start(&identity, sink, publisher).expect("watcher starts");
        until(
            || watcher.is_ready(),
            Duration::from_secs(15),
            "the watcher registration",
        );

        land(&root, "server: [oops");
        until(
            || watcher.look_count() >= 1,
            Duration::from_secs(10),
            "the leading look after the landing",
        );
        // Sit through a full window so a folded event's trailing look —
        // if the backend split the landing into two matching events —
        // has also run. One landing must still be one line.
        std::thread::sleep(Duration::from_millis(
            2 * COALESCE_WINDOW.as_millis() as u64 + 300,
        ));

        let lines = lines.lock().unwrap();
        assert_eq!(lines.len(), 1, "one landing, one line: {lines:?}");
        let line = &lines[0];
        assert!(line.starts_with("vadis: reload refused ("), "got: {line}");
        assert!(
            line.contains(&format!(
                "still serving revision {}",
                identity.config_digest
            )),
            "got: {line}"
        );
        assert!(
            line.contains("does not parse"),
            "the loader's reason, verbatim: {line}"
        );
        assert!(
            line.contains(&identity.root_path.display().to_string()),
            "the path read: {line}"
        );
        drop(watcher);
    }

    /// The no-op half through the real watcher: a landing that changes
    /// no byte of either file's content as loaded is a no-op (D1) — a
    /// look may run (the event is only a hint), but nothing is reported.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_same_bytes_landing_reports_nothing() {
        let (_g, root) = tempdir("same-bytes");
        std::fs::write(&root, MINIMAL).unwrap();
        let identity = load_identity(&root);

        let (sink, lines) = capture();
        let (publisher, _store, _cell, _sink_handle) = publisher_for(&root, &sink);
        let watcher = Watcher::start(&identity, sink, publisher).expect("watcher starts");
        until(
            || watcher.is_ready(),
            Duration::from_secs(15),
            "the watcher registration",
        );

        land(&root, MINIMAL);
        std::thread::sleep(Duration::from_millis(
            3 * COALESCE_WINDOW.as_millis() as u64 + 300,
        ));
        assert!(
            lines.lock().unwrap().is_empty(),
            "a no-change look is nothing observable (D1)"
        );
        drop(watcher);
    }

    // -------------------------------------------------------------
    // RV-9 through the real `serve` assembly: a refused candidate in a
    // RUNNING process — /health's digest unchanged, the store gaining
    // no row, the service uninterrupted. The line's content and count
    // are pinned by the rig above against the same sink seam `serve`
    // wires to stderr; the process-level run evidence (the real binary's
    // stderr, one line) is the landing's own tracked probe run.
    // -------------------------------------------------------------

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn http_get(addr: &str, path: &str) -> (u16, String) {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(addr).expect("connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let status: u16 = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .expect("status line");
        let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    fn health_digest(addr: &str) -> String {
        let (status, body) = http_get(addr, "/health");
        assert_eq!(status, 200, "service continues: /health answers");
        let v: serde_json::Value = serde_json::from_str(&body).expect("health json");
        v["config"]["config_digest"]
            .as_str()
            .expect("the digest member")
            .to_string()
    }

    /// The accepted half through the real process (RV-1 and RV-8 end to
    /// end): a one-key landing on a running serve moves /health's digest
    /// to the new bytes' own digest, the process keeps serving, and the
    /// log holds exactly two `config.applied` rows — the startup's (both
    /// switch members null) and the switch's (the predecessor named,
    /// exactly the moved path listed).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_accepted_landing_switches_the_served_revision() {
        let (_g, root) = tempdir("accepted-serve");
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let config = MINIMAL.replace("127.0.0.1:8790", &addr);
        std::fs::write(&root, &config).unwrap();
        let startup = load_identity(&root).config_digest;

        let cfg = root.to_string_lossy().into_owned();
        let serve_task = tokio::spawn(async move { crate::serve(&cfg).await });
        until(
            || std::net::TcpStream::connect(&addr).is_ok(),
            Duration::from_secs(10),
            "serve to listen",
        );
        assert_eq!(health_digest(&addr), startup);

        // The registration is asynchronous and its readiness is not
        // visible from outside the process — a landing that arrives
        // before it is an event nobody heard (D12.6: the next event is
        // the next look). So land, and if the switch has not happened,
        // land again: each landing is a fresh event, and once the
        // registration is live one of them delivers. (A fixed sleep here
        // lost to the backend's stream start under test parallelism.)
        land(&root, &config.replace("ttl: 12h", "ttl: 6h"));
        let expected = load_identity(&root).config_digest;
        assert_ne!(expected, startup, "the landing moved the bytes");

        // The switch, delivered: poll /health for the new digest rather
        // than sleeping a fixed window (delivery + coalesce + the
        // publish, all off the request path).
        let mut switched = false;
        for _ in 0..15 {
            for _ in 0..10 {
                if health_digest(&addr) == expected {
                    switched = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if switched {
                break;
            }
            // Not yet: either the registration is still starting or the
            // coalescer is mid-burst — a fresh event either way.
            land(&root, &config.replace("ttl: 12h", "ttl: 6h"));
        }
        assert!(
            switched,
            "RV-1: /health answers for the revision in force after the publish"
        );
        assert!(
            !serve_task.is_finished(),
            "an accepted switch never stops the process"
        );
        serve_task.abort();
        let _ = serve_task.await;

        // The log: exactly the two config.applied rows, nothing else (no
        // request was served, no plugin edge was crossed).
        let store =
            vadis_store::SqliteStore::open(&root.parent().unwrap().join("state/vadis.db")).unwrap();
        use vadis_core::store::{Query, QueryRow, Store as _};
        let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
            panic!("events")
        };
        let applied: Vec<_> = events
            .iter()
            .filter(|e| e.kind_raw == "config.applied")
            .collect();
        assert_eq!(applied.len(), 2, "one row per revision: {events:?}");
        assert!(
            applied[0].payload["previous_config_digest"].is_null(),
            "RV-8's startup state: no predecessor to name"
        );
        assert!(
            applied[0].payload["changed_keys"].is_null(),
            "RV-8's startup state: nothing to diff against"
        );
        assert_eq!(
            applied[1].payload["previous_config_digest"],
            serde_json::json!(startup),
            "the switch's row names the revision it replaced"
        );
        assert_eq!(
            applied[1].payload["config_digest"],
            serde_json::json!(expected)
        );
        assert_eq!(
            applied[1].payload["changed_keys"],
            serde_json::json!([{ "path": "session.ttl", "change": "changed" }]),
            "a one-key revision lists exactly that path"
        );
        assert_eq!(events.len(), 2, "no other row of any kind: {events:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rv9_a_refused_candidate_leaves_a_running_process_unchanged() {
        let (_g, root) = tempdir("rv9");
        let port = free_port();
        let addr = format!("127.0.0.1:{port}");
        let config = MINIMAL.replace("127.0.0.1:8790", &addr);
        std::fs::write(&root, &config).unwrap();
        let identity = load_identity(&root);

        let cfg = root.to_string_lossy().into_owned();
        let serve_task = tokio::spawn(async move { crate::serve(&cfg).await });
        until(
            || std::net::TcpStream::connect(&addr).is_ok(),
            Duration::from_secs(10),
            "serve to listen",
        );

        let before = health_digest(&addr);
        assert_eq!(before, identity.config_digest);

        // The watcher registers asynchronously (the backend's stream
        // start is slow; see reload::Watcher's doc) — wait past it, so
        // the landing below is an event the registration can deliver.
        std::thread::sleep(Duration::from_secs(3));

        // The landing the loader refuses.
        land(&root, "server: [oops");
        // The watcher is inside the process and reports on its own
        // stderr; this test's non-vacuity rests on the rig above (the
        // same constructor, the same loop, a capturing sink) plus the
        // tracked process-level probe. Here: wait out the delivery +
        // window, then hold the four inert facts.
        std::thread::sleep(Duration::from_secs(2));

        // 1. /health's digest is the pre-attempt value…
        assert_eq!(health_digest(&addr), before, "RV-9: /health unchanged");
        // 2. …and the service is uninterrupted (a second fetch, answered).
        assert_eq!(health_digest(&addr), before, "RV-9: still serving");

        // 4. The process is the same process: no exit code 2/4 is
        // attributable to a reload — the serve task is still running
        // (a refusal that exited would have finished it).
        assert!(
            !serve_task.is_finished(),
            "RV-9: a refused candidate never stops the process"
        );
        serve_task.abort();
        let _ = serve_task.await;

        // 3. The store gained no row of any kind: this fixture declares
        // no plugins and serves no request, so its boot writes exactly
        // the startup `config.applied` — the refusal adds nothing (RV-2,
        // RV-3, RV-9: a refused candidate writes nothing).
        let store =
            vadis_store::SqliteStore::open(&root.parent().unwrap().join("state/vadis.db")).unwrap();
        use vadis_core::store::{Query, QueryRow, Store as _};
        let QueryRow::Events(events) = store.query(Query::AllEvents).unwrap() else {
            panic!("events")
        };
        assert_eq!(
            events.len(),
            1,
            "RV-9: the store gains no row — only the boot's config.applied exists: {events:?}"
        );
        assert_eq!(events[0].kind_raw, "config.applied");
        // RV-8's startup state, on the row the real process wrote: the
        // two switch members are null — there is no predecessor to name
        // and nothing to diff against (ADR-040 D10: one flat object on
        // every row 13, so a reader never asks which vintage it holds).
        assert!(
            events[0].payload["previous_config_digest"].is_null(),
            "startup carries no predecessor: {:?}",
            events[0].payload
        );
        assert!(
            events[0].payload["changed_keys"].is_null(),
            "startup carries no diff: {:?}",
            events[0].payload
        );
    }
}
