//! Identity and typed-slot vocabulary: `PluginId`, `ServiceId`, `RealmId`,
//! `ROOT_REALM` and `ServiceKey<T>` (DESIGN §12.2's frozen sketch, ADR-036 D5).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// A plugin's identity inside the loader.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId(pub String);

impl PluginId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PluginId({:?})", self.0)
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of a product-defined typed service slot (spec §4.3: slot names
/// are product-defined, never arbitrary strings). The four product slots
/// (`cache_ledger` / `session_table` / `quota_store` / `trace_sink`) are
/// deliberately *not* declared here this round: the traits they would be
/// keyed over exist nowhere in `crates/`, so they land with the round that
/// first binds a real implementation (ADR-036 D3's order).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceId(&'static str);

impl ServiceId {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub fn name(&self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServiceId({:?})", self.0)
    }
}

impl fmt::Display for ServiceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// One binding set of a service slot. `ROOT_REALM` is where `Ctx::provide` /
/// `Ctx::get` operate; `Ctx::isolate` opens additional realms so two binding
/// sets for the same key coexist (A/B and shadow, ADR-013).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RealmId(u32);

/// The realm every non-isolated binding lives in.
pub const ROOT_REALM: RealmId = RealmId(0);

impl RealmId {
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for RealmId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == ROOT_REALM {
            write!(f, "ROOT_REALM")
        } else {
            write!(f, "RealmId({})", self.0)
        }
    }
}

/// A typed handle for one service slot (DESIGN §12.2). The `fn() -> T`
/// marker keeps the key covariant in `T` and unconditionally `Send + Sync`,
/// so `ServiceKey<dyn Trait>` keys work without `T: Sized` — the same
/// type-level trick ADR-018 uses for money.
///
/// Keys compare by **name**: two keys with the same name address the same
/// slot, and a wrong-`T` read of a bound slot is checked at `get` time
/// (it reads as absent, never as a wrong-typed value).
pub struct ServiceKey<T: ?Sized> {
    name: &'static str,
    _m: PhantomData<fn() -> T>,
}

impl<T: ?Sized> ServiceKey<T> {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _m: PhantomData,
        }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }
}

// Manual `Clone`/`Copy`: deriving them would add a spurious `T: Clone` bound.
impl<T: ?Sized> Clone for ServiceKey<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for ServiceKey<T> {}

impl<T: ?Sized> fmt::Debug for ServiceKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ServiceKey({:?})", self.name)
    }
}

impl<T: ?Sized> PartialEq for ServiceKey<T> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<T: ?Sized> Eq for ServiceKey<T> {}

impl<T: ?Sized> Hash for ServiceKey<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    trait AnySvc: Send + Sync {}

    #[test]
    fn service_key_reports_its_name_and_compares_by_it() {
        const KEY: ServiceKey<String> = ServiceKey::new("some_slot");
        assert_eq!(KEY.name(), "some_slot");
        assert_eq!(KEY, ServiceKey::<String>::new("some_slot"));
        assert_ne!(KEY, ServiceKey::<String>::new("other_slot"));
    }

    #[test]
    fn service_key_is_copy_and_send_sync_even_over_trait_objects() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_copy<T: Copy>() {}
        assert_send_sync::<ServiceKey<String>>();
        assert_send_sync::<ServiceKey<dyn AnySvc>>();
        assert_copy::<ServiceKey<dyn AnySvc>>();
        let key = ServiceKey::<dyn AnySvc>::new("slot");
        let copied = key; // a move would fail to compile below without Copy
        assert_eq!(key.name(), copied.name());
    }

    #[test]
    fn root_realm_is_zero_and_realm_ids_roundtrip() {
        assert_eq!(ROOT_REALM.raw(), 0);
        assert_eq!(RealmId::new(7).raw(), 7);
        assert_ne!(RealmId::new(7), ROOT_REALM);
        assert_eq!(format!("{ROOT_REALM:?}"), "ROOT_REALM");
    }

    #[test]
    fn service_id_is_a_const_constructible_name() {
        const ID: ServiceId = ServiceId::new("cache_ledger");
        assert_eq!(ID.name(), "cache_ledger");
        assert_eq!(ID, ServiceId::new("cache_ledger"));
    }
}
