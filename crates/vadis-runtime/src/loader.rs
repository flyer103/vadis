//! The declarative loader (DESIGN §12.2, ADR-036 D5): takes plugin instances,
//! resolves their `inject` declarations **at load time**, activates them in
//! dependency order, and unloads them in §12.2's four-step order:
//!
//! ① recursively move the dependents into `Unloading` and wait for them to
//!   finish (synchronous here: a dependent's whole deactivate sequence runs
//!   before its provider's undos start);
//! ② run this fiber's `Effect::undo` in reverse LIFO;
//! ③ withdraw the service bindings (and the intercept entries) the undos did
//!   not already restore — bindings made through a `RealmGuard` have no
//!   on-stack inverse by design (`Ctx::isolate` documents why), so the sweep
//!   by owner is what withdraws them;
//! ④ `Removed`.
//!
//! There is no per-request lookup anywhere in this crate: resolution happens
//! here, at load/unload time (ADR-036 D8 — a per-request lookup would be a
//! defect even before R41-3's latency ladder could measure it).

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::ctx::{Ctx, Shared};
use crate::effect::{Effect, EffectId};
use crate::fiber::{FiberState, Plugin};
use crate::service::{PluginId, ServiceId, ServiceKey, ROOT_REALM};

/// Why a `load`/`unload` *call* failed — distinct from `PluginError`, which
/// is a fiber's failure and never leaves its fiber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// A fiber with this id is already loaded (in any state, including
    /// `Removed` — ids are not recycled within one loader).
    Duplicate(PluginId),
    /// No fiber with this id was ever loaded.
    Unknown(PluginId),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Duplicate(id) => write!(f, "plugin '{id}' is already loaded"),
            LoadError::Unknown(id) => write!(f, "plugin '{id}' was never loaded"),
        }
    }
}

impl Error for LoadError {}

/// One loaded plugin and its accumulated effect stack.
struct Fiber {
    plugin: Box<dyn Plugin>,
    effects: Vec<(EffectId, Effect)>,
}

/// The P9 runtime: owns the fibers, the shared tables and the state map.
///
/// Consumed by nobody outside this crate in R41-2 — the assembly that will
/// drive it from the `plugins:` list is R41-3 (ADR-036 D8).
pub struct Loader {
    shared: Arc<Shared>,
    fibers: HashMap<PluginId, Fiber>,
    /// Load order, so waiter resolution is deterministic.
    order: Vec<PluginId>,
}

