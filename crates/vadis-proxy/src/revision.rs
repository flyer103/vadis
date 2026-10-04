//! The published revision and the capture-once seam (ADR-040 D2/D3; DESIGN
//! §12.20): the reload publishes **one immutable value** — the whole serving
//! assembly for one `config_digest` — and the request path takes that value
//! **once, at receive**, reading it for the request's whole lifetime.
//!
//! The mechanism is the one DESIGN §12.20 leaves to the implementer ("Arc
//! swap, an epoch, a RwLock read once — a code shape"): a `RwLock` around
//! an `Arc`, where the read half clones the Arc and releases the lock
//! before the request begins (nothing awaits while holding it), and the
//! write half — the reload's publish, its **only** writer — replaces the
//! Arc after the `config.applied` row is committed (step 3 before step 5,
//! so RV-1's "the row precedes the record" holds by construction).
//!
//! What this buys, structurally:
//!
//! - **One revision per request (D2).** A request's view is the Arc it
//!   captured; nothing behind it re-reads the cell, opens a file, or asks
//!   "what is current" mid-flight.
//! - **In-flight requests finish on the revision they captured (D3).** The
//!   old `Arc<Revision>` lives exactly as long as its last holder — the
//!   two-revision window is bounded by the last in-flight request, with no
//!   deadline, no timer and no drain knob (the no-pin arm; the
//!   session-level question is the owner's, ADR-040's open A1).
//! - **The record and its revision cannot disagree.** The revision bundles
//!   its own trace writer, stamped with its own digest
//!   (`ConfigTraceWriter`'s discipline one level up), so a request
//!   finishing after a switch is still priced — and stamped — by the
//!   revision that served it.

use std::sync::{Arc, RwLock};

use crate::auth::AuthGate;
use crate::forward::Forwarder;
use crate::health::{ConfigIdentity, ProviderKeyFacts};

/// One published revision: the immutable value one accepted load of the
/// pair produces (DESIGN §12.20 — "the validated `VadisConfig`, the
/// resolved paths and the identity, and whatever the runtime mounted for
/// it"). Everything per-revision the request path or `/health` reads lives
/// here; the process-level facts (the bound listener, the store path, the
/// trace directory, the auth gates) never join this struct — that is D5's
/// refused set, enforced by the publish before a revision gets this far.
pub struct Revision {
    /// The serving engine: the validated config, one transport per
    /// provider with a key present, the api keys, the mounted transform
    /// engine, the session TTL in µs, and the trace writer carrying THIS
    /// revision's digest.
    pub forwarder: Forwarder,
    /// `/health`'s `config` member (spec §9.1): the revision's identity,
    /// as strings (ADR-037 D4 — the proxy never opens, resolves or hashes
    /// a config file).
    pub config_identity: ConfigIdentity,
    /// `/health`'s provider facts (spec §9.1/§4.8), probed at the
    /// revision's build: key presence per provider beside the declared
    /// region and currency.
    pub provider_keys: Vec<ProviderKeyFacts>,
    /// The inbound token gate for this revision (spec §4.7, ADR-040 D5's
    /// honesty note): `server.auth_token_env`'s *name* is per-revision —
    /// the publish resolves the newly-named variable (refusing the switch
    /// when it is unset) so a revision that adds, drops or renames the key
    /// takes effect without a restart. The *value* of an unchanged name is
    /// the startup's one read, unchanged — §4.7's "read once" contract.
    pub auth_gate: Option<AuthGate>,
}

/// The published handle: one store a reader either sees complete or does
/// not see (D2 step 5). Shared as `Arc<RevisionCell>` between the routes
/// (readers, via [`RevisionCell::capture`]) and the reload's publisher
/// (the only writer, via [`RevisionCell::publish`]).
pub struct RevisionCell {
    current: RwLock<Arc<Revision>>,
}

/// The shared handle's name at the wiring sites.
pub type SharedRevision = Arc<RevisionCell>;

