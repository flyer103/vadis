//! `Ctx` — one fiber's scope: the service table, the effect stack and the
//! realm table behind it (DESIGN §12.2). A `Ctx` exists only for the duration
//! of one `Plugin::apply` call; everything registered through it is
//! attributed to its fiber and rolled back when that fiber unloads.
//!
//! Interior choices the sketch leaves open (`/* fiber scope: … */`):
//!
//! - The tables live in one `Shared` per loader, behind a `Mutex`. All access
//!   is load-time (the loader resolves `inject` declarations when plugins are
//!   loaded, ADR-036 D8); there is no request path in this crate, so the lock
//!   is never held across a user closure and never contended per request.
//! - Fiber *states* live in `Shared` too — not in the loader — so an
//!   `Effect::undo` closure can observe mid-unload states. That is what makes
//!   the unload **order** assertable, not just the end state.
//! - The service table is keyed by `(slot name, realm)`: one key, several
//!   binding sets (ADR-013's A/B and shadow). `Ctx::provide`/`Ctx::get`
//!   operate on `ROOT_REALM`; other realms are reachable only through the
//!   `RealmGuard` that `Ctx::isolate` hands out.

use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::effect::{Effect, EffectId};
use crate::fiber::{FiberState, PluginError};
use crate::service::{PluginId, RealmId, ServiceKey, ROOT_REALM};

/// Metadata about *how* a binding is used — sample rate, timeout, shadow —
/// never a rebinding (ADR-036 D5: "a test that finds the intercept table
/// changed" the *binding* "is a defect"). The fields mirror the `intercept:`
/// config vocabulary (`docs/spec.md` §4.3) plus the timeout the design table
/// names; config → meta conversion is the assembly's job (R41-3), not this
/// crate's.
#[derive(Debug, Clone, PartialEq)]
pub struct InterceptMeta {
    /// Fraction of uses to sample, in `[0, 1]`; `None` = no sampling. The
    /// range is the caller's contract (the config layer validates it today).
    pub sample: Option<f64>,
    /// Per-use timeout override; `None` = no override.
    pub timeout: Option<Duration>,
    /// Invoke the binding without serving its result (shadow rail, ADR-013).
    pub shadow: bool,
}

impl InterceptMeta {
    /// No sampling, no timeout override, no shadow.
    pub const fn none() -> Self {
        Self {
            sample: None,
            timeout: None,
            shadow: false,
        }
    }
}

/// One binding in the service table: the value, its concrete type (checked at
/// `get` time) and the fiber that owns it. The owner is what unload sweeps by.
pub(crate) struct Binding {
    value: Arc<dyn Any + Send + Sync>,
    type_id: TypeId,
    pub(crate) owner: PluginId,
}

