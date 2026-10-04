//! The exact-match response cache (spec §4.17, ADR-042, DESIGN §12.22):
//! the **one** key derivation and the **one** store, owned here so no
//! second derivation and no second store can exist anywhere else.
//!
//! **What a hit proves, and what it never claims** (ADR-042 §3.2 — the
//! sentence that is the honesty of the whole feature): a hit proves
//! exactly that *these exact bytes were sent before* — in this session,
//! under this revision, under this protocol, under this mode — and that
//! *the response bytes returned then are the response bytes returned
//! now*, verbatim. **A hit is a replay, not a prediction**: the bytes
//! came from a recorded response, and the vadis asserts nothing about
//! what the provider would answer for this request at this moment — not
//! its content, not its price, not its liveness.
//!
//! **No clock is read anywhere in this module** (ADR-042 §2.2): no TTL,
//! no timestamp comparison, no "stale after N". An entry leaves the
//! store only by FIFO eviction under the two frozen bounds or by the
//! owning fiber's unload, so the sequence of hits, misses and evictions
//! is a pure function of the request sequence and the frozen constants
//! (AGENTS constraint 2). The store is in-process memory and nothing
//! else: no file, no store row, no event-log write — and it holds
//! response bytes plus the source reference, **no derived figure** a
//! report could sum (ADR-042 §3.4/§9.3).

use sha2::{Digest, Sha256};

use crate::transform::TransformMode;

/// The entry bound (spec §4.17 — a frozen constant, stated in-band,
/// never a config key).
pub const MAX_ENTRIES: usize = 1024;
/// The byte bound, 64 MiB (spec §4.17): an entry bound alone cannot
/// bound memory, because a response's size is the upstream's choice.
pub const MAX_STORED_BYTES: u64 = 64 << 20;

/// What the key derivation reads (ADR-042 §3.1): values the pipeline
/// already holds at the lookup's position — the last step before the
/// upstream attempt (§4.4). `body` is the client's inbound body bytes
/// **exactly as received**, before and independently of the two
/// permitted mutations; `session` is required — a request with no
/// session has no key and is neither looked up nor stored (§3.3).
pub struct RequestFacts<'a> {
    /// The request's own inbound protocol (path-derived, spec §2).
    pub protocol_in: &'a str,
    /// The revision in force — the same digest the request's own record
    /// is stamped with (ADR-037 D6), so the key and the record can never
    /// disagree about which revision served the request.
    pub config_digest: &'a str,
    /// spec §4's `key_sources`, preferring `prompt_cache_key`.
    pub session: &'a str,
    /// spec §2.1's mode word — two requests with identical bodies and
    /// different mode words produce different outbound bodies, so the
    /// key must separate them.
    pub transform_mode: TransformMode,
    /// The client's inbound body bytes as received — the content
    /// component, and the only one that is bytes.
    pub body: &'a [u8],
}

/// The five-component key (ADR-042 §3.1). **One derivation** —
/// [`ResponseKey::for_request`] — and no normalisation of any kind: the
/// digest is taken over the client's own bytes, so a change to a
/// mutation cannot silently change what is matched, and a component
/// already inside the digest (the client's `model` string, its `stream`
/// scalar) is not repeated.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResponseKey {
    protocol_in: String,
    config_digest: String,
    session: String,
    /// `TransformMode::as_str()` — the mode word itself, not a copy of
    /// the enum (the enum is not `Hash`; the word is the component).
    transform_mode: &'static str,
    /// sha256 of the client's inbound body bytes as received.
    key_digest: [u8; 32],
}

impl ResponseKey {
    /// The one key derivation (ADR-042 §9.1): a pure function of the
    /// request's own facts — no clock, no I/O, no second copy anywhere.
    pub fn for_request(facts: &RequestFacts<'_>) -> ResponseKey {
        ResponseKey {
            protocol_in: facts.protocol_in.to_string(),
            config_digest: facts.config_digest.to_string(),
            session: facts.session.to_string(),
            transform_mode: facts.transform_mode.as_str(),
            key_digest: Sha256::digest(facts.body).into(),
        }
    }

