//! Rust-owned rejection diagnostics, independent of engine handles.
//!
//! Keep the original copied diagnostic until its handled transition. Delivery
//! buffers can be drained independently; a handled notification must not invoke
//! JavaScript again to reconstruct an exception that may have changed.

use std::collections::HashMap;

pub(crate) struct RejectionLedger<T> {
    reported: HashMap<u64, T>,
}

impl<T> RejectionLedger<T> {
    pub(crate) fn new() -> Self {
        Self {
            reported: HashMap::new(),
        }
    }

    pub(crate) fn unhandled(&mut self, id: u64, diagnostic: T) {
        self.reported.insert(id, diagnostic);
    }

    pub(crate) fn handled(&mut self, id: u64) -> Option<T> {
        self.reported.remove(&id)
    }

    pub(crate) fn len(&self) -> usize {
        self.reported.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct Diagnostic(Rc<Cell<usize>>);

    impl Drop for Diagnostic {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn handled_moves_the_original_diagnostic_exactly_once() {
        let drops = Rc::new(Cell::new(0));
        let mut ledger = RejectionLedger::new();
        ledger.unhandled(7, Diagnostic(Rc::clone(&drops)));
        assert_eq!(ledger.len(), 1);
        assert!(ledger.handled(8).is_none());
        let diagnostic = ledger.handled(7).unwrap();
        assert_eq!(ledger.len(), 0);
        assert!(ledger.handled(7).is_none());
        drop(ledger);
        assert_eq!(drops.get(), 0);
        drop(diagnostic);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn shutdown_drops_unhandled_diagnostics_and_replacements() {
        let drops = Rc::new(Cell::new(0));
        {
            let mut ledger = RejectionLedger::new();
            for id in [1, 1, 2] {
                ledger.unhandled(id, Diagnostic(Rc::clone(&drops)));
            }
            assert_eq!(drops.get(), 1);
        }
        assert_eq!(drops.get(), 3);
    }

    #[test]
    fn identities_are_isolate_local_and_diagnostics_are_owned() {
        let mut first = RejectionLedger::new();
        let mut second = RejectionLedger::new();
        let mut description = String::from("original stack");
        first.unhandled(1, description.clone());
        second.unhandled(1, String::from("other isolate"));
        description.clear();
        assert_eq!(first.handled(1).as_deref(), Some("original stack"));
        assert_eq!(second.handled(1).as_deref(), Some("other isolate"));
    }
}