/// The tables behind every `Ctx` of one loader.
#[derive(Default)]
pub(crate) struct Tables {
    /// slot name → realm → binding.
    pub(crate) services: HashMap<&'static str, HashMap<RealmId, Binding>>,
    /// (slot name, fiber) → how that fiber's uses of the slot are mediated.
    pub(crate) intercepts: HashMap<(&'static str, PluginId), InterceptMeta>,
}

/// A comparable snapshot of both tables — the evidence for §12.2's
/// deep-equality condition after load → activate → unload. Equality is over
/// (slot, realm, owner, concrete type, value *identity*): the values are
/// opaque `Arc`s, so identity is compared, not content.
#[cfg(test)]
#[derive(Debug, PartialEq)]
pub(crate) struct TableSnapshot {
    pub(crate) services: BTreeMap<(&'static str, RealmId), (PluginId, TypeId, usize)>,
    pub(crate) intercepts: BTreeMap<(&'static str, PluginId), InterceptMeta>,
}

impl Tables {
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> TableSnapshot {
        let services = self
            .services
            .iter()
            .flat_map(|(name, realms)| {
                realms.iter().map(move |(realm, b)| {
                    (
                        (*name, *realm),
                        (
                            b.owner.clone(),
                            b.type_id,
                            Arc::as_ptr(&b.value) as *const () as usize,
                        ),
                    )
                })
            })
            .collect();
        let intercepts = self
            .intercepts
            .iter()
            .map(|((name, owner), md)| ((*name, owner.clone()), md.clone()))
            .collect();
        TableSnapshot {
            services,
            intercepts,
        }
    }

    /// Whether any binding exists at (slot, realm) — the observation undo
    /// closures make to prove the unload *order* (assertion (a)).
    #[cfg(test)]
    pub(crate) fn has_binding(&self, name: &'static str, realm: RealmId) -> bool {
        self.services
            .get(name)
            .is_some_and(|realms| realms.contains_key(&realm))
    }

    /// Removes everything owned by `id` — service bindings in every realm and
    /// every intercept entry. This is unload step ③ (and the tail of a failed
    /// `apply`'s rollback): whatever the effect-stack undos did not withdraw,
    /// the sweep does.
    pub(crate) fn sweep(&mut self, id: &PluginId) {
        self.services.retain(|_, realms| {
            realms.retain(|_, b| &b.owner != id);
            !realms.is_empty()
        });
        self.intercepts.retain(|(_, owner), _| owner != id);
    }
}

/// The state every part of one loader's runtime shares: the tables plus one
/// `FiberState` per loaded plugin.
pub(crate) struct Shared {
    tables: Mutex<Tables>,
    states: Mutex<BTreeMap<PluginId, FiberState>>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Self {
            tables: Mutex::new(Tables::default()),
            states: Mutex::new(BTreeMap::new()),
        }
    }

    pub(crate) fn lock_tables(&self) -> MutexGuard<'_, Tables> {
        // A poisoned lock means a plugin closure panicked while holding it;
        // the loader is load-time machinery, so propagating the panic's
        // witness (rather than silently recovering) is the honest move.
        self.tables.lock().expect("plugin-runtime tables poisoned")
    }

    pub(crate) fn lock_states(&self) -> MutexGuard<'_, BTreeMap<PluginId, FiberState>> {
        self.states.lock().expect("plugin-runtime states poisoned")
    }
}

/// Reads and downcast-checks one binding out of a realm map. A binding made
/// under a different `T` reads as absent, never as a wrong-typed value.
pub(crate) fn read_binding<T: Send + Sync + 'static>(
    tables: &Tables,
    name: &'static str,
    realm: RealmId,
) -> Option<Arc<T>> {
    let binding = tables.services.get(name)?.get(&realm)?;
    if binding.type_id != TypeId::of::<T>() {
        return None;
    }
    Arc::clone(&binding.value).downcast::<T>().ok()
}

/// One fiber's scope (DESIGN §12.2). Hands out `EffectId`s in registration
/// order and stages the fiber's effect stack; the loader drains the stack
/// when `apply` returns.
pub struct Ctx {
    fiber: PluginId,
    shared: Arc<Shared>,
    effects: Vec<(EffectId, Effect)>,
    next_effect: u64,
}

impl Ctx {
    pub(crate) fn new(fiber: PluginId, shared: Arc<Shared>) -> Self {
        Self {
            fiber,
            shared,
            effects: Vec::new(),
            next_effect: 0,
        }
    }

    /// The fiber this scope belongs to.
    pub fn fiber(&self) -> &PluginId {
        &self.fiber
    }

    /// Registers an inverse on this fiber's effect stack (`ctx.effect(cb) →
    /// dispose`). Undos run in reverse registration order (LIFO) at unload.
    pub fn effect(&mut self, e: Effect) -> EffectId {
        let id = EffectId(self.next_effect);
        self.next_effect += 1;
        self.effects.push((id, e));
        id
    }

