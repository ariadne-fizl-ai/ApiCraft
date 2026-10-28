//! Undo and redo for the catalog. A snapshot is recorded once an edit has
//! settled (the catalog stopped changing for a moment), so a burst of typing
//! is one step while deleting a workflow step, moving a request or removing
//! a collection are each their own.
//!
//! Values captured from responses (`runtime`) and the chosen environment
//! aren't edits: they're left out of the comparison and kept as they are
//! when undoing.

use std::hash::{Hash, Hasher};

use apicraft_core::model::Catalog;

const LIMIT: usize = 100;

pub struct History {
    undo: Vec<Catalog>,
    redo: Vec<Catalog>,
    /// The last recorded state, and its hash.
    base: Catalog,
    base_hash: u64,
    /// The hash seen at the previous check, to tell when edits have settled.
    seen: u64,
}

/// A hash of the parts of the catalog that undo covers.
pub fn edit_hash(c: &Catalog) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(&(&c.collections, &c.environments, &c.workflows, &c.network)).unwrap_or_default().hash(&mut h);
    h.finish()
}

impl History {
    pub fn new(cat: &Catalog) -> Self {
        let h = edit_hash(cat);
        History { undo: Vec::new(), redo: Vec::new(), base: cat.clone(), base_hash: h, seen: h }
    }

    /// Called now and then (and before undoing): records the previous state
    /// once the catalog has changed and then held still since the last call,
    /// or straight away when `force` is set.
    pub fn observe(&mut self, cat: &Catalog, force: bool) {
        let h = edit_hash(cat);
        let settled = h == self.seen;
        self.seen = h;
        if h != self.base_hash && (settled || force) {
            let prev = std::mem::replace(&mut self.base, cat.clone());
            self.undo.push(prev);
            if self.undo.len() > LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.base_hash = h;
        }
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self, cat: &mut Catalog) -> bool {
        self.observe(cat, true);
        let Some(prev) = self.undo.pop() else { return false };
        let current = std::mem::replace(&mut self.base, prev.clone());
        self.redo.push(current);
        restore(cat, prev);
        self.base_hash = edit_hash(cat);
        self.seen = self.base_hash;
        true
    }

    pub fn redo(&mut self, cat: &mut Catalog) -> bool {
        self.observe(cat, true);
        let Some(next) = self.redo.pop() else { return false };
        let current = std::mem::replace(&mut self.base, next.clone());
        self.undo.push(current);
        restore(cat, next);
        self.base_hash = edit_hash(cat);
        self.seen = self.base_hash;
        true
    }
}

fn restore(cat: &mut Catalog, snapshot: Catalog) {
    let runtime = std::mem::take(&mut cat.runtime);
    let active = cat.active_environment.take();
    *cat = snapshot;
    cat.runtime = runtime;
    cat.active_environment = active.filter(|id| cat.environments.iter().any(|e| &e.id == id)).or(cat.active_environment.take());
}

#[cfg(test)]
mod tests {
    use super::*;
    use apicraft_core::model::*;

    #[test]
    fn undoes_and_redoes_settled_edits() {
        let mut cat = Catalog::default();
        let mut w = Workflow::new("Flow");
        w.steps = vec![WorkflowStep::new("a"), WorkflowStep::new("b")];
        cat.workflows.push(w);
        let mut h = History::new(&cat);
        cat.workflows[0].steps.remove(0);
        cat.runtime.push(KeyValue::new("captured", "1"));
        assert!(h.undo(&mut cat));
        assert_eq!(cat.workflows[0].steps.len(), 2, "the deleted step is back");
        assert_eq!(cat.runtime.len(), 1, "captured values are kept");
        assert!(h.redo(&mut cat));
        assert_eq!(cat.workflows[0].steps.len(), 1);
        assert!(!h.redo(&mut cat));
    }

    #[test]
    fn typing_is_one_step_once_it_settles() {
        let mut cat = Catalog::default();
        cat.collections.push(Collection::new("A"));
        let mut h = History::new(&cat);
        for name in ["Ab", "Abc", "Abcd"] {
            cat.collections[0].name = name.into();
            h.observe(&cat, false);
        }
        h.observe(&cat, false);
        cat.collections[0].name = "Abcd!".into();
        h.undo(&mut cat);
        assert_eq!(cat.collections[0].name, "Abcd");
        h.undo(&mut cat);
        assert_eq!(cat.collections[0].name, "A");
    }
}