impl RevisionCell {
    /// The startup revision is published exactly as a switch's is: the
    /// cell is born holding it, after its `config.applied` row committed.
    pub fn new(rev: Revision) -> SharedRevision {
        Arc::new(Self {
            current: RwLock::new(Arc::new(rev)),
        })
    }

    /// **The** capture-once read (ADR-040 D2; DESIGN §12.20's one rule):
    /// a request takes the published revision here, once, at receive, and
    /// reads that revision for its whole lifetime. The lock is held only
    /// for the Arc clone — never across an await.
    ///
    /// A poisoned lock means a publisher panicked mid-publish: a process
    /// defect, not a degradation path (there is no honest "keep serving"
    /// answer to a half-written cell, and the publish cannot half-write —
    /// the panic would have had to happen between two field writes of one
    /// assignment).
    pub fn capture(&self) -> Arc<Revision> {
        self.current
            .read()
            .expect("the revision cell is poisoned: a publisher panicked mid-publish")
            .clone()
    }

    /// The publish (D2 step 5): replaces the revision readers see. The
    /// caller has already committed this revision's `config.applied` row
    /// (step 3) — the row precedes the effect it authorizes (ADR-010), so
    /// no record can ever carry a digest no row names (RV-1).
    pub fn publish(&self, rev: Revision) {
        *self
            .current
            .write()
            .expect("the revision cell is poisoned: a publisher panicked mid-publish") =
            Arc::new(rev);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vadis_core::config::VadisConfig;

    fn fixture(digest: &str) -> Revision {
        // The smallest Forwarder: an empty config is never valid, but the
        // cell does not read it — the fixture exists to be pointed at.
        let config: VadisConfig =
            serde_json::from_value(serde_json::json!({
                "server": {"addr": "127.0.0.1:1", "upstream_attempt_timeout": "60s", "request_timeout": "10m"},
                "session": {"key_sources": ["prompt_cache_key"], "ttl": "12h"},
                "cache": {"sticky": true, "breakeven": {"enabled": true, "min_remaining_turns": 3, "safety_factor": 1.2}},
                "trace": {"dir": "./state/traces", "rollover": "hourly"},
                "providers": [],
                "aliases": {},
                "plugins": [],
                "fallback": []
            }))
            .unwrap();
        Revision {
            forwarder: Forwarder {
                config,
                transports: Default::default(),
                api_keys: Default::default(),
                store: None,
                trace: None,
                transform_engine: None,
                response_cache: None,
                session_ttl_us: 0,
            },
            config_identity: ConfigIdentity {
                root_path: String::new(),
                roster_path: None,
                root_sha16: String::new(),
                roster_sha16: String::new(),
                config_digest: digest.to_string(),
            },
            provider_keys: Vec::new(),
            auth_gate: None,
        }
    }

    /// The seam's contract in one test: a capture taken BEFORE a publish
    /// still reads the revision it took (an in-flight request finishes on
    /// the revision it captured, D3), a capture after it reads the new
    /// one, and the old revision is dropped with its last holder.
    #[test]
    fn a_capture_survives_a_publish_and_dies_with_its_last_holder() {
        let cell = RevisionCell::new(fixture("aaaa1111bbbb2222"));
        let in_flight = cell.capture();
        assert_eq!(in_flight.config_identity.config_digest, "aaaa1111bbbb2222");

        cell.publish(fixture("cccc3333dddd4444"));

        // The captured Arc is the old revision, immutably (D4: a
        // published revision is never patched in place).
        assert_eq!(in_flight.config_identity.config_digest, "aaaa1111bbbb2222");
        // …while a new capture takes the revision in force.
        assert_eq!(
            cell.capture().config_identity.config_digest,
            "cccc3333dddd4444"
        );
        // The window's bound is the last holder, literally: drop the
        // in-flight capture and the old revision's strong count is gone.
        assert_eq!(Arc::strong_count(&in_flight), 1);
        drop(in_flight);
        assert_eq!(
            Arc::strong_count(&cell.capture()),
            2,
            "the cell and this capture"
        );
    }
}
