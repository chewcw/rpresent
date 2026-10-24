use std::collections::{BTreeSet, VecDeque};

use crate::document::{Object, ObjectId};

/// One reversible document mutation.
///
/// Each entry pairs an object with its position in the list with every object
/// affected by the same change removed. Applying a record therefore works by
/// deleting the affected identifiers and inserting the replacement entries in
/// ascending order.
#[derive(Debug, Clone)]
pub(crate) struct Change {
    pub before: Vec<(usize, Object)>,
    pub after: Vec<(usize, Object)>,
}

/// Bounded undo and redo stacks that never touch the document itself.
#[derive(Debug)]
pub(crate) struct History {
    undo: VecDeque<Change>,
    redo: Vec<Change>,
    depth: usize,
}

impl History {
    pub fn new(depth: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            depth,
        }
    }

    pub fn record(&mut self, change: Change) {
        self.redo.clear();
        self.undo.push_back(change);
        while self.undo.len() > self.depth {
            self.undo.pop_front();
        }
    }

    pub fn undo(&mut self) -> Option<Change> {
        let change = self.undo.pop_back()?;
        self.redo.push(change.clone());
        Some(change)
    }

    pub fn redo(&mut self) -> Option<Change> {
        let change = self.redo.pop()?;
        self.undo.push_back(change.clone());
        Some(change)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

/// Entries for the affected objects, indexed as if the affected objects were
/// already removed from the list.
pub(crate) fn reduced_entries(
    objects: &[Object],
    affected: &BTreeSet<ObjectId>,
) -> Vec<(usize, Object)> {
    let mut index = 0;
    let mut entries = Vec::new();
    for object in objects {
        if affected.contains(&object.id) {
            entries.push((index, object.clone()));
        } else {
            index += 1;
        }
    }
    entries
}