impl Loader {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared::new()),
            fibers: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// The current state of a fiber, or `None` if the id was never loaded.
    pub fn state(&self, id: &PluginId) -> Option<FiberState> {
        self.shared.lock_states().get(id).cloned()
    }

    /// Reads a root-realm binding (the assembly's read path, and the test
    /// oracle for assertion (e): realm bindings must not be visible here).
    pub fn get<T: Send + Sync + 'static>(&self, key: &ServiceKey<T>) -> Option<Arc<T>> {
        crate::ctx::read_binding(&self.shared.lock_tables(), key.name(), ROOT_REALM)
    }

    /// Loads `plugin`. Returns `Ok` even when its `inject` is unsatisfied:
    /// the fiber then stays at `Loading{waiting_on}` with **no error**
    /// (assertion (d)) and activates later, when a provider lands.
    pub fn load(&mut self, plugin: Box<dyn Plugin>) -> Result<(), LoadError> {
        let id = plugin.id().clone();
        if self.fibers.contains_key(&id) {
            return Err(LoadError::Duplicate(id));
        }
        self.shared
            .lock_states()
            .insert(id.clone(), FiberState::Created);
        self.fibers.insert(
            id.clone(),
            Fiber {
                plugin,
                effects: Vec::new(),
            },
        );
        self.order.push(id);
        self.resolve();
        Ok(())
    }

    /// Unloads one fiber in the four-step order documented on this module.
    /// Unloading an already-`Removed` fiber is a no-op; unloading an id that
    /// was never loaded is an error.
    pub fn unload(&mut self, id: &PluginId) -> Result<(), LoadError> {
        if !self.fibers.contains_key(id) {
            return Err(LoadError::Unknown(id.clone()));
        }
        if self.state(id) == Some(FiberState::Removed) {
            return Ok(());
        }
        self.set_state(id, FiberState::Unloading);
        // ① Dependents first, recursively. A dependent is *deactivated*, not
        // removed: it is still declared, so it lands back at
        // `Loading{waiting_on}` and re-activates if a provider returns.
        for dep in self.dependents_of(id) {
            self.deactivate(&dep);
        }
        // ② This fiber's undos, reverse LIFO.
        self.run_undos(id);
        // ③ Withdraw whatever the undos did not restore (realm bindings,
        // intercept entries) — swept by owner.
        self.shared.lock_tables().sweep(id);
        // ④ Terminal.
        self.set_state(id, FiberState::Removed);
        Ok(())
    }

    fn set_state(&self, id: &PluginId, state: FiberState) {
        self.shared.lock_states().insert(id.clone(), state);
    }

    /// The `Active` fibers whose `inject` names a root-realm binding owned
    /// by `id`, in load order. Only `Active` fibers have anything to roll
    /// back, and only root-realm bindings are consumable via `inject`.
    fn dependents_of(&self, id: &PluginId) -> Vec<PluginId> {
        let provided: Vec<&'static str> = self
            .shared
            .lock_tables()
            .services
            .iter()
            .filter(|(_, realms)| {
                realms
                    .get(&ROOT_REALM)
                    .is_some_and(|binding| &binding.owner == id)
            })
            .map(|(name, _)| *name)
            .collect();
        self.order
            .iter()
            .filter(|other| {
                *other != id
                    && matches!(self.state(other), Some(FiberState::Active))
                    && self.fibers.get(*other).is_some_and(|f| {
                        f.plugin
                            .inject()
                            .iter()
                            .any(|s| provided.contains(&s.name()))
                    })
            })
            .cloned()
            .collect()
    }

    /// Rolls an `Active` fiber back to `Loading{waiting_on}` because one of
    /// its coeffects is going away: its own dependents first (recursively),
    /// then its undos in reverse LIFO, then the owner sweep.
    fn deactivate(&mut self, id: &PluginId) {
        if self.state(id) != Some(FiberState::Active) {
            return;
        }
        self.set_state(id, FiberState::Unloading);
        for dep in self.dependents_of(id) {
            self.deactivate(&dep);
        }
        self.run_undos(id);
        self.shared.lock_tables().sweep(id);
        let waiting_on = self.unsatisfied(id);
        self.set_state(id, FiberState::Loading { waiting_on });
    }

    /// Runs and discards a fiber's effect stack in reverse registration
    /// order (LIFO). Each inverse runs at most once (`Effect::run_undo`).
    fn run_undos(&mut self, id: &PluginId) {
        let Some(fiber) = self.fibers.get_mut(id) else {
            return;
        };
        let effects = std::mem::take(&mut fiber.effects);
        for (_, mut effect) in effects.into_iter().rev() {
            effect.run_undo();
        }
    }

    /// The slots `id` declares that are not *usable* right now: a slot is
    /// satisfied only by a root-realm binding whose owner is `Active`. A
    /// provider that has entered `Unloading` no longer satisfies `inject`
    /// (DESIGN §4: "when a provider enters UNLOADING its dependents are
    /// deactivated first") — this is what lets a dependent's `waiting_on`
    /// name the departing service even though the provider's own bindings
    /// are not withdrawn until steps ②/③ of its unload.
    fn unsatisfied(&self, id: &PluginId) -> Vec<ServiceId> {
        let Some(fiber) = self.fibers.get(id) else {
            return Vec::new();
        };
        let tables = self.shared.lock_tables();
        fiber
            .plugin
            .inject()
            .iter()
            .filter(|s| {
                let usable = tables
                    .services
                    .get(s.name())
                    .and_then(|realms| realms.get(&ROOT_REALM))
                    .is_some_and(|binding| self.state(&binding.owner) == Some(FiberState::Active));
                !usable
            })
            .copied()
            .collect()
    }

    /// Load-time resolution: walks the fibers in load order and activates
    /// every one whose coeffects are now satisfied, repeating until a pass
    /// activates nobody (one activation can satisfy another's `inject`).
    /// Fibers that stay unsatisfied remain at `Loading{waiting_on}` — with
    /// no error. `Failed` and `Removed` fibers are never retried.
    fn resolve(&mut self) {
        loop {
            let mut progressed = false;
            for id in self.order.clone() {
                match self.state(&id) {
                    Some(FiberState::Created) | Some(FiberState::Loading { .. }) => {
                        let waiting_on = self.unsatisfied(&id);
                        if waiting_on.is_empty() {
                            self.activate(&id);
                            progressed = true;
                        } else {
                            self.set_state(&id, FiberState::Loading { waiting_on });
                        }
                    }
                    _ => {}
                }
            }
            if !progressed {
                break;
            }
        }
    }

    /// Applies one fiber: on success its staged effects (plus the returned
    /// `Effect`, pushed last) become the fiber's stack and it goes `Active`;
    /// on failure everything it registered is rolled back immediately —
    /// undos in reverse LIFO, then the owner sweep — and it lands at
    /// `Failed(err)` without touching any other fiber (assertion (c)).
    fn activate(&mut self, id: &PluginId) {
        let Some(fiber) = self.fibers.get_mut(id) else {
            return;
        };
        let mut ctx = Ctx::new(id.clone(), Arc::clone(&self.shared));
        match fiber.plugin.apply(&mut ctx) {
            Ok(effect) => {
                ctx.effect(effect);
                fiber.effects = ctx.take_effects();
                self.set_state(id, FiberState::Active);
            }
            Err(err) => {
                for (_, mut staged) in ctx.take_effects().into_iter().rev() {
                    staged.run_undo();
                }
                self.shared.lock_tables().sweep(id);
                self.set_state(id, FiberState::Failed(err));
            }
        }
    }

    /// A deep-comparable snapshot of both tables (assertion (b)'s evidence).
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> crate::ctx::TableSnapshot {
        self.shared.lock_tables().snapshot()
    }

    /// Test oracle for realm reads from outside a fiber (assertion (e)).
    #[cfg(test)]
    pub(crate) fn get_in_realm<T: Send + Sync + 'static>(
        &self,
        key: &ServiceKey<T>,
        realm: crate::service::RealmId,
    ) -> Option<Arc<T>> {
        crate::ctx::read_binding(&self.shared.lock_tables(), key.name(), realm)
    }

    /// Test access to the shared state map, so undo closures can observe
    /// mid-unload states (assertion (a) verifies the *order*, not end state).
    #[cfg(test)]
    pub(crate) fn shared(&self) -> Arc<Shared> {
        Arc::clone(&self.shared)
    }
}