    /// Binds `v` to `key` in the root realm and registers the inverse on the
    /// effect stack: undo restores *exactly* what the slot held before
    /// (usually nothing, but a displaced earlier binding is put back), so
    /// repeated load → activate → unload leaves the table deep-equal to its
    /// pre-load state even under overwrite (§12.2's closing paragraph).
    pub fn provide<T: Send + Sync + 'static>(&mut self, key: ServiceKey<T>, v: Arc<T>) -> EffectId {
        let displaced = self
            .shared
            .lock_tables()
            .services
            .entry(key.name())
            .or_default()
            .insert(
                ROOT_REALM,
                Binding {
                    value: v,
                    type_id: TypeId::of::<T>(),
                    owner: self.fiber.clone(),
                },
            );
        let shared = Arc::clone(&self.shared);
        let name = key.name();
        self.effect(Effect::new(move || {
            let mut tables = shared.lock_tables();
            let realms = tables.services.entry(name).or_default();
            match displaced {
                Some(prev) => {
                    realms.insert(ROOT_REALM, prev);
                }
                None => {
                    realms.remove(&ROOT_REALM);
                }
            }
            if realms.is_empty() {
                tables.services.remove(name);
            }
        }))
    }

    /// Reads the root-realm binding for `key`, downcast-checked at the
    /// concrete type the binding was made with.
    pub fn get<T: Send + Sync + 'static>(&self, key: &ServiceKey<T>) -> Option<Arc<T>> {
        read_binding(&self.shared.lock_tables(), key.name(), ROOT_REALM)
    }

    /// Opens a second binding set for `key` in `realm` (DESIGN §12.2:
    /// multiple sets of bindings for the same key — A/B and shadow coexist).
    ///
    /// The returned guard is a *capability handle*, not RAII: a plugin cannot
    /// hold it past `apply`, so a guard that withdrew on drop would make
    /// realm bindings unreachable by construction. The inverse of everything
    /// bound through it is fiber unload (step ③ sweeps by owner).
    pub fn isolate<T: Send + Sync + 'static>(
        &mut self,
        key: ServiceKey<T>,
        realm: RealmId,
    ) -> RealmGuard {
        RealmGuard {
            fiber: self.fiber.clone(),
            name: key.name(),
            realm,
            type_id: TypeId::of::<T>(),
            shared: Arc::clone(&self.shared),
        }
    }

    /// Attaches `md` to *this fiber's* use of `key` — sample rate, timeout,
    /// shadow switch. The binding itself is never touched (ADR-036 D5:
    /// intercept rebinds nothing); the entry is keyed by (slot, fiber) and
    /// swept when the fiber unloads.
    pub fn intercept<T: Send + Sync + 'static>(&mut self, key: &ServiceKey<T>, md: InterceptMeta) {
        self.shared
            .lock_tables()
            .intercepts
            .insert((key.name(), self.fiber.clone()), md);
    }

    /// Drains the staged effect stack; the loader takes ownership of it when
    /// `apply` returns (on success *and* on failure, for rollback).
    pub(crate) fn take_effects(&mut self) -> Vec<(EffectId, Effect)> {
        std::mem::take(&mut self.effects)
    }
}

impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("fiber", &self.fiber)
            .field("pending_effects", &self.effects.len())
            .finish()
    }
}

/// The handle `Ctx::isolate` returns: it can bind and read values in its one
/// (slot, realm) pair, and nothing else. See `Ctx::isolate` for why it is
/// not RAII.
pub struct RealmGuard {
    fiber: PluginId,
    name: &'static str,
    realm: RealmId,
    type_id: TypeId,
    shared: Arc<Shared>,
}

impl RealmGuard {
    /// The realm this guard binds into.
    pub fn realm(&self) -> RealmId {
        self.realm
    }

    /// Binds `v` in this guard's realm. Fails — without touching the table —
    /// if `T` is not the type the realm was isolated with, or if the slot is
    /// currently owned by a *different* fiber (a realm binding set is one
    /// fiber's scope; silent cross-fiber overwrite would corrupt the other
    /// fiber's unload, which is exactly what the sweep-by-owner forbids).
    pub fn provide<T: Send + Sync + 'static>(&self, v: Arc<T>) -> Result<(), PluginError> {
        if TypeId::of::<T>() != self.type_id {
            return Err(PluginError::new(format!(
                "realm provide on '{}': type differs from the type the realm was isolated with",
                self.name
            )));
        }
        let mut tables = self.shared.lock_tables();
        let realms = tables.services.entry(self.name).or_default();
        if let Some(existing) = realms.get(&self.realm) {
            if existing.owner != self.fiber {
                return Err(PluginError::new(format!(
                    "realm provide on '{}' in {:?}: slot is owned by '{}'",
                    self.name, self.realm, existing.owner
                )));
            }
        }
        realms.insert(
            self.realm,
            Binding {
                value: v,
                type_id: self.type_id,
                owner: self.fiber.clone(),
            },
        );
        Ok(())
    }

    /// Reads this guard's own realm binding (downcast-checked, like
    /// `Ctx::get`). Root bindings are not visible through a realm guard and
    /// realm bindings are not visible through `Ctx::get` — the realms are
    /// separate in both directions.
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        if TypeId::of::<T>() != self.type_id {
            return None;
        }
        read_binding(&self.shared.lock_tables(), self.name, self.realm)
    }
}