    /// The record's `cache.key_digest` (spec §6): the full sha256 hex of
    /// the client's inbound body bytes as received — the one key
    /// component the record does not already carry, so the whole key is
    /// reconstructible from the record that discloses it (§3.1).
    pub fn key_digest_hex(&self) -> String {
        let mut out = String::with_capacity(64);
        for b in self.key_digest {
            out.push_str(&format!("{b:02x}"));
        }
        out
    }

    /// The session component — the recorded source reference's session
    /// is this value by construction (the reference can never point
    /// across sessions, ADR-042 §4.3).
    pub fn session(&self) -> &str {
        &self.session
    }
}

/// The source reference (ADR-042 §4.2): the id of the record the bytes
/// came from, its session, and the turn index that record itself
/// reported — resolvable with the trace's own join key (`request_id`),
/// and necessarily of the same revision as any hit it produces (the key
/// pins `config_digest`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRef {
    pub request_id: String,
    pub session: String,
    pub turn_index: u32,
}

/// What the store holds: the recorded response bytes, the recorded
/// status and content-type, and the source reference — **nothing
/// derived** (no token estimate, no price, no count a report could sum;
/// the store is not a second ledger, ADR-042 §3.4/§9.3).
#[derive(Debug, Clone)]
pub struct RecordedResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub source: SourceRef,
}

/// The serving path's handle on the one store (ADR-042 §9.1): the
/// plugin's assembled fiber owns the concrete [`ResponseStore`] and the
/// request path reaches it through this port at exactly one call site
/// per forwarding medium. Both directions are clock-free and I/O-free.
pub trait ResponseCache: Send + Sync {
    /// The recorded response for this exact key, if one is held.
    fn lookup(&self, key: &ResponseKey) -> Option<RecordedResponse>;
    /// Offer a response to the store. Fail-closed (ADR-042 §3.3): a
    /// non-`2xx` status or a body past the byte bound is refused here,
    /// whatever the caller knows.
    fn record(&self, key: ResponseKey, response: RecordedResponse);
}

/// The one store (ADR-042 §3.4): in-process memory, FIFO eviction by
/// insertion under the two frozen bounds. The hit/miss/eviction sequence
/// is a pure function of the insertion sequence and the constants —
/// that is what the no-clock determinism claim rests on (§2.2).
#[derive(Default)]
pub struct ResponseStore {
    entries: std::collections::HashMap<ResponseKey, RecordedResponse>,
    /// Insertion order, oldest first — the FIFO the bounds evict by.
    order: std::collections::VecDeque<ResponseKey>,
    stored_bytes: u64,
}

impl ResponseStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The recorded response for this exact key, if one is held.
    pub fn lookup(&self, key: &ResponseKey) -> Option<&RecordedResponse> {
        self.entries.get(key)
    }

    /// How many entries are held (bounded by [`MAX_ENTRIES`]).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds no entry (clippy's `len`/`is_empty` pair).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The bytes currently held (bounded by [`MAX_STORED_BYTES`]).
    pub fn stored_bytes(&self) -> u64 {
        self.stored_bytes
    }

    /// Record a response under its key.
    ///
    /// Fail-closed (ADR-042 §3.3): a non-`2xx` status is never stored,
    /// and an entry whose own size exceeds [`MAX_STORED_BYTES`] is never
    /// stored. Otherwise the entry is inserted and the bounds are
    /// re-established by evicting in insertion order until it fits.
    ///
    /// A key that is already held is replaced and re-enters at the back
    /// of the insertion order (the ADR is silent here — the serving path
    /// can only re-record after an eviction or a previously unstored
    /// class; the rule keeps "FIFO by insertion" a pure function of the
    /// insertion sequence).
    pub fn record(&mut self, key: ResponseKey, response: RecordedResponse) {
        if !(200..300).contains(&response.status) {
            return; // an error is not an outcome worth replaying (§3.3)
        }
        let len = response.body.len() as u64;
        if len > MAX_STORED_BYTES {
            return; // a too-large body is simply not a candidate (§3.3)
        }
        if let Some(old) = self.entries.remove(&key) {
            self.stored_bytes -= old.body.len() as u64;
            self.order.retain(|k| k != &key);
        }
        self.stored_bytes += len;
        self.order.push_back(key.clone());
        self.entries.insert(key, response);
        while self.entries.len() > MAX_ENTRIES || self.stored_bytes > MAX_STORED_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.stored_bytes -= evicted.body.len() as u64;
            }
        }
    }
}
