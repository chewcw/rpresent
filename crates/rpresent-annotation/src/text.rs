use cosmic_text::{
    Action, Attrs, Buffer, BufferRef, Change, Cursor, Edit, Editor, Family, FontSystem, Metrics,
    Motion, Selection, Shaping, SwashCache, Wrap,
};

use crate::document::{Object, ObjectData};
use crate::geometry::Rect;

const LINE_HEIGHT_FACTOR: f32 = 1.2;

/// One font system and glyph cache shared by every annotation text object.
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl TextSystem {
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Build a font system from one font file, with system discovery disabled.
    #[cfg(test)]
    pub(crate) fn with_font(font: &[u8]) -> Self {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(font.to_vec());
        Self {
            font_system: FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
            swash_cache: SwashCache::new(),
        }
    }

    pub(crate) fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Layout bounds of a committed text object, including glyph boxes.
    pub fn bounds(&mut self, object: &Object) -> Rect {
        let ObjectData::Text {
            text,
            family,
            size_px,
        } = &object.data
        else {
            return Rect::default();
        };
        let buffer = self.layout_buffer(text, family, *size_px);
        let line_height = line_height(*size_px);
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let left = glyph.x + glyph.x_offset;
                let top = run.line_y + glyph.y + glyph.y_offset;
                min_x = min_x.min(left);
                max_x = max_x.max(left + glyph.w);
                min_y = min_y.min(top);
                max_y = max_y.max(top + glyph.font_size);
            }
            min_y = min_y.min(run.line_top);
            max_y = max_y.max(run.line_top + run.line_height);
        }
        if !min_x.is_finite() || !min_y.is_finite() {
            return Rect {
                x: object.origin.x,
                y: object.origin.y,
                width: 0,
                height: line_height.ceil() as u32,
            };
        }
        let x = min_x.floor() as i32;
        let y = min_y.floor() as i32;
        Rect {
            x: object.origin.x + x,
            y: object.origin.y + y,
            width: (max_x.ceil() as i32 - x).max(0) as u32,
            height: (max_y.ceil() as i32 - y).max(0) as u32,
        }
    }

    /// Draw a committed text object and hand every shaded glyph pixel to
    /// `plot` as `(x, y, width, height, red, green, blue, alpha)`.
    pub(crate) fn draw_object<F>(&mut self, object: &Object, color: cosmic_text::Color, mut plot: F)
    where
        F: FnMut(i32, i32, u32, u32, u8, u8, u8, u8),
    {
        let ObjectData::Text {
            text,
            family,
            size_px,
        } = &object.data
        else {
            return;
        };
        let mut buffer = self.layout_buffer(text, family, *size_px);
        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            color,
            |x, y, width, height, color| {
                plot(
                    x,
                    y,
                    width,
                    height,
                    color.r(),
                    color.g(),
                    color.b(),
                    color.a(),
                );
            },
        );
    }

    fn layout_buffer(&mut self, text: &str, family: &str, size_px: u32) -> Buffer {
        let mut buffer = Buffer::new_empty(metrics(size_px));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        buffer.set_text(text, &attrs(family), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        buffer
    }
}

pub(crate) fn metrics(size_px: u32) -> Metrics {
    Metrics::new(size_px as f32, line_height(size_px))
}

pub(crate) fn line_height(size_px: u32) -> f32 {
    size_px as f32 * LINE_HEIGHT_FACTOR
}

pub(crate) fn attrs(family: &str) -> Attrs<'_> {
    match family {
        "sans-serif" => Attrs::new().family(Family::SansSerif),
        "serif" => Attrs::new().family(Family::Serif),
        "monospace" => Attrs::new().family(Family::Monospace),
        name => Attrs::new().family(Family::Name(name)),
    }
}

struct DraftChange {
    change: Change,
    before: (Cursor, Selection),
    after: (Cursor, Selection),
}

/// An editable text object. Draft identifiers are separate from object IDs.
pub struct TextDraft {
    id: u64,
    editor: Editor<'static>,
    family: String,
    size_px: u32,
    undo: Vec<DraftChange>,
    redo: Vec<DraftChange>,
}