impl fmt::Debug for RealmGuard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RealmGuard")
            .field("fiber", &self.fiber)
            .field("name", &self.name)
            .field("realm", &self.realm)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **One** shared table set for the whole test — two `Ctx`s with
    /// separate `Shared`s would be two runtimes, not two fibers of one.
    fn two_ctxs() -> (Ctx, Ctx) {
        let shared = Arc::new(Shared::new());
        (
            Ctx::new(PluginId::new("p"), Arc::clone(&shared)),
            Ctx::new(PluginId::new("q"), shared),
        )
    }

    #[derive(Debug, PartialEq)]
    struct TestSvc(u32);

    const SVC: ServiceKey<TestSvc> = ServiceKey::new("test_svc");

    #[test]
    fn provide_then_get_roundtrips_the_typed_value() {
        let (mut ctx, _) = two_ctxs();
        ctx.provide(SVC, Arc::new(TestSvc(42)));
        let got = ctx.get(&SVC).expect("binding must be readable");
        assert_eq!(*got, TestSvc(42));
        assert_eq!(ctx.effects.len(), 1, "provide registers its inverse");
    }

    #[test]
    fn get_with_a_different_type_reads_as_absent() {
        let (mut ctx, _) = two_ctxs();
        ctx.provide(SVC, Arc::new(TestSvc(42)));
        let wrong: ServiceKey<String> = ServiceKey::new("test_svc");
        assert!(ctx.get(&wrong).is_none());
        assert!(ctx.get(&SVC).is_some());
    }

    #[test]
    fn intercept_records_metadata_without_rebinding() {
        let (mut ctx, _) = two_ctxs();
        let value = Arc::new(TestSvc(1));
        ctx.provide(SVC, Arc::clone(&value));
        let md = InterceptMeta {
            sample: Some(0.5),
            timeout: Some(Duration::from_millis(250)),
            shadow: true,
        };
        ctx.intercept(&SVC, md.clone());
        // The binding is untouched: same Arc identity, same value.
        let got = ctx.get(&SVC).unwrap();
        assert!(Arc::ptr_eq(&got, &value));
        // And the metadata is recorded under (slot, fiber).
        let tables = ctx.shared.lock_tables();
        assert_eq!(
            tables.intercepts.get(&("test_svc", PluginId::new("p"))),
            Some(&md)
        );
    }

    #[test]
    fn realm_guard_provide_rejects_a_type_mismatch_and_a_foreign_owner() {
        let (mut ctx, mut other) = two_ctxs();
        let guard = ctx.isolate(SVC, RealmId::new(1));
        // Wrong T: rejected, table untouched.
        assert!(guard
            .provide(Arc::new("not a TestSvc".to_string()))
            .is_err());
        assert!(guard.get::<TestSvc>().is_none());
        // Right T: lands.
        guard.provide(Arc::new(TestSvc(7))).unwrap();
        assert_eq!(guard.get::<TestSvc>().as_deref(), Some(&TestSvc(7)));

        // A second fiber isolating the same (slot, realm) may read but not
        // overwrite the first fiber's binding.
        let other_guard = other.isolate(SVC, RealmId::new(1));
        assert_eq!(other_guard.get::<TestSvc>().as_deref(), Some(&TestSvc(7)));
        let err = other_guard.provide(Arc::new(TestSvc(8))).unwrap_err();
        assert!(err.to_string().contains("owned by 'p'"));
        // The original binding survived the refused overwrite.
        assert_eq!(guard.get::<TestSvc>().as_deref(), Some(&TestSvc(7)));
    }
}
