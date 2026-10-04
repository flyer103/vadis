//! The `builtin/response_cache` tier-A plugin (spec §4.17, ADR-042,
//! DESIGN §12.22): the mount that owns the store's lifetime. The fiber's
//! own lifetime **is** the store's — an entry leaves the store only by
//! FIFO eviction under the frozen bounds or by the fiber's unload (a
//! config change that rebuilds the entry, or `disabled: true`); no clock
//! is read anywhere on the lookup path (ADR-042 §2.2).
//!
//! **Off by default** (spec §4.17, ADR-042 §6.1): `config.enabled` is the
//! capability's own opt-in and its default is **`false`** — an entry
//! that omits the key, or writes `false`, is **mounted inert**: the
//! fiber is loaded (the mechanism's `disabled` is a different switch)
//! but it provides nothing into the serving path, reads no store and
//! answers no request differently from a build that does not know the
//! kind. Listing the plugin cannot silently enable it.
//!
//! The store itself — the one key derivation and the one store — lives
//! in `vadis_core::response_cache` (the single-owner rule, ADR-042 §9);
//! this module is only the mount and the serving path's handle. It
//! reads nothing but the request-path values the pipeline hands it, and
//! the trace JSONL stays the only product-side observation channel
//! (ADR-005): the module's own grep for the loop's directory name must
//! stay 0.

use std::sync::{Arc, Mutex};

use vadis_core::config::PluginCfg;
use vadis_core::response_cache::{RecordedResponse, ResponseCache, ResponseKey, ResponseStore};
use vadis_runtime::{Ctx, Effect, Plugin, PluginError, PluginId, ServiceId};

/// The serving path's handle on the one store (ADR-042 §9.1: reached
/// through the plugin's assembled handle at the one seam in
/// `forward.rs` — never a second store, never a cache-of-the-cache).
/// The mutex is held only for the in-memory probe or insert: no I/O, no
/// clock, no allocation beyond the clone of the recorded bytes.
pub struct ResponseCacheHandle {
    store: Mutex<ResponseStore>,
}

impl ResponseCacheHandle {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(ResponseStore::new()),
        }
    }
}

impl Default for ResponseCacheHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl ResponseCache for ResponseCacheHandle {
    fn lookup(&self, key: &ResponseKey) -> Option<RecordedResponse> {
        // No clock read on the lookup path (ADR-042 §2.2): the recorded
        // bytes are served exactly as held, or the request is a miss.
        self.store.lock().unwrap().lookup(key).cloned()
    }

    fn record(&self, key: ResponseKey, response: RecordedResponse) {
        // The store's own fail-closed rules (ADR-042 §3.3) refuse a
        // non-2xx status and an oversized body here, whatever the
        // caller knows.
        self.store.lock().unwrap().record(key, response);
    }
}

/// The plugin the loader mounts. `handle: None` is the **inert** mount
/// (spec §4.17): `apply` provides nothing, so the assembled context's
/// slot stays empty and the request path is byte-identical to a build
/// that does not know the kind (CONF-88).
pub(crate) struct ResponseCachePlugin {
    id: PluginId,
    inject: &'static [ServiceId],
    handle: Option<Arc<dyn ResponseCache>>,
}

impl Plugin for ResponseCachePlugin {
    fn id(&self) -> &PluginId {
        &self.id
    }

    fn inject(&self) -> &'static [ServiceId] {
        self.inject
    }

    fn apply(&self, ctx: &mut Ctx) -> Result<Effect, PluginError> {
        if let Some(handle) = &self.handle {
            ctx.provide(
                crate::assembly::RESPONSE_CACHE,
                Arc::new(crate::assembly::ResponseCacheSlot::new(Arc::clone(handle))),
            );
        }
        Ok(Effect::noop())
    }
}

/// The factory row for `builtin/response_cache`: reads the one config
/// key, `config.enabled`, default **`false`** (spec §4.17 — the key may
/// be omitted entirely; the default is the capability's off switch, and
/// it is the limb CONF-88's red control sabotages). An enabled entry
/// gets the store its fiber owns; an inert entry mounts with `None`.
pub(crate) fn build_response_cache(
    plug: &PluginCfg,
    inject: &'static [ServiceId],
) -> ResponseCachePlugin {
    let enabled = plug
        .config
        .as_ref()
        .and_then(|c| c.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ResponseCachePlugin {
        id: PluginId::new(plug.id.clone()),
        inject,
        handle: enabled.then(|| Arc::new(ResponseCacheHandle::new()) as Arc<dyn ResponseCache>),
    }
}