impl TextDraft {
    pub fn new(id: u64, text: &str, family: &str, size_px: u32, fonts: &mut TextSystem) -> Self {
        let mut buffer = Buffer::new_empty(metrics(size_px));
        buffer.set_wrap(Wrap::None);
        buffer.set_size(None, None);
        buffer.set_text(text, &attrs(family), Shaping::Advanced, None);
        let mut editor = Editor::new(BufferRef::<'static>::Owned(buffer));
        let end = end_cursor(&editor);
        editor.set_cursor(end);
        editor.shape_as_needed(fonts.font_system_mut(), true);
        Self {
            id,
            editor,
            family: family.to_owned(),
            size_px,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn plain_text(&self) -> String {
        self.editor.with_buffer(|buffer| {
            buffer
                .lines
                .iter()
                .map(|line| line.text())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    /// Insert text at the caret or replace the current selection. One call is
    /// one reversible draft change.
    pub fn insert(&mut self, text: &str, fonts: &mut TextSystem) {
        let before = (self.editor.cursor(), self.editor.selection());
        self.editor.start_change();
        self.editor.insert_string(text, None);
        let after = (self.editor.cursor(), self.editor.selection());
        if let Some(change) = self.editor.finish_change() {
            self.undo.push(DraftChange {
                change,
                before,
                after,
            });
            self.redo.clear();
        }
        self.editor.shape_as_needed(fonts.font_system_mut(), true);
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        let mut change = entry.change.clone();
        change.reverse();
        self.editor.apply_change(&change);
        self.editor.set_cursor(entry.before.0);
        self.editor.set_selection(entry.before.1);
        self.redo.push(entry);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo.pop() else {
            return false;
        };
        self.editor.apply_change(&entry.change);
        self.editor.set_cursor(entry.after.0);
        self.editor.set_selection(entry.after.1);
        self.undo.push(entry);
        true
    }

    /// Apply one editing action such as a deletion. A text change becomes one
    /// reversible draft change; pure caret movement records nothing.
    pub fn perform(&mut self, action: Action, fonts: &mut TextSystem) {
        let before = (self.editor.cursor(), self.editor.selection());
        self.editor.start_change();
        self.editor.action(fonts.font_system_mut(), action);
        let after = (self.editor.cursor(), self.editor.selection());
        if let Some(change) = self.editor.finish_change() {
            self.undo.push(DraftChange {
                change,
                before,
                after,
            });
            self.redo.clear();
        }
        self.editor.shape_as_needed(fonts.font_system_mut(), true);
    }

    /// Move the caret, extending the selection when `extend_selection` is set.
    pub fn move_caret(&mut self, motion: Motion, extend_selection: bool, fonts: &mut TextSystem) {
        let anchor = match self.editor.selection() {
            Selection::Normal(anchor) => Some(anchor),
            _ => None,
        };
        let before = self.editor.cursor();
        self.editor
            .action(fonts.font_system_mut(), Action::Motion(motion));
        if extend_selection {
            self.editor
                .set_selection(Selection::Normal(anchor.unwrap_or(before)));
        } else if anchor.is_some() {
            self.editor.set_selection(Selection::None);
        }
        self.editor.shape_as_needed(fonts.font_system_mut(), true);
    }

    pub fn select_all(&mut self) {
        let end = end_cursor(&self.editor);
        self.editor.set_cursor(Cursor::new(0, 0));
        self.editor.set_selection(Selection::Normal(end));
    }

    pub fn copy_selection(&self) -> Option<String> {
        self.editor.copy_selection()
    }

    pub fn delete_selection(&mut self) -> bool {
        self.editor.delete_selection()
    }

    pub fn cursor(&self) -> Cursor {
        self.editor.cursor()
    }

    pub fn selection(&self) -> Selection {
        self.editor.selection()
    }

    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.editor.set_cursor(cursor);
    }

    pub fn set_selection(&mut self, selection: Selection) {
        self.editor.set_selection(selection);
    }

    pub fn family(&self) -> &str {
        &self.family
    }

    pub fn size_px(&self) -> u32 {
        self.size_px
    }

    /// Screen-space bounds of the draft's text placed at `origin`.
    pub fn bounds(&mut self, origin: crate::document::Point, fonts: &mut TextSystem) -> Rect {
        self.editor.shape_as_needed(fonts.font_system_mut(), true);
        let line_height = line_height(self.size_px);
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        self.editor.with_buffer(|buffer| {
            for run in buffer.layout_runs() {
                for glyph in run.glyphs {
                    let left = glyph.x + glyph.x_offset;
                    let top = run.line_y + glyph.y + glyph.y_offset;
                    min_x = min_x.min(left);
                    max_x = max_x.max(left + glyph.w);
                    min_y = min_y.min(top);
                    max_y = max_y.max(top + glyph.font_size);
                }
                min_y = min_y.min(run.line_top);
                max_y = max_y.max(run.line_top + run.line_height);
            }
        });
        if !min_x.is_finite() || !min_y.is_finite() {
            return Rect {
                x: origin.x,
                y: origin.y,
                width: 0,
                height: line_height.ceil() as u32,
            };
        }
        let x = min_x.floor() as i32;
        let y = min_y.floor() as i32;
        Rect {
            x: origin.x + x,
            y: origin.y + y,
            width: (max_x.ceil() as i32 - x).max(0) as u32,
            height: (max_y.ceil() as i32 - y).max(0) as u32,
        }
    }

    pub(crate) fn draw<F>(
        &mut self,
        fonts: &mut TextSystem,
        text_color: cosmic_text::Color,
        cursor_color: cosmic_text::Color,
        selection_color: cosmic_text::Color,
        selected_text_color: cosmic_text::Color,
        mut plot: F,
    ) where
        F: FnMut(i32, i32, u32, u32, u8, u8, u8, u8),
    {
        self.editor.draw(
            &mut fonts.font_system,
            &mut fonts.swash_cache,
            text_color,
            cursor_color,
            selection_color,
            selected_text_color,
            |x, y, width, height, color| {
                plot(
                    x,
                    y,
                    width,
                    height,
                    color.r(),
                    color.g(),
                    color.b(),
                    color.a(),
                );
            },
        );
    }
}

fn end_cursor(editor: &Editor<'static>) -> Cursor {
    editor.with_buffer(|buffer| {
        let line = buffer.lines.len().saturating_sub(1);
        let index = buffer.lines.get(line).map_or(0, |line| line.text().len());
        Cursor::new(line, index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FONT: &[u8] = include_bytes!("../tests/fixtures/DejaVuSans.ttf");

    fn text_object(origin: crate::document::Point, text: &str, size_px: u32) -> Object {
        Object {
            id: 1,
            origin,
            color: [255, 0, 0],
            data: ObjectData::Text {
                text: text.to_owned(),
                family: "sans-serif".to_owned(),
                size_px,
            },
        }
    }

    #[test]
    fn pasted_unicode_is_one_reversible_draft_change() {
        let mut fonts = TextSystem::new();
        let mut draft = TextDraft::new(1, "Label", "sans-serif", 24, &mut fonts);
        draft.insert("\n你好", &mut fonts);
        assert_eq!(draft.plain_text(), "Label\n你好");
        assert!(draft.undo());
        assert_eq!(draft.plain_text(), "Label");
        assert!(draft.redo());
        assert_eq!(draft.plain_text(), "Label\n你好");
    }

    #[test]
    fn draft_starts_with_the_caret_at_the_end() {
        let mut fonts = TextSystem::with_font(TEST_FONT);
        let draft = TextDraft::new(1, "two\nlines", "sans-serif", 24, &mut fonts);
        let cursor = draft.cursor();
        assert_eq!((cursor.line, cursor.index), (1, 5));
    }

    #[test]
    fn multiline_layout_grows_with_explicit_newlines() {
        let mut fonts = TextSystem::with_font(TEST_FONT);
        let one = text_object(crate::document::Point { x: 10, y: 20 }, "one", 24);
        let three = text_object(
            crate::document::Point { x: 10, y: 20 },
            "one\ntwo\nthree",
            24,
        );
        let one_bounds = fonts.bounds(&one);
        let three_bounds = fonts.bounds(&three);
        assert!(one_bounds.width > 0);
        assert!(one_bounds.height > 0);
        let two_more_lines = 2.0 * line_height(24.0 as u32);
        assert!(three_bounds.height as f32 >= one_bounds.height as f32 + two_more_lines);
        assert_eq!(three_bounds.x, one_bounds.x);
    }

    #[test]
    fn empty_and_reopened_text_layout_agree() {
        let mut fonts = TextSystem::with_font(TEST_FONT);
        let empty = text_object(crate::document::Point { x: 4, y: 6 }, "", 32);
        let empty_bounds = fonts.bounds(&empty);
        assert_eq!(empty_bounds.x, 4);
        assert_eq!(empty_bounds.y, 6);
        assert_eq!(empty_bounds.width, 0);
        assert!(empty_bounds.height > 0);

        let committed = text_object(crate::document::Point { x: 4, y: 6 }, "Label\nTwo", 32);
        let mut draft = TextDraft::new(9, "Label\nTwo", "sans-serif", 32, &mut fonts);
        let draft_bounds = draft.bounds(crate::document::Point { x: 4, y: 6 }, &mut fonts);
        assert_eq!(fonts.bounds(&committed), draft_bounds);
        assert_eq!(draft.plain_text(), "Label\nTwo");
        draft.insert("!", &mut fonts);
        assert_eq!(draft.plain_text(), "Label\nTwo!");
    }
}
