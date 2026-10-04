//! `Effect` — every registration carries its own inverse — and `EffectId`
//! (DESIGN §12.2's frozen sketch). Inverses accumulate on a fiber's effect
//! stack and run in reverse registration order (LIFO) when the fiber unloads.

use std::fmt;

/// Identifies one registered inverse on a fiber's effect stack. Numbered
/// per fiber, in registration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EffectId(pub u64);

/// A registration together with its own inverse (`ctx.effect(cb) → dispose`
/// in Cordis). The inverse runs **at most once**: it is taken out of the
/// effect when it runs, so a double-undo is a no-op by construction.
pub struct Effect {
    undo: Option<Box<dyn FnOnce() + Send>>,
}

impl Effect {
    /// An effect whose inverse is `f`.
    pub fn new(f: impl FnOnce() + Send + 'static) -> Self {
        Self {
            undo: Some(Box::new(f)),
        }
    }

    /// An effect with nothing to roll back.
    pub fn noop() -> Self {
        Self { undo: None }
    }

    /// Runs the inverse, consuming it. Called by the loader during unload
    /// (reverse LIFO) and during rollback of a failed `apply` — never by
    /// plugin code.
    pub(crate) fn run_undo(&mut self) {
        if let Some(undo) = self.undo.take() {
            undo();
        }
    }
}

impl fmt::Debug for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Effect")
            .field("armed", &self.undo.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn effect_new_runs_its_inverse_exactly_once() {
        let runs = Arc::new(Mutex::new(0_u32));
        let runs2 = Arc::clone(&runs);
        let mut eff = Effect::new(move || *runs2.lock().unwrap() += 1);
        eff.run_undo();
        eff.run_undo(); // the inverse was consumed: this is a no-op
        assert_eq!(*runs.lock().unwrap(), 1);
    }

    #[test]
    fn effect_noop_is_inert() {
        let mut eff = Effect::noop();
        eff.run_undo(); // must not panic and has nothing to do
        assert_eq!(format!("{eff:?}"), "Effect { armed: false }");
    }
}
