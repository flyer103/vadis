//! Cordis-style semantic runtime (DESIGN §4/§12.2, the contract frozen by
//! ADR-036): `Ctx` / `Effect` / `ServiceKey` / `FiberState`, the `Plugin`
//! surface, and the declarative `Loader` with realms and intercept.
//!
//! **Status: implemented in R41-2, consumed by nobody.** The assembly that
//! will drive a `Loader` from the `plugins:` config list is R41-3 (ADR-036
//! D8); until then this crate is wired into nothing and changes no
//! user-visible behaviour. The four product service keys
//! (`CACHE_LEDGER` / `SESSION_TABLE` / `QUOTA_STORE` / `TRACE_SINK`) are
//! deliberately absent: the traits they would be declared over exist nowhere
//! in `crates/`, so they land in the round that first binds a real
//! implementation (ADR-036 D3's order).

#![forbid(unsafe_code)]

mod ctx;
mod effect;
mod fiber;
mod loader;
mod service;

pub use ctx::{Ctx, InterceptMeta, RealmGuard};
pub use effect::{Effect, EffectId};
pub use fiber::{FiberState, Plugin, PluginError};
pub use loader::{LoadError, Loader};
pub use service::{PluginId, RealmId, ServiceId, ServiceKey, ROOT_REALM};
