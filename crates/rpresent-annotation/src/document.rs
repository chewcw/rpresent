use std::collections::BTreeSet;

use crate::undo::{Change, History, reduced_entries};

pub type ObjectId = u64;
pub use crate::catalog::Rgb;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectData {
    Pen {
        points: Vec<Point>,
        width_px: u32,
    },
    Line {
        end: Point,
        width_px: u32,
    },
    Rect {
        end: Point,
        width_px: u32,
    },
    Ellipse {
        end: Point,
        width_px: u32,
    },
    Arrow {
        end: Point,
        width_px: u32,
    },
    Text {
        text: String,
        family: String,
        size_px: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub id: ObjectId,
    pub origin: Point,
    pub color: Rgb,
    pub data: ObjectData,
}

impl Object {
    /// Local points are relative to the origin, so moving an object only
    /// changes its origin.
    pub fn point(&self, offset: Point) -> Point {
        Point {
            x: self.origin.x + offset.x,
            y: self.origin.y + offset.y,
        }
    }
}

/// An ordered collection of annotation objects with bounded undo history.
#[derive(Debug)]
pub struct Document {
    objects: Vec<Object>,
    history: History,
    next_id: ObjectId,
}

impl Document {
    pub fn new(depth: usize) -> Self {
        Self {
            objects: Vec::new(),
            history: History::new(depth),
            next_id: 0,
        }
    }

    pub fn objects(&self) -> &[Object] {
        &self.objects
    }

    pub fn insert(&mut self, origin: Point, color: Rgb, data: ObjectData) -> ObjectId {
        let id = self.next_id;
        self.next_id += 1;
        let object = Object {
            id,
            origin,
            color,
            data,
        };
        let index = self.objects.len();
        self.objects.push(object.clone());
        self.history.record(Change {
            before: Vec::new(),
            after: vec![(index, object)],
        });
        id
    }

    pub fn move_by(&mut self, id: ObjectId, delta: Point) -> bool {
        if delta == Point::default() {
            return false;
        }
        let Some(index) = self.objects.iter().position(|object| object.id == id) else {
            return false;
        };
        let before = self.objects[index].clone();
        let mut after = before.clone();
        after.origin.x += delta.x;
        after.origin.y += delta.y;
        self.objects[index] = after.clone();
        self.history.record(Change {
            before: vec![(index, before)],
            after: vec![(index, after)],
        });
        true
    }

    pub fn replace(&mut self, id: ObjectId, origin: Point, color: Rgb, data: ObjectData) -> bool {
        let Some(index) = self.objects.iter().position(|object| object.id == id) else {
            return false;
        };
        let before = self.objects[index].clone();
        let after = Object {
            id,
            origin,
            color,
            data,
        };
        if before == after {
            return false;
        }
        self.objects[index] = after.clone();
        self.history.record(Change {
            before: vec![(index, before)],
            after: vec![(index, after)],
        });
        true
    }

    pub fn delete(&mut self, ids: &[ObjectId]) -> bool {
        if ids.is_empty() {
            return false;
        }
        let affected: BTreeSet<ObjectId> = ids.iter().copied().collect();
        let before = reduced_entries(&self.objects, &affected);
        if before.is_empty() {
            return false;
        }
        self.objects.retain(|object| !affected.contains(&object.id));
        self.history.record(Change {
            before,
            after: Vec::new(),
        });
        true
    }

    pub fn clear(&mut self) -> bool {
        if self.objects.is_empty() {
            return false;
        }
        let affected: BTreeSet<ObjectId> = self.objects.iter().map(|object| object.id).collect();
        let before = reduced_entries(&self.objects, &affected);
        self.objects.clear();
        self.history.record(Change {
            before,
            after: Vec::new(),
        });
        true
    }

    pub fn undo(&mut self) -> bool {
        let Some(change) = self.history.undo() else {
            return false;
        };
        self.apply(&change.after, &change.before);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(change) = self.history.redo() else {
            return false;
        };
        self.apply(&change.before, &change.after);
        true
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    fn apply(&mut self, remove: &[(usize, Object)], insert: &[(usize, Object)]) {
        let removed: BTreeSet<ObjectId> = remove.iter().map(|(_, object)| object.id).collect();
        self.objects.retain(|object| !removed.contains(&object.id));
        for (offset, (index, object)) in insert.iter().enumerate() {
            let at = (index + offset).min(self.objects.len());
            self.objects.insert(at, object.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(end: Point) -> ObjectData {
        ObjectData::Line { end, width_px: 6 }
    }

    fn rect(end: Point) -> ObjectData {
        ObjectData::Rect { end, width_px: 6 }
    }

    fn arrow(end: Point) -> ObjectData {
        ObjectData::Arrow { end, width_px: 6 }
    }

    fn ellipse(end: Point) -> ObjectData {
        ObjectData::Ellipse { end, width_px: 6 }
    }

    #[test]
    fn ellipse_round_trips_through_undo_and_redo() {
        let mut document = Document::new(4);
        let origin = Point { x: 10, y: 20 };
        let end = Point { x: 50, y: 30 };
        let id = document.insert(origin, [1, 2, 3], ellipse(end));
        assert!(document.undo());
        assert!(document.objects().is_empty());
        assert!(document.redo());
        assert_eq!(
            document.objects(),
            &[Object {
                id,
                origin,
                color: [1, 2, 3],
                data: ellipse(end),
            }]
        );
    }

    #[test]
    fn history_eviction_preserves_artwork() {
        let mut document = Document::new(1);
        let data = ObjectData::Line {
            end: Point { x: 20, y: 0 },
            width_px: 6,
        };
        let first = document.insert(Point { x: 0, y: 0 }, [255, 0, 0], data.clone());
        let second = document.insert(Point { x: 0, y: 20 }, [255, 0, 0], data);
        assert_eq!(document.objects().len(), 2);
        assert!(document.undo());
        assert_eq!(document.objects()[0].id, first);
        assert!(!document.undo());
        assert!(document.redo());
        assert_eq!(document.objects()[1].id, second);
    }

    #[test]
    fn undo_restores_exact_order_and_style() {
        let mut document = Document::new(8);
        let pen = document.insert(
            Point { x: 1, y: 2 },
            [1, 2, 3],
            ObjectData::Pen {
                points: vec![Point { x: 0, y: 0 }, Point { x: 5, y: 5 }],
                width_px: 4,
            },
        );
        let line = document.insert(Point { x: 9, y: 9 }, [4, 5, 6], line(Point { x: 4, y: 4 }));
        let text = document.insert(
            Point { x: 3, y: 3 },
            [7, 8, 9],
            ObjectData::Text {
                text: "hello".to_owned(),
                family: "sans-serif".to_owned(),
                size_px: 24,
            },
        );
        let original = document.objects().to_vec();

        assert!(document.move_by(line, Point { x: -4, y: 6 }));
        assert!(document.replace(
            pen,
            Point { x: 1, y: 2 },
            [9, 9, 9],
            ObjectData::Pen {
                points: vec![Point { x: 0, y: 0 }],
                width_px: 10,
            },
        ));
        assert!(document.delete(&[text]));

        assert!(document.undo());
        assert!(document.undo());
        assert!(document.undo());
        assert_eq!(document.objects(), original.as_slice());

        assert!(document.redo());
        assert!(document.redo());
        assert!(document.redo());
        assert_eq!(document.objects().len(), 2);
        assert_eq!(document.objects()[1].id, line);
    }

    #[test]
    fn clear_is_one_reversible_operation() {
        let mut document = Document::new(4);
        for index in 0..4 {
            document.insert(
                Point { x: index, y: 0 },
                [255, 255, 255],
                line(Point { x: 10, y: 10 }),
            );
        }
        let original = document.objects().to_vec();
        assert!(document.clear());
        assert!(document.objects().is_empty());
        assert!(document.undo());
        assert_eq!(document.objects(), original.as_slice());
        assert!(document.redo());
        assert!(document.objects().is_empty());
    }

    #[test]
    fn grouped_deletion_restores_relative_positions() {
        let mut document = Document::new(4);
        let ids: Vec<_> = (0..5)
            .map(|index| {
                document.insert(
                    Point { x: index, y: 0 },
                    [255, 0, 0],
                    line(Point { x: 1, y: 1 }),
                )
            })
            .collect();
        let original = document.objects().to_vec();
        assert!(document.delete(&[ids[1], ids[3]]));
        assert_eq!(document.objects().len(), 3);
        assert!(document.undo());
        assert_eq!(document.objects(), original.as_slice());
    }

    #[test]
    fn replacement_keeps_identity_and_order() {
        let mut document = Document::new(4);
        let first = document.insert(Point::default(), [0, 0, 0], line(Point { x: 1, y: 0 }));
        let second = document.insert(Point { x: 5, y: 5 }, [0, 0, 0], line(Point { x: 2, y: 0 }));
        assert!(document.replace(
            first,
            Point { x: 2, y: 2 },
            [1, 1, 1],
            ObjectData::Text {
                text: "new".to_owned(),
                family: "sans-serif".to_owned(),
                size_px: 16,
            },
        ));
        assert_eq!(document.objects()[0].id, first);
        assert_eq!(document.objects()[1].id, second);
        assert!(document.undo());
        assert_eq!(document.objects()[0].origin, Point::default());
    }

    #[test]
    fn new_change_invalidates_redo() {
        let mut document = Document::new(4);
        let first = document.insert(Point::default(), [0, 0, 0], line(Point { x: 1, y: 0 }));
        document.insert(Point { x: 3, y: 3 }, [0, 0, 0], line(Point { x: 1, y: 0 }));
        assert!(document.undo());
        assert!(document.move_by(first, Point { x: 1, y: 1 }));
        assert!(!document.redo());
        assert_eq!(document.objects().len(), 1);
        assert_eq!(document.objects()[0].origin, Point { x: 1, y: 1 });
    }

    #[test]
    fn no_op_mutations_create_no_history() {
        let mut document = Document::new(4);
        let id = document.insert(Point::default(), [0, 0, 0], line(Point { x: 1, y: 0 }));
        assert!(!document.move_by(id, Point::default()));
        assert!(!document.move_by(999, Point { x: 1, y: 1 }));
        assert!(!document.replace(id, Point::default(), [0, 0, 0], line(Point { x: 1, y: 0 })));
        assert!(!document.delete(&[]));
        assert!(!document.delete(&[999]));
        assert!(document.undo());
        assert!(document.objects().is_empty());
        assert!(!document.undo());
    }

    #[test]
    fn ids_are_never_reused_after_undo() {
        let mut document = Document::new(4);
        let first = document.insert(Point::default(), [0, 0, 0], line(Point { x: 1, y: 0 }));
        assert!(document.undo());
        let second = document.insert(Point::default(), [0, 0, 0], line(Point { x: 1, y: 0 }));
        assert_ne!(first, second);
    }

    #[test]
    fn empty_clear_is_a_no_op() {
        let mut document = Document::new(4);
        assert!(!document.clear());
        assert!(!document.undo());
    }

    #[test]
    fn history_availability_tracks_undo_redo_and_new_changes() {
        let mut document = Document::new(2);
        assert!(!document.can_undo());
        assert!(!document.can_redo());
        document.insert(
            Point { x: 10, y: 20 },
            [255, 0, 0],
            ObjectData::Line {
                end: Point { x: 30, y: 0 },
                width_px: 4,
            },
        );
        assert!(document.can_undo());
        assert!(!document.can_redo());
        assert!(document.undo());
        assert!(!document.can_undo());
        assert!(document.can_redo());
        assert!(document.redo());
        assert!(document.can_undo());
        assert!(!document.can_redo());
        assert!(document.undo());
        document.insert(
            Point::default(),
            [0, 255, 0],
            ObjectData::Line {
                end: Point { x: 1, y: 1 },
                width_px: 2,
            },
        );
        assert!(!document.can_redo());
    }

    #[test]
    fn rect_round_trips_through_undo_and_redo() {
        let mut document = Document::new(4);
        let origin = Point { x: 10, y: 20 };
        let end = Point { x: 40, y: 30 };
        let id = document.insert(origin, [1, 2, 3], rect(end));
        assert!(document.undo());
        assert!(document.objects().is_empty());
        assert!(document.redo());
        assert_eq!(
            document.objects(),
            &[Object {
                id,
                origin,
                color: [1, 2, 3],
                data: rect(end),
            }]
        );
    }

    #[test]
    fn rect_bounds_cover_stroke_width_and_negative_deltas() {
        let object = Object {
            id: 0,
            origin: Point { x: 100, y: 100 },
            color: [0, 0, 0],
            data: rect(Point { x: -40, y: 20 }),
        };
        let mut text_system = crate::text::TextSystem::new();
        let bounds = crate::render::object_paint_bounds(&mut text_system, &object);
        assert_eq!(
            bounds,
            crate::geometry::Rect {
                x: 55,
                y: 95,
                width: 50,
                height: 30,
            }
        );
    }

    #[test]
    fn arrow_round_trips_through_undo_and_redo() {
        let mut document = Document::new(4);
        let origin = Point { x: 10, y: 20 };
        let end = Point { x: 60, y: -10 };
        let id = document.insert(origin, [1, 2, 3], arrow(end));
        assert!(document.undo());
        assert!(document.objects().is_empty());
        assert!(document.redo());
        assert_eq!(
            document.objects(),
            &[Object {
                id,
                origin,
                color: [1, 2, 3],
                data: arrow(end),
            }]
        );
    }

    #[test]
    fn arrow_bounds_cover_the_shaft_and_head() {
        let end = Point { x: -80, y: 20 };
        let object = Object {
            id: 0,
            origin: Point { x: 10, y: 20 },
            color: [0, 0, 0],
            data: ObjectData::Arrow { end, width_px: 10 },
        };
        let mut text_system = crate::text::TextSystem::new();
        let outline = crate::geometry::arrow_outline(end, 10);
        let bounds = crate::render::object_paint_bounds(&mut text_system, &object);
        let min_x = outline
            .iter()
            .map(|local| object.point(*local).x)
            .min()
            .unwrap();
        assert!(bounds.x < min_x, "stroke padding");
        for local in outline {
            let absolute = object.point(local);
            assert!(
                bounds.x <= absolute.x && absolute.x <= bounds.right(),
                "{absolute:?} outside {bounds:?}"
            );
            assert!(
                bounds.y <= absolute.y && absolute.y <= bounds.bottom(),
                "{absolute:?} outside {bounds:?}"
            );
        }
    }
}
