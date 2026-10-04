//! The fiber state machine and the `Plugin` surface (DESIGN §12.2's frozen
//! sketch, ADR-036 D5). `Plugin` is implemented by nobody in this round —
//! the first implementers are R41-3's assembly migrations and R41-4's
//! observer.

use std::error::Error;
use std::fmt;

use crate::ctx::Ctx;
use crate::effect::Effect;
use crate::service::{PluginId, ServiceId};

/// The error a plugin's `apply` returns. A fiber carrying one is *isolated*:
/// it sits at `Failed(err)` and other fibers are unaffected (ADR-002's
/// failure isolation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError {
    message: String,
}

impl PluginError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for PluginError {}

/// Where one fiber is in its lifecycle (DESIGN §12.2):
/// `Created → Loading{waiting_on} → Active → Unloading → Removed`, with
/// `Failed(err)` off to the side. While `inject` is unsatisfied the fiber
/// stays at `Loading{waiting_on}` — it does not error, and other plugins are
/// unaffected. A fiber deactivated because its provider went away lands back
/// at `Loading{waiting_on}` (it is still declared; its coeffect is gone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiberState {
    Created,
    Loading { waiting_on: Vec<ServiceId> },
    Active,
    Unloading,
    Failed(PluginError),
    Removed,
}

/// The tier-A plugin surface: exactly `id` / `inject` / `apply` (DESIGN
/// §12.2's frozen sketch — no more, no less).
pub trait Plugin: Send + Sync {
    fn id(&self) -> &PluginId;

    /// The coeffect declaration (spec §4.3): the product-defined service
    /// slots this plugin depends on. While any of them is unbound the fiber
    /// stays at `Loading{waiting_on}`; it does not error.
    fn inject(&self) -> &'static [ServiceId];

    /// Registers this plugin's effects, bindings, realms and intercepts on
    /// its fiber's scope. The returned `Effect` is pushed onto the fiber's
    /// stack last, so it is the first inverse to run at unload.
    fn apply(&self, ctx: &mut Ctx) -> Result<Effect, PluginError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_error_displays_its_message() {
        let err = PluginError::new("boom");
        assert_eq!(err.to_string(), "boom");
        assert_eq!(err.message(), "boom");
    }

    #[test]
    fn fiber_states_compare_structurally() {
        assert_eq!(FiberState::Created, FiberState::Created);
        assert_eq!(
            FiberState::Loading {
                waiting_on: vec![ServiceId::new("a")],
            },
            FiberState::Loading {
                waiting_on: vec![ServiceId::new("a")],
            }
        );
        assert_ne!(
            FiberState::Failed(PluginError::new("x")),
            FiberState::Failed(PluginError::new("y"))
        );
        assert_ne!(FiberState::Active, FiberState::Removed);
    }
}