impl Default for Loader {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Loader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Loader")
            .field("fibers", &self.order)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::InterceptMeta;
    use crate::fiber::PluginError;
    use crate::service::RealmId;
    use std::sync::Mutex;
    use std::time::Duration;

    #[derive(Debug, PartialEq)]
    struct TestSvc(u32);

    const SVC: ServiceKey<TestSvc> = ServiceKey::new("test_svc");
    const SVC_ID: ServiceId = ServiceId::new("test_svc");
    const OTHER: ServiceKey<TestSvc> = ServiceKey::new("other_svc");
    const REALM_1: RealmId = RealmId::new(1);
    static NEEDS_SVC: &[ServiceId] = &[SVC_ID];
    static NEEDS_NONE: &[ServiceId] = &[];

    type ApplyFn = Box<dyn Fn(&mut Ctx) -> Result<Effect, PluginError> + Send + Sync>;

    struct TestPlugin {
        id: PluginId,
        inject: &'static [ServiceId],
        on_apply: ApplyFn,
    }

    impl Plugin for TestPlugin {
        fn id(&self) -> &PluginId {
            &self.id
        }
        fn inject(&self) -> &'static [ServiceId] {
            self.inject
        }
        fn apply(&self, ctx: &mut Ctx) -> Result<Effect, PluginError> {
            (self.on_apply)(ctx)
        }
    }

    fn pid(s: &str) -> PluginId {
        PluginId::new(s)
    }

    fn plugin(
        id: &str,
        inject: &'static [ServiceId],
        on_apply: impl Fn(&mut Ctx) -> Result<Effect, PluginError> + Send + Sync + 'static,
    ) -> Box<dyn Plugin> {
        Box::new(TestPlugin {
            id: pid(id),
            inject,
            on_apply: Box::new(on_apply),
        })
    }

    /// A short, comparable word for a fiber's current state, read from inside
    /// undo closures — this is how assertion (a) observes the *order* of the
    /// unload, not just its end state.
    fn state_tag(shared: &Shared, id: &str) -> &'static str {
        match shared.lock_states().get(&pid(id)) {
            Some(FiberState::Created) => "created",
            Some(FiberState::Loading { .. }) => "waiting",
            Some(FiberState::Active) => "active",
            Some(FiberState::Unloading) => "unloading",
            Some(FiberState::Failed(_)) => "failed",
            Some(FiberState::Removed) => "removed",
            None => "absent",
        }
    }

    fn bound_tag(shared: &Shared, name: &'static str, realm: RealmId) -> &'static str {
        if shared.lock_tables().has_binding(name, realm) {
            "t"
        } else {
            "f"
        }
    }

    /// **Assertion (a)** — the unload order, exactly: ① dependents
    /// recursively into `Unloading`, waited to completion → ② this fiber's
    /// `Effect::undo` in reverse LIFO → ③ the service bindings withdrawn →
    /// ④ `Removed`. The event log below is written by the undo closures
    /// themselves, so a wrong order produces a wrong log, not a wrong state.
    ///
    /// RED recipe: in `run_undos`, drop the `.rev()` (undos run in
    /// registration order) — the expected log below stops matching.
    #[test]
    fn unload_order_is_dependents_then_reverse_lifo_then_withdraw_then_removed() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut loader = Loader::new();
        let shared = loader.shared();

        // The provider: binds SVC in ROOT_REALM (inverse on the stack), binds
        // SVC in REALM_1 (no on-stack inverse — only step ③ withdraws it),
        // then registers e1 and e2; its apply returns e3. Undo order must be
        // e3, e2, e1, provide-inverse.
        let p_events = Arc::clone(&events);
        let p_shared = Arc::clone(&shared);
        loader
            .load(plugin("p", NEEDS_NONE, move |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                let realm = ctx.isolate(SVC, REALM_1);
                realm.provide(Arc::new(TestSvc(2))).unwrap();
                for tag in ["e1", "e2"] {
                    let ev = Arc::clone(&p_events);
                    let sh = Arc::clone(&p_shared);
                    ctx.effect(Effect::new(move || {
                        let root = bound_tag(&sh, "test_svc", ROOT_REALM);
                        let realm = bound_tag(&sh, "test_svc", REALM_1);
                        ev.lock()
                            .unwrap()
                            .push(format!("p:undo:{tag}(root={root},realm={realm})"));
                    }));
                }
                let ev = Arc::clone(&p_events);
                let sh = Arc::clone(&p_shared);
                Ok(Effect::new(move || {
                    let root = bound_tag(&sh, "test_svc", ROOT_REALM);
                    let realm = bound_tag(&sh, "test_svc", REALM_1);
                    let d = state_tag(&sh, "d");
                    ev.lock()
                        .unwrap()
                        .push(format!("p:undo:e3(root={root},realm={realm},d={d})"));
                }))
            }))
            .unwrap();

        // The dependent: injects SVC; its undo records both fibers' states
        // as seen *from inside* step ①.
        let d_events = Arc::clone(&events);
        let d_shared = Arc::clone(&shared);
        loader
            .load(plugin("d", NEEDS_SVC, move |ctx| {
                let ev = Arc::clone(&d_events);
                let sh = Arc::clone(&d_shared);
                ctx.effect(Effect::new(move || {
                    let p = state_tag(&sh, "p");
                    let d = state_tag(&sh, "d");
                    ev.lock().unwrap().push(format!("d:undo(p={p},d={d})"));
                }));
                Ok(Effect::noop())
            }))
            .unwrap();
        assert_eq!(loader.state(&pid("p")), Some(FiberState::Active));
        assert_eq!(loader.state(&pid("d")), Some(FiberState::Active));

        loader.unload(&pid("p")).unwrap();

        // The order, observed from inside: the dependent's undo ran first,
        // while both fibers were mid-unload; then p's undos in reverse LIFO,
        // each still seeing both bindings (③ has not run yet); and by the
        // time p's first undo runs, the dependent has *finished* (waiting).
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "d:undo(p=unloading,d=unloading)".to_string(),
                "p:undo:e3(root=t,realm=t,d=waiting)".to_string(),
                "p:undo:e2(root=t,realm=t)".to_string(),
                "p:undo:e1(root=t,realm=t)".to_string(),
            ]
        );
        // ③ + ④: everything p owned is withdrawn, and p is Removed; the
        // dependent is back at Loading, waiting on the service that left.
        assert!(loader.get(&SVC).is_none(), "root binding must be withdrawn");
        assert!(
            loader.get_in_realm(&SVC, REALM_1).is_none(),
            "realm binding must be swept (step ③)"
        );
        assert_eq!(loader.state(&pid("p")), Some(FiberState::Removed));
        assert_eq!(
            loader.state(&pid("d")),
            Some(FiberState::Loading {
                waiting_on: vec![SVC_ID],
            })
        );
    }

    /// **Assertion (b)** — after load → activate → unload, the service table
    /// AND the intercept table are deep-equal to their pre-load state. A
    /// resident fiber with its own binding and intercept entry makes the
    /// empty case impossible: equality must hold *with content present*.
    ///
    /// RED recipe: in `Tables::sweep`, delete the `intercepts.retain` line —
    /// p's intercept entry survives unload and the final `assert_eq!` fails.
    #[test]
    fn tables_are_deep_equal_to_pre_load_state_after_unload() {
        let mut loader = Loader::new();
        loader
            .load(plugin("resident", NEEDS_NONE, |ctx| {
                ctx.provide(OTHER, Arc::new(TestSvc(9)));
                ctx.intercept(
                    &OTHER,
                    InterceptMeta {
                        sample: Some(1.0),
                        timeout: None,
                        shadow: false,
                    },
                );
                Ok(Effect::noop())
            }))
            .unwrap();
        let before = loader.snapshot();

        loader
            .load(plugin("p", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                let realm = ctx.isolate(SVC, REALM_1);
                realm.provide(Arc::new(TestSvc(2))).unwrap();
                ctx.intercept(
                    &SVC,
                    InterceptMeta {
                        sample: Some(0.25),
                        timeout: Some(Duration::from_secs(1)),
                        shadow: true,
                    },
                );
                Ok(Effect::new(|| {}))
            }))
            .unwrap();
        // Non-vacuity: loading p really changed BOTH tables — two new
        // service entries (root + realm) and one new intercept entry.
        let during = loader.snapshot();
        assert_eq!(during.services.len(), before.services.len() + 2);
        assert_eq!(during.intercepts.len(), before.intercepts.len() + 1);
        assert!(during.intercepts.contains_key(&("test_svc", pid("p"))));

        loader.unload(&pid("p")).unwrap();
        assert_eq!(
            loader.snapshot(),
            before,
            "service table and intercept table must be deep-equal to pre-load"
        );
    }

    /// **Assertion (c)** — a `Failed(err)` fiber leaves the other fibers
    /// `Active` (ADR-002's failure isolation), even when the failure happens
    /// *during* apply after the plugin already registered an effect and a
    /// binding: both are rolled back before the failure lands.
    ///
    /// RED recipe: in `activate`'s `Err` arm, delete the rollback loop (the
    /// `for ... staged.run_undo()` lines) — the `bad:undo` assertion below
    /// fails.
    #[test]
    fn a_failed_fiber_leaves_the_other_fibers_active() {
        let undos = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let mut loader = Loader::new();
        loader
            .load(plugin("good1", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                Ok(Effect::noop())
            }))
            .unwrap();

        let bad_undos = Arc::clone(&undos);
        loader
            .load(plugin("bad", NEEDS_NONE, move |ctx| {
                ctx.provide(OTHER, Arc::new(TestSvc(2)));
                let u = Arc::clone(&bad_undos);
                ctx.effect(Effect::new(move || u.lock().unwrap().push("bad:undo")));
                Err(PluginError::new("boom"))
            }))
            .unwrap(); // load itself succeeds; the failure is the fiber's

        loader
            .load(plugin("good2", NEEDS_SVC, |_ctx| Ok(Effect::noop())))
            .unwrap();

        assert_eq!(loader.state(&pid("good1")), Some(FiberState::Active));
        assert_eq!(loader.state(&pid("good2")), Some(FiberState::Active));
        assert_eq!(
            loader.state(&pid("bad")),
            Some(FiberState::Failed(PluginError::new("boom")))
        );
        // The failed apply's partial registrations were rolled back...
        assert_eq!(*undos.lock().unwrap(), vec!["bad:undo"]);
        assert!(
            loader.get(&OTHER).is_none(),
            "the failed fiber's binding must be swept"
        );
        // ...and nobody else's state moved.
        assert!(loader.get(&SVC).is_some());
    }

    /// **Assertion (d)** — an unsatisfied `inject` stays
    /// `Loading{waiting_on}` with **no error**: `load` returns `Ok`, `apply`
    /// never runs, and when the provider lands later the waiter activates at
    /// load time (resolution is a load-time fixpoint, not a runtime lookup).
    ///
    /// RED recipe: in `load`, replace the unconditional `self.resolve()` tail
    /// with an early `Err` when `unsatisfied` is non-empty — the
    /// `assert_eq!(result, Ok(()))` below fails.
    #[test]
    fn an_unsatisfied_inject_waits_at_loading_with_no_error() {
        let applied = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let mut loader = Loader::new();
        let a = Arc::clone(&applied);
        let result = loader.load(plugin("waiter", NEEDS_SVC, move |_ctx| {
            a.lock().unwrap().push("waiter:applied");
            Ok(Effect::noop())
        }));
        assert_eq!(result, Ok(()));
        assert_eq!(
            loader.state(&pid("waiter")),
            Some(FiberState::Loading {
                waiting_on: vec![SVC_ID],
            })
        );
        assert!(
            applied.lock().unwrap().is_empty(),
            "apply must not run while inject is unsatisfied"
        );

        // Out-of-order loading is safe: the provider arrives second and the
        // waiter activates in the same load call's fixpoint pass.
        let a2 = Arc::clone(&applied);
        loader
            .load(plugin("provider", NEEDS_NONE, move |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                a2.lock().unwrap().push("provider:applied");
                Ok(Effect::noop())
            }))
            .unwrap();
        assert_eq!(loader.state(&pid("waiter")), Some(FiberState::Active));
        assert_eq!(loader.state(&pid("provider")), Some(FiberState::Active));
        assert_eq!(
            *applied.lock().unwrap(),
            vec!["provider:applied", "waiter:applied"]
        );
    }

    /// **Assertion (e)** — a realm's bindings do not appear in `ROOT_REALM`,
    /// checked in both directions: the realm binding is invisible from root,
    /// and a later root binding for the same key neither overwrites nor
    /// shadows the realm's set.
    ///
    /// RED recipe: in `Ctx::isolate`, hard-code `realm: ROOT_REALM` into the
    /// constructed `RealmGuard` — the first `assert!(loader.get(&SVC).is_none())`
    /// below fails.
    #[test]
    fn realm_bindings_do_not_leak_into_the_root_realm() {
        let mut loader = Loader::new();
        loader
            .load(plugin("p", NEEDS_NONE, move |ctx| {
                let realm = ctx.isolate(SVC, REALM_1);
                realm.provide(Arc::new(TestSvc(7))).unwrap();
                Ok(Effect::noop())
            }))
            .unwrap();
        // realm → root: the root realm has no binding for the key.
        assert!(loader.get(&SVC).is_none());
        assert_eq!(
            loader.get_in_realm(&SVC, REALM_1).as_deref(),
            Some(&TestSvc(7))
        );

        // root → realm: a root binding for the same key lands beside the
        // realm's set, not on top of it.
        loader
            .load(plugin("q", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                Ok(Effect::noop())
            }))
            .unwrap();
        assert_eq!(loader.get(&SVC).as_deref(), Some(&TestSvc(1)));
        assert_eq!(
            loader.get_in_realm(&SVC, REALM_1).as_deref(),
            Some(&TestSvc(7)),
            "the root binding must not overwrite the realm's set"
        );

        // And a fresh guard on the same (key, realm) — made by a third fiber
        // — still reads the realm's own binding, not root's.
        let seen = Arc::new(Mutex::new(None));
        let seen2 = Arc::clone(&seen);
        loader
            .load(plugin("r", NEEDS_NONE, move |ctx| {
                let guard = ctx.isolate(SVC, REALM_1);
                *seen2.lock().unwrap() = guard.get::<TestSvc>().map(|v| v.0);
                Ok(Effect::noop())
            }))
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), Some(7));
    }

    #[test]
    fn duplicate_ids_are_refused_and_unknown_unloads_error() {
        let mut loader = Loader::new();
        loader
            .load(plugin("p", NEEDS_NONE, |_c| Ok(Effect::noop())))
            .unwrap();
        assert_eq!(
            loader.load(plugin("p", NEEDS_NONE, |_c| Ok(Effect::noop()))),
            Err(LoadError::Duplicate(pid("p")))
        );
        assert_eq!(
            loader.unload(&pid("ghost")),
            Err(LoadError::Unknown(pid("ghost")))
        );
        // Unloading twice: the second is a no-op on the Removed fiber.
        loader.unload(&pid("p")).unwrap();
        assert_eq!(loader.state(&pid("p")), Some(FiberState::Removed));
        loader.unload(&pid("p")).unwrap();
        assert_eq!(loader.state(&pid("p")), Some(FiberState::Removed));
    }

    /// Undoing an overwrite restores the displaced binding, so deep-equality
    /// survives a provide-over-provide chain (both fibers cleanly unrolled).
    #[test]
    fn an_undone_overwrite_restores_the_displaced_binding() {
        let mut loader = Loader::new();
        loader
            .load(plugin("first", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                Ok(Effect::noop())
            }))
            .unwrap();
        loader
            .load(plugin("second", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(2)));
                Ok(Effect::noop())
            }))
            .unwrap();
        assert_eq!(loader.get(&SVC).as_deref(), Some(&TestSvc(2)));
        loader.unload(&pid("second")).unwrap();
        assert_eq!(
            loader.get(&SVC).as_deref(),
            Some(&TestSvc(1)),
            "the displaced binding must be restored by the overwrite's inverse"
        );
        loader.unload(&pid("first")).unwrap();
        assert!(loader.get(&SVC).is_none());
    }

    /// Dependents are deactivated transitively: unloading A deactivates B
    /// (which injects A's service) and therefore C (which injects B's).
    #[test]
    fn dependents_are_deactivated_recursively() {
        const SA: ServiceKey<TestSvc> = ServiceKey::new("svc_a");
        const SA_ID: ServiceId = ServiceId::new("svc_a");
        const SB: ServiceKey<TestSvc> = ServiceKey::new("svc_b");
        const SB_ID: ServiceId = ServiceId::new("svc_b");
        static NEEDS_SA: &[ServiceId] = &[SA_ID];
        static NEEDS_SB: &[ServiceId] = &[SB_ID];

        let mut loader = Loader::new();
        loader
            .load(plugin("a", NEEDS_NONE, |ctx| {
                ctx.provide(SA, Arc::new(TestSvc(1)));
                Ok(Effect::noop())
            }))
            .unwrap();
        loader
            .load(plugin("b", NEEDS_SA, |ctx| {
                ctx.provide(SB, Arc::new(TestSvc(2)));
                Ok(Effect::noop())
            }))
            .unwrap();
        loader
            .load(plugin("c", NEEDS_SB, |_c| Ok(Effect::noop())))
            .unwrap();
        assert_eq!(loader.state(&pid("c")), Some(FiberState::Active));

        loader.unload(&pid("a")).unwrap();
        assert_eq!(loader.state(&pid("a")), Some(FiberState::Removed));
        assert_eq!(
            loader.state(&pid("b")),
            Some(FiberState::Loading {
                waiting_on: vec![SA_ID],
            })
        );
        assert_eq!(
            loader.state(&pid("c")),
            Some(FiberState::Loading {
                waiting_on: vec![SB_ID],
            })
        );
        assert!(loader.get(&SA).is_none());
        assert!(loader.get(&SB).is_none());
    }

    /// A dependent whose provider is unloaded waits again — and re-activates
    /// when a provider returns (resolution is a fixpoint over load events,
    /// not a one-shot at first load).
    #[test]
    fn a_reloaded_provider_reactivates_its_dependents() {
        let mut loader = Loader::new();
        loader
            .load(plugin("p", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(1)));
                Ok(Effect::noop())
            }))
            .unwrap();
        loader
            .load(plugin("d", NEEDS_SVC, |_c| Ok(Effect::noop())))
            .unwrap();
        assert_eq!(loader.state(&pid("d")), Some(FiberState::Active));

        loader.unload(&pid("p")).unwrap();
        assert_eq!(
            loader.state(&pid("d")),
            Some(FiberState::Loading {
                waiting_on: vec![SVC_ID],
            })
        );

        loader
            .load(plugin("p2", NEEDS_NONE, |ctx| {
                ctx.provide(SVC, Arc::new(TestSvc(3)));
                Ok(Effect::noop())
            }))
            .unwrap();
        assert_eq!(loader.state(&pid("d")), Some(FiberState::Active));
        assert_eq!(loader.get(&SVC).as_deref(), Some(&TestSvc(3)));
    }
}
