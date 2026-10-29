use std::collections::BTreeMap;
use std::time::Duration;

use cosmic_text::{Action, Motion};
use rpresent_config::shortcuts::{
    Binding, Key, KeyInput as ShortcutInput, ShortcutAction, ShortcutDispatcher,
};
use rpresent_config::{AnnotationConfig, AnnotationTool, ConfigError};

use crate::catalog::{Preset, Rgb, cycle_index, parse_rgb};
use crate::command::AnnotationCommand;
use crate::document::{Document, Object, ObjectData, ObjectId, Point};
use crate::geometry::{
    Rect, arrow_outline, ellipse_outline, point_rect_distance, point_segment_distance,
    rect_outline, segment_rect_distance, segment_segment_distance,
};
use crate::text::{TextDraft, TextSystem};
use crate::toolbar::{DisplayGeometry, Toolbar, ToolbarLayout};
use crate::x11_clipboard::{
    PasteRequest, PasteResult, normalize_newlines, paste_belongs_to_session,
};

/// Extra screen pixels accepted around a visible path when hit testing.
pub(crate) const HIT_TOLERANCE_PX: f64 = 4.0;
/// Maximum distance between two clicks that can form a double click.
pub(crate) const DOUBLE_CLICK_SLOP_PX: i32 = 4;
pub(crate) const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);
/// Movement needed before a selected object starts moving.
pub(crate) const DRAG_THRESHOLD_PX: i32 = 4;
pub(crate) const DEFAULT_TEXT_FAMILY: &str = "sans-serif";
pub(crate) const TEXT_BOX_PADDING_PX: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerInput {
    Down {
        root: Point,
        time: Duration,
        modifiers: Modifiers,
    },
    Move {
        root: Point,
        time: Duration,
        modifiers: Modifiers,
    },
    Up {
        root: Point,
        time: Duration,
        modifiers: Modifiers,
    },
}

impl PointerInput {
    pub fn root(&self) -> Point {
        match self {
            PointerInput::Down { root, .. }
            | PointerInput::Move { root, .. }
            | PointerInput::Up { root, .. } => *root,
        }
    }

    pub fn time(&self) -> Duration {
        match self {
            PointerInput::Down { time, .. }
            | PointerInput::Move { time, .. }
            | PointerInput::Up { time, .. } => *time,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput {
    pub key: Key,
    pub text: Option<char>,
    pub modifiers: Modifiers,
    pub shortcut: Option<ShortcutInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionStatus {
    pub editing_requested: bool,
    pub has_content: bool,
    pub text_editing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Gesture {
    Pen {
        origin: Point,
        points: Vec<Point>,
    },
    Line {
        origin: Point,
        end: Point,
    },
    Rect {
        origin: Point,
        end: Point,
    },
    Ellipse {
        origin: Point,
        end: Point,
    },
    Arrow {
        origin: Point,
        end: Point,
    },
    Move {
        id: ObjectId,
        start: Point,
        offset: Point,
        moved: bool,
    },
    Erase {
        hidden: Vec<ObjectId>,
        last: Point,
    },
}

pub(crate) struct TextEdit {
    pub id: u64,
    pub object: Option<ObjectId>,
    pub origin: Point,
    pub color: Rgb,
    pub draft: TextDraft,
}

pub struct Session {
    editing_requested: bool,
    owner_epoch: Option<u64>,
    tool: AnnotationTool,
    colors: Vec<Rgb>,
    color_index: usize,
    widths: Vec<u32>,
    width_index: usize,
    text_sizes: Vec<u32>,
    text_size_index: usize,
    document: Document,
    shortcuts: ShortcutDispatcher,
    pub(crate) text_system: TextSystem,
    selection: Option<ObjectId>,
    pub(crate) gesture: Option<Gesture>,
    pub(crate) text_edit: Option<TextEdit>,
    pub(crate) damage: Option<Rect>,
    last_click: Option<(ObjectId, Duration, Point)>,
    next_draft_id: u64,
    next_paste_sequence: u64,
    next_paste_expected: u64,
    paste_requests: Vec<PasteRequest>,
    pending_pastes: BTreeMap<u64, Result<String, String>>,
    /// Present only after display geometry is known; canvas-only sessions
    /// (and tests without geometry) keep it `None`.
    toolbar: Option<Toolbar>,
}

impl Session {
    pub fn new(config: &AnnotationConfig, bindings: &[Binding]) -> Result<Self, ConfigError> {
        config.validate()?;
        let colors: Vec<Rgb> = config
            .colors
            .iter()
            .map(|text| parse_rgb(text).expect("validated annotation color"))
            .collect();
        let initial_color = parse_rgb(&config.color).expect("validated annotation color");
        let color_index = colors
            .iter()
            .position(|color| *color == initial_color)
            .expect("validated annotation color selection");
        let width_index = config
            .stroke_widths_px
            .iter()
            .position(|width| *width == config.stroke_width_px)
            .expect("validated stroke width selection");
        let text_size_index = config
            .text_sizes_px
            .iter()
            .position(|size| *size == config.text_size_px)
            .expect("validated text size selection");
        Ok(Self {
            editing_requested: false,
            owner_epoch: None,
            tool: config.default_tool,
            colors,
            color_index,
            widths: config.stroke_widths_px.clone(),
            width_index,
            text_sizes: config.text_sizes_px.clone(),
            text_size_index,
            document: Document::new(config.history_depth),
            shortcuts: ShortcutDispatcher::new(bindings),
            text_system: TextSystem::new(),
            selection: None,
            gesture: None,
            text_edit: None,
            damage: None,
            last_click: None,
            next_draft_id: 1,
            next_paste_sequence: 1,
            next_paste_expected: 1,
            paste_requests: Vec::new(),
            pending_pastes: BTreeMap::new(),
            toolbar: None,
        })
    }

    pub fn document(&self) -> &Document {
        &self.document
    }

    pub fn status(&self) -> SessionStatus {
        SessionStatus {
            editing_requested: self.editing_requested,
            has_content: !self.document.objects().is_empty(),
            text_editing: self.text_edit.is_some(),
        }
    }

    pub fn selection(&self) -> Option<ObjectId> {
        self.selection
    }

    pub fn tool(&self) -> AnnotationTool {
        self.tool
    }

    pub fn color(&self) -> Rgb {
        self.colors[self.color_index]
    }

    pub fn width_px(&self) -> u32 {
        self.widths[self.width_index]
    }

    pub fn text_size_px(&self) -> u32 {
        self.text_sizes[self.text_size_index]
    }

    /// Read-only view of the authoritative drawing settings and action
    /// availability for the toolbar. Borrows the configured catalogs in
    /// place; never clones them or mutates history.
    pub(crate) fn toolbar_values(&self) -> crate::toolbar::ToolbarValues<'_> {
        crate::toolbar::ToolbarValues {
            editing: self.editing_requested,
            tool: self.tool,
            color: self.color(),
            width_px: self.width_px(),
            text_size_px: self.text_size_px(),
            colors: &self.colors,
            widths: &self.widths,
            text_sizes: &self.text_sizes,
            can_undo: self.document.can_undo(),
            can_redo: self.document.can_redo(),
            has_selection: self.selection.is_some(),
            has_content: !self.document.objects().is_empty(),
        }
    }

    pub fn draft_id(&self) -> Option<u64> {
        self.text_edit.as_ref().map(|edit| edit.id)
    }

    pub fn take_paste_requests(&mut self) -> Vec<PasteRequest> {
        std::mem::take(&mut self.paste_requests)
    }

    /// Apply an ordered clipboard result to the active draft. Results that
    /// arrive ahead of their sequence wait until earlier ones are applied.
    pub fn accept_paste(&mut self, result: PasteResult) {
        if !paste_belongs_to_session(self.draft_id(), result.request) {
            return;
        }
        if result.request.sequence < self.next_paste_expected {
            return;
        }
        self.pending_pastes
            .insert(result.request.sequence, result.text);
        while let Some(text) = self.pending_pastes.remove(&self.next_paste_expected) {
            self.next_paste_expected += 1;
            if let Ok(text) = text {
                let text = normalize_newlines(&text);
                if !text.is_empty() {
                    self.draft_insert(&text);
                }
            }
        }
    }

    pub fn command(&mut self, command: AnnotationCommand) -> Result<(), String> {
        let result = self.apply_command(command);
        self.refresh_toolbar();
        result
    }

    fn apply_command(&mut self, command: AnnotationCommand) -> Result<(), String> {
        match command {
            AnnotationCommand::On => {
                self.editing_requested = true;
            }
            AnnotationCommand::Off => {
                self.mark_selection();
                self.cancel();
                self.selection = None;
                self.editing_requested = false;
            }
            AnnotationCommand::Toggle => {
                if self.editing_requested {
                    self.mark_selection();
                    self.cancel();
                    self.selection = None;
                }
                self.editing_requested = !self.editing_requested;
            }
            AnnotationCommand::Clear => {
                self.mark_all();
                self.mark_selection();
                self.cancel();
                self.selection = None;
                self.document.clear();
            }
            AnnotationCommand::Undo => {
                self.mark_all();
                self.mark_selection();
                self.cancel();
                self.selection = None;
                self.document.undo();
                self.mark_all();
            }
            AnnotationCommand::Redo => {
                self.mark_all();
                self.mark_selection();
                self.cancel();
                self.selection = None;
                self.document.redo();
                self.mark_all();
            }
            AnnotationCommand::Delete => {
                self.mark_selection();
                self.cancel();
                if let Some(id) = self.selection.take() {
                    self.document.delete(&[id]);
                }
            }
            AnnotationCommand::Tool(tool) => {
                self.cancel_transient();
                self.tool = tool;
            }
            AnnotationCommand::Color(preset) => {
                let (_, index) = resolve_preset(preset, &self.colors, self.color_index)?;
                self.cancel_transient();
                self.color_index = index;
            }
            AnnotationCommand::Width(preset) => {
                let (_, index) = resolve_preset(preset, &self.widths, self.width_index)?;
                self.cancel_transient();
                self.width_index = index;
            }
            AnnotationCommand::TextSize(preset) => {
                let (_, index) = resolve_preset(preset, &self.text_sizes, self.text_size_index)?;
                self.cancel_transient();
                self.text_size_index = index;
            }
        }
        Ok(())
    }

    pub fn pointer(&mut self, epoch: u64, input: PointerInput) {
        if self.owner_epoch != Some(epoch) || !self.editing_requested {
            return;
        }
        let reply = if let Some(mut toolbar) = self.toolbar.take() {
            let reply = toolbar.pointer(input, &self.toolbar_values());
            self.toolbar = Some(toolbar);
            reply
        } else {
            crate::toolbar::ToolbarReply::default()
        };
        if let Some(damage) = reply.damage {
            self.mark(damage);
        }
        if let Some(command) = reply.command {
            self.command(command)
                .expect("toolbar command uses a configured value");
        }
        if !reply.consumed {
            self.canvas_pointer(input);
        }
        self.refresh_toolbar();
    }

    fn canvas_pointer(&mut self, input: PointerInput) {
        if self.text_edit.is_some() && self.draft_pointer(input) {
            return;
        }
        match input {
            PointerInput::Down { root, time, .. } => self.pointer_down(root, time),
            PointerInput::Move {
                root, modifiers, ..
            } => self.pointer_move(root, modifiers),
            PointerInput::Up {
                root, modifiers, ..
            } => self.pointer_up(root, modifiers),
        }
    }

    pub fn ownership(&mut self, epoch: Option<u64>) {
        if self.owner_epoch == epoch {
            return;
        }
        self.owner_epoch = epoch;
        if epoch.is_none() {
            self.cancel();
        } else {
            // A new epoch must never complete a toolbar press from an older
            // one.
            self.cancel_toolbar();
        }
    }

    pub fn cancel(&mut self) {
        self.mark_edited_object();
        self.mark_transient();
        self.gesture = None;
        self.text_edit = None;
        self.clear_pastes();
        self.last_click = None;
        self.cancel_toolbar();
    }

    /// Cancel toolbar interactions (presses, drags, open pickers) while
    /// keeping the committed toolbar position. Used when ownership changes
    /// and when the backend is lost.
    pub(crate) fn cancel_toolbar(&mut self) {
        let Some(mut toolbar) = self.toolbar.take() else {
            return;
        };
        let damage = toolbar.cancel(&self.toolbar_values());
        self.toolbar = Some(toolbar);
        if let Some(damage) = damage {
            self.mark(damage);
        }
        self.refresh_toolbar();
    }

    /// Rebuild the visible toolbar layout and mark any change to its painted
    /// bounds as damage.
    fn refresh_toolbar(&mut self) {
        let Some(mut toolbar) = self.toolbar.take() else {
            return;
        };
        let damage = toolbar.sync(&self.toolbar_values());
        self.toolbar = Some(toolbar);
        if let Some(damage) = damage {
            self.mark(damage);
        }
    }

    /// Adopt display geometry, creating the toolbar on first use. Existing
    /// canvas-only sessions without geometry are untouched by tests that
    /// never call this.
    pub(crate) fn update_display(&mut self, geometry: DisplayGeometry) {
        match self.toolbar.take() {
            Some(mut toolbar) => {
                let damage = toolbar.set_geometry(geometry, &self.toolbar_values());
                self.toolbar = Some(toolbar);
                if let Some(damage) = damage {
                    self.mark(damage);
                }
            }
            None => {
                self.toolbar = Some(Toolbar::new(geometry));
                self.refresh_toolbar();
            }
        }
    }

    pub(crate) fn toolbar_layout(&self) -> Option<&ToolbarLayout> {
        self.toolbar.as_ref().and_then(|toolbar| toolbar.layout())
    }

    fn cancel_transient(&mut self) {
        self.mark_edited_object();
        self.mark_transient();
        self.gesture = None;
        self.text_edit = None;
        self.clear_pastes();
        self.last_click = None;
    }

    pub fn key(&mut self, epoch: u64, input: KeyInput) {
        if self.owner_epoch != Some(epoch) || !self.editing_requested {
            return;
        }
        if input.key == Key::Esc {
            let (consumed, damage) = {
                let Some(mut toolbar) = self.toolbar.take() else {
                    self.canvas_key(input);
                    self.refresh_toolbar();
                    return;
                };
                let reply = toolbar.escape(&self.toolbar_values());
                self.toolbar = Some(toolbar);
                reply
            };
            if let Some(damage) = damage {
                self.mark(damage);
            }
            if consumed {
                self.refresh_toolbar();
                return;
            }
        }
        self.canvas_key(input);
        self.refresh_toolbar();
    }

    fn canvas_key(&mut self, input: KeyInput) {
        if self.text_edit.is_some() {
            self.draft_key(input);
            return;
        }
        if input.key == Key::Esc {
            self.mark_transient();
            self.mark_selection();
            self.escape();
            return;
        }
        if let Some(shortcut) = input.shortcut
            && let Some(action) = self.shortcuts.dispatch(shortcut)
        {
            self.apply_shortcut(action)
                .expect("validated annotation shortcuts use configured commands");
        }
    }

    /// Apply a configured annotation action. Global actions are rejected so
    /// annotation input can never toggle another layer.
    pub fn apply_shortcut(&mut self, action: ShortcutAction) -> Result<(), String> {
        match action {
            ShortcutAction::AnnotationUndo => self.apply_command(AnnotationCommand::Undo),
            ShortcutAction::AnnotationRedo => self.apply_command(AnnotationCommand::Redo),
            ShortcutAction::AnnotationDelete => self.apply_command(AnnotationCommand::Delete),
            ShortcutAction::AnnotationClear => self.apply_command(AnnotationCommand::Clear),
            ShortcutAction::ToolSelect => {
                self.set_tool(AnnotationTool::Select);
                Ok(())
            }
            ShortcutAction::ToolPen => {
                self.set_tool(AnnotationTool::Pen);
                Ok(())
            }
            ShortcutAction::ToolLine => {
                self.set_tool(AnnotationTool::Line);
                Ok(())
            }
            ShortcutAction::ToolArrow => {
                self.set_tool(AnnotationTool::Arrow);
                Ok(())
            }
            ShortcutAction::ToolRect => {
                self.set_tool(AnnotationTool::Rect);
                Ok(())
            }
            ShortcutAction::ToolEllipse => {
                self.set_tool(AnnotationTool::Ellipse);
                Ok(())
            }
            ShortcutAction::ToolEraser => {
                self.set_tool(AnnotationTool::Eraser);
                Ok(())
            }
            ShortcutAction::ToolText => {
                self.set_tool(AnnotationTool::Text);
                Ok(())
            }
            ShortcutAction::ColorNext => self.apply_command(AnnotationCommand::Color(Preset::Next)),
            ShortcutAction::ColorPrev => self.apply_command(AnnotationCommand::Color(Preset::Prev)),
            ShortcutAction::WidthPrev => self.apply_command(AnnotationCommand::Width(Preset::Prev)),
            ShortcutAction::WidthNext => self.apply_command(AnnotationCommand::Width(Preset::Next)),
            ShortcutAction::TextSizePrev => {
                self.apply_command(AnnotationCommand::TextSize(Preset::Prev))
            }
            ShortcutAction::TextSizeNext => {
                self.apply_command(AnnotationCommand::TextSize(Preset::Next))
            }
            ShortcutAction::SpotlightToggle
            | ShortcutAction::AnnotationToggle
            | ShortcutAction::ZoomToggle => Err(format!("{action:?} is not an annotation action")),
        }
    }

    fn escape(&mut self) {
        if self.gesture.is_some() {
            self.gesture = None;
        } else if self.selection.is_some() {
            self.selection = None;
        } else {
            self.editing_requested = false;
        }
    }

    fn set_tool(&mut self, tool: AnnotationTool) {
        if self.tool != tool {
            self.cancel_transient();
            self.tool = tool;
        }
    }

    fn draft_key(&mut self, input: KeyInput) {
        let ctrl = input.modifiers.ctrl;
        let shift = input.modifiers.shift;
        if ctrl {
            self.mark_transient();
            match lower_letter(input.key) {
                Some('z') if shift => {
                    if let Some(edit) = self.text_edit.as_mut() {
                        edit.draft.redo();
                    }
                }
                Some('z') => {
                    if let Some(edit) = self.text_edit.as_mut() {
                        edit.draft.undo();
                    }
                }
                Some('y') => {
                    if let Some(edit) = self.text_edit.as_mut() {
                        edit.draft.redo();
                    }
                }
                Some('a') => {
                    if let Some(edit) = self.text_edit.as_mut() {
                        edit.draft.select_all();
                    }
                }
                Some('v') => {
                    if let Some(id) = self.text_edit.as_ref().map(|edit| edit.id) {
                        self.paste_requests.push(PasteRequest {
                            session: id,
                            sequence: self.next_paste_sequence,
                        });
                        self.next_paste_sequence += 1;
                    }
                }
                _ => {}
            }
            self.mark_transient();
            return;
        }
        match input.key {
            Key::Enter => {
                if shift {
                    self.draft_insert("\n");
                } else {
                    self.commit_text();
                }
            }
            Key::Esc => self.cancel_text(),
            Key::Backspace => self.draft_action(Action::Backspace),
            Key::Delete => self.draft_action(Action::Delete),
            Key::Left => self.draft_motion(Motion::Left, shift),
            Key::Right => self.draft_motion(Motion::Right, shift),
            Key::Up => self.draft_motion(Motion::Up, shift),
            Key::Down => self.draft_motion(Motion::Down, shift),
            Key::Home => self.draft_motion(Motion::Home, shift),
            Key::End => self.draft_motion(Motion::End, shift),
            _ => {
                if let Some(character) = input.text {
                    self.draft_insert(&character.to_string());
                }
            }
        }
    }

    fn draft_insert(&mut self, text: &str) {
        self.mark_transient();
        if let Some(edit) = self.text_edit.as_mut() {
            edit.draft.insert(text, &mut self.text_system);
        }
        self.mark_transient();
    }

    fn draft_action(&mut self, action: Action) {
        self.mark_transient();
        if let Some(edit) = self.text_edit.as_mut() {
            edit.draft.perform(action, &mut self.text_system);
        }
        self.mark_transient();
    }

    fn draft_motion(&mut self, motion: Motion, extend_selection: bool) {
        self.mark_transient();
        if let Some(edit) = self.text_edit.as_mut() {
            edit.draft
                .move_caret(motion, extend_selection, &mut self.text_system);
        }
        self.mark_transient();
    }

    /// Route pointer input to the draft. Returns false when the pointer is
    /// outside the editing box, in which case the draft is committed first.
    fn draft_pointer(&mut self, input: PointerInput) -> bool {
        let root = input.root();
        let (origin, bounds) = {
            let Some(edit) = self.text_edit.as_mut() else {
                return false;
            };
            let origin = edit.origin;
            let bounds = edit
                .draft
                .bounds(origin, &mut self.text_system)
                .padded(TEXT_BOX_PADDING_PX);
            (origin, bounds)
        };
        if matches!(input, PointerInput::Down { .. }) && !bounds.contains(root) {
            self.commit_text();
            return false;
        }
        if !bounds.contains(root) {
            return true;
        }
        let local = Point {
            x: root.x - origin.x,
            y: root.y - origin.y,
        };
        let action = match input {
            PointerInput::Down { .. } => Action::Click {
                x: local.x,
                y: local.y,
            },
            PointerInput::Move { .. } => Action::Drag {
                x: local.x,
                y: local.y,
            },
            PointerInput::Up { .. } => return true,
        };
        self.mark_transient();
        if let Some(edit) = self.text_edit.as_mut() {
            edit.draft.perform(action, &mut self.text_system);
        }
        self.mark_transient();
        true
    }

    fn pointer_down(&mut self, root: Point, time: Duration) {
        self.mark_transient();
        self.mark_selection();
        match self.tool {
            AnnotationTool::Pen => {
                self.gesture = Some(Gesture::Pen {
                    origin: root,
                    points: vec![Point::default()],
                });
            }
            AnnotationTool::Line => {
                self.gesture = Some(Gesture::Line {
                    origin: root,
                    end: Point::default(),
                });
            }
            AnnotationTool::Rect => {
                self.gesture = Some(Gesture::Rect {
                    origin: root,
                    end: Point::default(),
                });
            }
            AnnotationTool::Ellipse => {
                self.gesture = Some(Gesture::Ellipse {
                    origin: root,
                    end: Point::default(),
                });
            }
            AnnotationTool::Arrow => {
                self.gesture = Some(Gesture::Arrow {
                    origin: root,
                    end: Point::default(),
                });
            }
            AnnotationTool::Eraser => {
                let hidden = self.erase_hits(root, root);
                self.gesture = Some(Gesture::Erase { hidden, last: root });
            }
            AnnotationTool::Select => self.select_down(root, time),
            AnnotationTool::Text => self.text_down(root, time),
        }
        self.mark_transient();
        self.mark_selection();
    }

    fn pointer_move(&mut self, root: Point, modifiers: Modifiers) {
        self.mark_transient();
        match &mut self.gesture {
            Some(Gesture::Pen { origin, points }) => {
                let local = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                if points.last() != Some(&local) {
                    points.push(local);
                }
            }
            Some(Gesture::Line { origin, end })
            | Some(Gesture::Rect { origin, end })
            | Some(Gesture::Arrow { origin, end }) => {
                *end = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
            }
            Some(Gesture::Ellipse { origin, end }) => {
                let raw = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                *end = if modifiers.shift {
                    constrain_square(raw)
                } else {
                    raw
                };
            }
            Some(Gesture::Move {
                start,
                offset,
                moved,
                ..
            }) => {
                let delta = Point {
                    x: root.x - start.x,
                    y: root.y - start.y,
                };
                if !*moved
                    && (delta.x.abs() > DRAG_THRESHOLD_PX || delta.y.abs() > DRAG_THRESHOLD_PX)
                {
                    *moved = true;
                }
                if *moved {
                    *offset = delta;
                }
            }
            Some(Gesture::Erase { .. }) => {
                let last = match &self.gesture {
                    Some(Gesture::Erase { last, .. }) => *last,
                    _ => return,
                };
                let hits = self.erase_hits(last, root);
                if let Some(Gesture::Erase { hidden, last }) = &mut self.gesture {
                    for id in hits {
                        if !hidden.contains(&id) {
                            hidden.push(id);
                        }
                    }
                    *last = root;
                }
            }
            None => {}
        }
        self.mark_transient();
    }

    fn pointer_up(&mut self, root: Point, modifiers: Modifiers) {
        self.mark_transient();
        self.mark_selection();
        match self.gesture.take() {
            Some(Gesture::Pen { origin, mut points }) => {
                let local = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                if points.last() != Some(&local) {
                    points.push(local);
                }
                let width_px = self.width_px();
                let color = self.color();
                self.document
                    .insert(origin, color, ObjectData::Pen { points, width_px });
                self.mark_last_object();
            }
            Some(Gesture::Line { origin, .. }) => {
                let end = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                let width_px = self.width_px();
                let color = self.color();
                self.document
                    .insert(origin, color, ObjectData::Line { end, width_px });
                self.mark_last_object();
            }
            Some(Gesture::Rect { origin, .. }) => {
                let end = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                if end == Point::default() {
                    return;
                }
                let width_px = self.width_px();
                let color = self.color();
                self.document
                    .insert(origin, color, ObjectData::Rect { end, width_px });
                self.mark_last_object();
            }
            Some(Gesture::Ellipse { origin, .. }) => {
                let raw = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                let end = if modifiers.shift {
                    constrain_square(raw)
                } else {
                    raw
                };
                if end == Point::default() {
                    return;
                }
                let width_px = self.width_px();
                let color = self.color();
                self.document
                    .insert(origin, color, ObjectData::Ellipse { end, width_px });
                self.mark_last_object();
            }
            Some(Gesture::Arrow { origin, .. }) => {
                let end = Point {
                    x: root.x - origin.x,
                    y: root.y - origin.y,
                };
                if end == Point::default() {
                    return;
                }
                let width_px = self.width_px();
                let color = self.color();
                self.document
                    .insert(origin, color, ObjectData::Arrow { end, width_px });
                self.mark_last_object();
            }
            Some(Gesture::Move {
                id, start, moved, ..
            }) => {
                let delta = Point {
                    x: root.x - start.x,
                    y: root.y - start.y,
                };
                let dragged =
                    moved || delta.x.abs() > DRAG_THRESHOLD_PX || delta.y.abs() > DRAG_THRESHOLD_PX;
                if dragged {
                    self.document.move_by(id, delta);
                }
                self.mark_selection();
            }
            Some(Gesture::Erase {
                mut hidden, last, ..
            }) => {
                for id in self.erase_hits(last, root) {
                    if !hidden.contains(&id) {
                        hidden.push(id);
                    }
                }
                if !hidden.is_empty() {
                    self.document.delete(&hidden);
                }
            }
            None => {}
        }
    }

    fn select_down(&mut self, root: Point, time: Duration) {
        let Some(id) = self.hit_object(root) else {
            self.selection = None;
            self.last_click = None;
            return;
        };
        self.selection = Some(id);
        let double = self.register_click(id, time, root);
        if double && self.is_text(id) {
            self.open_text(id);
            return;
        }
        self.gesture = Some(Gesture::Move {
            id,
            start: root,
            offset: Point::default(),
            moved: false,
        });
    }

    fn text_down(&mut self, root: Point, time: Duration) {
        if let Some(id) = self.hit_object(root) {
            self.selection = Some(id);
            let double = self.register_click(id, time, root);
            if double && self.is_text(id) {
                self.open_text(id);
            }
            return;
        }
        self.selection = None;
        self.last_click = None;
        self.start_text_draft(root);
    }

    fn register_click(&mut self, id: ObjectId, time: Duration, root: Point) -> bool {
        let double = match self.last_click {
            Some((last_id, last_time, last_point)) => {
                last_id == id
                    && time.saturating_sub(last_time) <= DOUBLE_CLICK_INTERVAL
                    && (root.x - last_point.x).abs() <= DOUBLE_CLICK_SLOP_PX
                    && (root.y - last_point.y).abs() <= DOUBLE_CLICK_SLOP_PX
            }
            None => false,
        };
        self.last_click = if double { None } else { Some((id, time, root)) };
        double
    }

    fn start_text_draft(&mut self, origin: Point) {
        let size_px = self.text_size_px();
        let draft = TextDraft::new(
            self.next_draft_id,
            "",
            DEFAULT_TEXT_FAMILY,
            size_px,
            &mut self.text_system,
        );
        self.next_draft_id += 1;
        self.gesture = None;
        self.begin_text_edit(TextEdit {
            id: draft.id(),
            object: None,
            origin,
            color: self.color(),
            draft,
        });
    }

    fn open_text(&mut self, id: ObjectId) {
        let Some((data, origin, color)) = self
            .document
            .objects()
            .iter()
            .find(|object| object.id == id)
            .map(|object| (object.data.clone(), object.origin, object.color))
        else {
            return;
        };
        let ObjectData::Text {
            text,
            family,
            size_px,
        } = data
        else {
            return;
        };
        self.mark_all();
        let draft = TextDraft::new(
            self.next_draft_id,
            &text,
            &family,
            size_px,
            &mut self.text_system,
        );
        self.next_draft_id += 1;
        self.gesture = None;
        self.selection = Some(id);
        self.begin_text_edit(TextEdit {
            id: draft.id(),
            object: Some(id),
            origin,
            color,
            draft,
        });
        self.mark_transient();
    }

    fn commit_text(&mut self) {
        self.mark_transient();
        self.clear_pastes();
        let Some(edit) = self.text_edit.take() else {
            return;
        };
        let text = edit.draft.plain_text();
        let family = edit.draft.family().to_owned();
        let size_px = edit.draft.size_px();
        let data = ObjectData::Text {
            text: text.clone(),
            family,
            size_px,
        };
        match edit.object {
            Some(id) => {
                if text.is_empty() {
                    self.document.delete(&[id]);
                    self.selection = None;
                } else if self.document.replace(id, edit.origin, edit.color, data) {
                    self.selection = Some(id);
                }
            }
            None => {
                if !text.is_empty() {
                    let id = self.document.insert(edit.origin, edit.color, data);
                    self.selection = Some(id);
                }
            }
        }
        self.mark_all();
        self.mark_selection();
        self.mark_transient();
    }

    fn cancel_text(&mut self) {
        self.mark_edited_object();
        self.mark_transient();
        self.mark_selection();
        self.text_edit = None;
        self.clear_pastes();
    }

    fn begin_text_edit(&mut self, edit: TextEdit) {
        self.paste_requests.clear();
        self.pending_pastes.clear();
        self.next_paste_sequence = 1;
        self.next_paste_expected = 1;
        self.text_edit = Some(edit);
    }

    fn clear_pastes(&mut self) {
        self.paste_requests.clear();
        self.pending_pastes.clear();
    }

    fn mark_edited_object(&mut self) {
        let id = self.text_edit.as_ref().and_then(|edit| edit.object);
        if let Some(id) = id
            && let Some(object) = self
                .document
                .objects()
                .iter()
                .find(|object| object.id == id)
                .cloned()
        {
            self.mark_object(&object);
        }
    }

    fn is_text(&self, id: ObjectId) -> bool {
        self.document
            .objects()
            .iter()
            .any(|object| object.id == id && matches!(object.data, ObjectData::Text { .. }))
    }

    fn hit_object(&mut self, point: Point) -> Option<ObjectId> {
        let text_system = &mut self.text_system;
        self.document
            .objects()
            .iter()
            .rev()
            .find(|object| object_hit(text_system, object, point))
            .map(|object| object.id)
    }

    fn erase_hits(&mut self, start: Point, end: Point) -> Vec<ObjectId> {
        let text_system = &mut self.text_system;
        self.document
            .objects()
            .iter()
            .filter(|object| object_touched(text_system, object, start, end))
            .map(|object| object.id)
            .collect()
    }

    /// Take the union of paint bounds changed since the last redraw.
    pub fn take_damage(&mut self) -> Option<Rect> {
        self.damage.take()
    }

    fn mark(&mut self, bounds: Rect) {
        if bounds.width == 0 && bounds.height == 0 {
            return;
        }
        self.damage = Some(match self.damage {
            Some(existing) => existing.union(&bounds),
            None => bounds,
        });
    }

    fn mark_object(&mut self, object: &Object) {
        let bounds = crate::render::object_paint_bounds(&mut self.text_system, object);
        self.mark(bounds);
    }

    fn mark_last_object(&mut self) {
        if let Some(object) = self.document.objects().last().cloned() {
            self.mark_object(&object);
        }
    }

    fn mark_all(&mut self) {
        let objects = self.document.objects().to_vec();
        for object in &objects {
            self.mark_object(object);
        }
    }

    fn mark_selection(&mut self) {
        if let Some(id) = self.selection
            && let Some(object) = self
                .document
                .objects()
                .iter()
                .find(|object| object.id == id)
                .cloned()
        {
            let bounds =
                crate::render::object_paint_bounds(&mut self.text_system, &object).padded(2);
            self.mark(bounds);
        }
    }

    fn transient_bounds(&mut self) -> Option<Rect> {
        let mut bounds = match self.gesture.clone() {
            Some(Gesture::Pen { origin, points }) => Some(crate::render::stroke_bounds(
                origin,
                &points,
                self.width_px(),
            )),
            Some(Gesture::Line { origin, end }) => Some(crate::render::stroke_bounds(
                origin,
                &[Point::default(), end],
                self.width_px(),
            )),
            Some(Gesture::Rect { origin, end }) => Some(crate::render::stroke_bounds(
                origin,
                &rect_outline(end),
                self.width_px(),
            )),
            Some(Gesture::Ellipse { origin, end }) => Some(crate::render::stroke_bounds(
                origin,
                &ellipse_outline(end),
                self.width_px(),
            )),
            Some(Gesture::Arrow { origin, end }) => Some(crate::render::stroke_bounds(
                origin,
                &arrow_outline(end, self.width_px()),
                self.width_px(),
            )),
            Some(Gesture::Move { id, offset, .. }) => {
                let object = self
                    .document
                    .objects()
                    .iter()
                    .find(|object| object.id == id)
                    .cloned();
                object.map(|mut object| {
                    object.origin.x += offset.x;
                    object.origin.y += offset.y;
                    crate::render::object_paint_bounds(&mut self.text_system, &object)
                })
            }
            Some(Gesture::Erase { hidden, .. }) => {
                let objects: Vec<Object> = self
                    .document
                    .objects()
                    .iter()
                    .filter(|object| hidden.contains(&object.id))
                    .cloned()
                    .collect();
                objects
                    .iter()
                    .map(|object| crate::render::object_paint_bounds(&mut self.text_system, object))
                    .reduce(|left, right| left.union(&right))
            }
            None => None,
        };
        let draft_bounds = self.text_edit.as_mut().map(|edit| {
            edit.draft
                .bounds(edit.origin, &mut self.text_system)
                .padded(TEXT_BOX_PADDING_PX)
                .padded(2)
        });
        bounds = match (bounds, draft_bounds) {
            (Some(left), Some(right)) => Some(left.union(&right)),
            (left, right) => left.or(right),
        };
        bounds
    }

    fn mark_transient(&mut self) {
        if let Some(bounds) = self.transient_bounds() {
            self.mark(bounds);
        }
    }
}

fn object_hit(text_system: &mut TextSystem, object: &Object, point: Point) -> bool {
    match &object.data {
        ObjectData::Pen { points, width_px } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            if points.len() <= 1 {
                let local = points.first().copied().unwrap_or_default();
                let absolute = object.point(local);
                let dx = (point.x - absolute.x) as f64;
                let dy = (point.y - absolute.y) as f64;
                return dx.hypot(dy) <= tolerance;
            }
            points.windows(2).any(|pair| {
                point_segment_distance(point, object.point(pair[0]), object.point(pair[1]))
                    <= tolerance
            })
        }
        ObjectData::Line { end, width_px } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            point_segment_distance(point, object.origin, object.point(*end)) <= tolerance
        }
        ObjectData::Rect { end, width_px } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            rect_outline(*end).windows(2).any(|pair| {
                point_segment_distance(point, object.point(pair[0]), object.point(pair[1]))
                    <= tolerance
            })
        }
        ObjectData::Ellipse { end, width_px } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            ellipse_outline(*end).windows(2).any(|pair| {
                point_segment_distance(point, object.point(pair[0]), object.point(pair[1]))
                    <= tolerance
            })
        }
        ObjectData::Arrow { end, width_px } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            let [tail, tip, barb_a, barb_b] = arrow_outline(*end, *width_px);
            [(tail, tip), (tip, barb_a), (tip, barb_b)]
                .into_iter()
                .any(|(start, end)| {
                    point_segment_distance(point, object.point(start), object.point(end))
                        <= tolerance
                })
        }
        ObjectData::Text { .. } => text_system.bounds(object).contains(point),
    }
}

fn object_touched(text_system: &mut TextSystem, object: &Object, start: Point, end: Point) -> bool {
    match &object.data {
        ObjectData::Pen { points, width_px } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            if points.len() <= 1 {
                let local = points.first().copied().unwrap_or_default();
                let absolute = object.point(local);
                return segment_segment_distance(absolute, absolute, start, end) <= tolerance;
            }
            points.windows(2).any(|pair| {
                segment_segment_distance(object.point(pair[0]), object.point(pair[1]), start, end)
                    <= tolerance
            })
        }
        ObjectData::Line {
            end: line_end,
            width_px,
        } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            segment_segment_distance(object.origin, object.point(*line_end), start, end)
                <= tolerance
        }
        ObjectData::Rect {
            end: rect_end,
            width_px,
        } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            rect_outline(*rect_end).windows(2).any(|pair| {
                segment_segment_distance(object.point(pair[0]), object.point(pair[1]), start, end)
                    <= tolerance
            })
        }
        ObjectData::Ellipse {
            end: ellipse_end,
            width_px,
        } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            ellipse_outline(*ellipse_end).windows(2).any(|pair| {
                segment_segment_distance(object.point(pair[0]), object.point(pair[1]), start, end)
                    <= tolerance
            })
        }
        ObjectData::Arrow {
            end: arrow_end,
            width_px,
        } => {
            let tolerance = *width_px as f64 / 2.0 + HIT_TOLERANCE_PX;
            let [tail, tip, barb_a, barb_b] = arrow_outline(*arrow_end, *width_px);
            [(tail, tip), (tip, barb_a), (tip, barb_b)].into_iter().any(
                |(segment_start, segment_end)| {
                    segment_segment_distance(
                        object.point(segment_start),
                        object.point(segment_end),
                        start,
                        end,
                    ) <= tolerance
                },
            )
        }
        ObjectData::Text { .. } => {
            let bounds = text_system.bounds(object);
            segment_rect_distance(start, end, &bounds) == 0.0
                || point_rect_distance(start, &bounds) == 0.0
        }
    }
}

fn resolve_preset<T: Copy + PartialEq>(
    preset: Preset<T>,
    values: &[T],
    index: usize,
) -> Result<(T, usize), String> {
    let next_index = match preset {
        Preset::Value(value) => values
            .iter()
            .position(|candidate| *candidate == value)
            .ok_or_else(|| "value is not in the configured catalog".to_owned())?,
        Preset::Next => cycle_index(index, values.len(), true),
        Preset::Prev => cycle_index(index, values.len(), false),
    };
    Ok((values[next_index], next_index))
}

/// Shared default annotation bindings for tests that only exercise canvas
/// behavior.
#[cfg(test)]
pub(crate) fn default_bindings() -> Vec<Binding> {
    rpresent_config::shortcuts::ShortcutConfig::default()
        .validate()
        .expect("default shortcuts are valid")
        .annotation
}

/// Box for a Shift-constrained ellipse drag. The longer axis decides the
/// side, each axis keeps its direction, and a zero axis counts as positive
/// so a straight horizontal or vertical drag still becomes a square.
fn constrain_square(end: Point) -> Point {
    let side = end.x.abs().max(end.y.abs());
    Point {
        x: if end.x < 0 { -side } else { side },
        y: if end.y < 0 { -side } else { side },
    }
}

fn lower_letter(key: Key) -> Option<char> {
    match key {
        Key::Char(character) if character.is_ascii_alphabetic() => {
            Some(character.to_ascii_lowercase())
        }
        _ => None,
    }
}

#[cfg(test)]
mod toolbar_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> AnnotationConfig {
        AnnotationConfig::default()
    }

    fn ready_session() -> Session {
        let mut session = Session::new(&config(), &crate::session::default_bindings()).unwrap();
        session.command(AnnotationCommand::On).unwrap();
        session.ownership(Some(1));
        session
    }

    fn pointer(session: &mut Session, input: PointerInput) {
        session.pointer(1, input);
    }

    fn drag(session: &mut Session, from: Point, to: Point) {
        pointer(
            session,
            PointerInput::Down {
                root: from,
                time: Duration::ZERO,
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            session,
            PointerInput::Move {
                root: to,
                time: Duration::from_millis(10),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            session,
            PointerInput::Up {
                root: to,
                time: Duration::from_millis(20),
                modifiers: crate::Modifiers::default(),
            },
        );
    }

    fn drag_with(session: &mut Session, from: Point, to: Point, modifiers: Modifiers) {
        pointer(
            session,
            PointerInput::Down {
                root: from,
                time: Duration::ZERO,
                modifiers: Modifiers::default(),
            },
        );
        pointer(
            session,
            PointerInput::Move {
                root: to,
                time: Duration::from_millis(10),
                modifiers,
            },
        );
        pointer(
            session,
            PointerInput::Up {
                root: to,
                time: Duration::from_millis(20),
                modifiers,
            },
        );
    }

    #[test]
    fn shift_squares_the_ellipse_drag_box() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Ellipse))
            .unwrap();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        drag_with(
            &mut session,
            Point { x: 20, y: 20 },
            Point { x: 120, y: 70 },
            shift,
        );
        assert_eq!(
            session.document().objects()[0].data,
            ObjectData::Ellipse {
                end: Point { x: 100, y: 100 },
                width_px: 6,
            }
        );
    }

    #[test]
    fn shift_can_toggle_during_the_drag() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Ellipse))
            .unwrap();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        let free = Modifiers::default();
        pointer(
            &mut session,
            PointerInput::Down {
                root: Point { x: 20, y: 20 },
                time: Duration::ZERO,
                modifiers: free,
            },
        );
        pointer(
            &mut session,
            PointerInput::Move {
                root: Point { x: 120, y: 70 },
                time: Duration::from_millis(5),
                modifiers: free,
            },
        );
        assert_eq!(
            session.gesture,
            Some(Gesture::Ellipse {
                origin: Point { x: 20, y: 20 },
                end: Point { x: 100, y: 50 },
            }),
            "free drag stores the raw box"
        );
        pointer(
            &mut session,
            PointerInput::Move {
                root: Point { x: 130, y: 80 },
                time: Duration::from_millis(10),
                modifiers: shift,
            },
        );
        assert_eq!(
            session.gesture,
            Some(Gesture::Ellipse {
                origin: Point { x: 20, y: 20 },
                end: Point { x: 110, y: 110 },
            }),
            "pressing Shift mid-drag resquares the box"
        );
        pointer(
            &mut session,
            PointerInput::Up {
                root: Point { x: 130, y: 80 },
                time: Duration::from_millis(15),
                modifiers: free,
            },
        );
        assert_eq!(
            session.document().objects()[0].data,
            ObjectData::Ellipse {
                end: Point { x: 110, y: 60 },
                width_px: 6,
            },
            "releasing Shift before the release frees the box"
        );
    }

    fn click(session: &mut Session, at: Point, time: Duration) {
        pointer(
            session,
            PointerInput::Down {
                root: at,
                time,
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            session,
            PointerInput::Up {
                root: at,
                time: time + Duration::from_millis(10),
                modifiers: crate::Modifiers::default(),
            },
        );
    }

    fn ellipse_object(end: Point) -> ObjectData {
        ObjectData::Ellipse { end, width_px: 4 }
    }

    #[test]
    fn ellipse_drag_stores_the_drag_box() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Ellipse))
            .unwrap();
        drag(
            &mut session,
            Point { x: 20, y: 20 },
            Point { x: 100, y: 70 },
        );
        let objects = session.document().objects();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].origin, Point { x: 20, y: 20 });
        assert_eq!(
            objects[0].data,
            ObjectData::Ellipse {
                end: Point { x: 80, y: 50 },
                width_px: 6,
            }
        );
    }

    #[test]
    fn degenerate_ellipse_drag_creates_nothing() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Ellipse))
            .unwrap();
        click(
            &mut session,
            Point { x: 30, y: 30 },
            Duration::from_millis(0),
        );
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn ellipse_outline_is_selectable_but_the_interior_is_not() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        session.document.insert(
            Point { x: 20, y: 20 },
            [255, 0, 0],
            ellipse_object(Point { x: 60, y: 60 }),
        );

        click(
            &mut session,
            Point { x: 50, y: 50 },
            Duration::from_millis(0),
        );
        assert_eq!(session.selection(), None, "interior misses");

        click(
            &mut session,
            Point { x: 50, y: 20 },
            Duration::from_millis(100),
        );
        assert!(session.selection().is_some(), "outline hits");
    }

    #[test]
    fn eraser_skips_a_drag_that_stays_inside_an_ellipse() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        session.document.insert(
            Point { x: 20, y: 20 },
            [255, 0, 0],
            ellipse_object(Point { x: 60, y: 60 }),
        );

        drag(&mut session, Point { x: 40, y: 50 }, Point { x: 60, y: 55 });
        assert_eq!(
            session.document().objects().len(),
            1,
            "interior drag misses"
        );

        drag(&mut session, Point { x: 10, y: 50 }, Point { x: 90, y: 50 });
        assert!(
            session.document().objects().is_empty(),
            "crossing drag hits"
        );
    }

    fn shortcut_mods(modifiers: Modifiers) -> rpresent_config::shortcuts::Mods {
        use rpresent_config::shortcuts::Mods;
        let mut mods = Mods::NONE;
        for (active, flag) in [
            (modifiers.shift, Mods::SHIFT),
            (modifiers.ctrl, Mods::CTRL),
            (modifiers.alt, Mods::ALT),
            (modifiers.super_key, Mods::SUPER),
        ] {
            if active {
                mods |= flag;
            }
        }
        mods
    }

    fn key(session: &mut Session, kind: Key, text: Option<char>, modifiers: Modifiers) {
        session.key(
            1,
            KeyInput {
                key: kind,
                text,
                modifiers,
                shortcut: Some(ShortcutInput::new(kind, shortcut_mods(modifiers))),
            },
        );
    }

    fn press(session: &mut Session, expression: &str) {
        let shortcut = expression
            .parse::<ShortcutInput>()
            .expect("test shortcut parses");
        session.key(
            1,
            KeyInput {
                key: shortcut.key(),
                text: None,
                modifiers: Modifiers::default(),
                shortcut: Some(shortcut),
            },
        );
    }

    fn ready_session_with_bindings(bindings: &[Binding]) -> Session {
        let mut session = Session::new(&config(), bindings).unwrap();
        session.command(AnnotationCommand::On).unwrap();
        session.ownership(Some(1));
        session
    }

    fn binding_for(bindings: &[Binding], action: ShortcutAction) -> &Binding {
        bindings
            .iter()
            .find(|binding| binding.action == action)
            .unwrap_or_else(|| panic!("no binding for {}", action.name()))
    }

    fn type_text(session: &mut Session, text: &str) {
        for character in text.chars() {
            key(
                session,
                Key::Char(character),
                Some(character),
                Modifiers::default(),
            );
        }
    }

    fn press_enter(session: &mut Session) {
        key(session, Key::Enter, None, Modifiers::default());
    }

    fn text_bounds(session: &mut Session, id: ObjectId) -> Rect {
        let object = session
            .document()
            .objects()
            .iter()
            .find(|object| object.id == id)
            .unwrap()
            .clone();
        session.text_system.bounds(&object)
    }

    #[test]
    fn toolbar_values_borrow_catalogs_and_follow_commands() {
        let mut session = Session::new(
            &AnnotationConfig::default(),
            &crate::session::default_bindings(),
        )
        .unwrap();
        let values = session.toolbar_values();
        assert_eq!(values.colors, session.colors.as_slice());
        assert_eq!(values.colors.as_ptr(), session.colors.as_ptr());
        assert_eq!(values.widths.as_ptr(), session.widths.as_ptr());
        assert_eq!(values.text_sizes.as_ptr(), session.text_sizes.as_ptr());
        assert!(!values.can_undo);
        assert!(!values.can_redo);
        assert!(!values.has_selection);
        assert!(!values.has_content);
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        assert_eq!(session.toolbar_values().tool, AnnotationTool::Text);
    }

    #[test]
    fn off_keeps_committed_objects_and_rejects_stale_input() {
        let mut session = Session::new(&config(), &crate::session::default_bindings()).unwrap();
        session.command(AnnotationCommand::On).unwrap();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Pen))
            .unwrap();
        session.ownership(Some(7));
        let root = Point { x: 10, y: 20 };
        session.pointer(
            7,
            PointerInput::Down {
                root,
                time: Duration::ZERO,
                modifiers: crate::Modifiers::default(),
            },
        );
        session.pointer(
            7,
            PointerInput::Up {
                root,
                time: Duration::from_millis(20),
                modifiers: crate::Modifiers::default(),
            },
        );
        session.command(AnnotationCommand::Off).unwrap();
        assert!(session.status().has_content);
        assert!(!session.status().editing_requested);
        session.pointer(
            7,
            PointerInput::Down {
                root,
                time: Duration::from_millis(30),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert_eq!(session.document().objects().len(), 1);
    }

    #[test]
    fn pen_click_creates_a_dot() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Pen))
            .unwrap();
        assert_eq!(session.tool(), AnnotationTool::Pen);
        click(&mut session, Point { x: 5, y: 6 }, Duration::ZERO);
        let objects = session.document().objects();
        assert_eq!(objects.len(), 1);
        match &objects[0].data {
            ObjectData::Pen { points, width_px } => {
                assert_eq!(points, &vec![Point::default()]);
                assert_eq!(*width_px, 6);
            }
            other => panic!("unexpected object {other:?}"),
        }
    }

    #[test]
    fn cancelled_move_restores_the_original_position() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, Point { x: 10, y: 20 }, Point { x: 70, y: 20 });
        let id = session.document().objects()[0].id;
        let origin = session.document().objects()[0].origin;

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        pointer(
            &mut session,
            PointerInput::Down {
                root: Point { x: 40, y: 20 },
                time: Duration::from_millis(100),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            &mut session,
            PointerInput::Move {
                root: Point { x: 55, y: 20 },
                time: Duration::from_millis(120),
                modifiers: crate::Modifiers::default(),
            },
        );
        key(&mut session, Key::Esc, None, Modifiers::default());
        assert_eq!(session.document().objects()[0].id, id);
        assert_eq!(session.document().objects()[0].origin, origin);
    }

    #[test]
    fn up_applies_the_final_move_without_a_move_event() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, Point { x: 10, y: 20 }, Point { x: 70, y: 20 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        pointer(
            &mut session,
            PointerInput::Down {
                root: Point { x: 40, y: 20 },
                time: Duration::from_millis(100),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            &mut session,
            PointerInput::Up {
                root: Point { x: 60, y: 40 },
                time: Duration::from_millis(180),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert_eq!(
            session.document().objects()[0].origin,
            Point { x: 30, y: 40 }
        );
    }

    #[test]
    fn grouped_erase_is_one_reversible_operation() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, Point { x: 10, y: 20 }, Point { x: 60, y: 20 });
        drag(&mut session, Point { x: 10, y: 40 }, Point { x: 60, y: 40 });

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        drag(&mut session, Point { x: 30, y: 5 }, Point { x: 30, y: 55 });
        assert!(session.document().objects().is_empty());

        session.command(AnnotationCommand::Undo).unwrap();
        assert_eq!(session.document().objects().len(), 2);
    }

    #[test]
    fn fast_erase_crossing_a_stroke_uses_the_release_segment() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, Point { x: 10, y: 20 }, Point { x: 80, y: 20 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        pointer(
            &mut session,
            PointerInput::Down {
                root: Point { x: 40, y: 0 },
                time: Duration::from_millis(100),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            &mut session,
            PointerInput::Up {
                root: Point { x: 40, y: 60 },
                time: Duration::from_millis(110),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn text_can_be_reopened_by_double_click_without_duplication() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 50, y: 60 }, Duration::ZERO);
        type_text(&mut session, "Hi");
        press_enter(&mut session);
        assert_eq!(session.document().objects().len(), 1);
        let id = session.document().objects()[0].id;
        let bounds = text_bounds(&mut session, id);
        let target = Point {
            x: bounds.x + 2,
            y: bounds.y + 2,
        };

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        click(&mut session, target, Duration::from_millis(1000));
        click(&mut session, target, Duration::from_millis(1200));
        assert!(session.status().text_editing);
        type_text(&mut session, "!");
        press_enter(&mut session);
        assert_eq!(session.document().objects().len(), 1);
        match &session.document().objects()[0].data {
            ObjectData::Text { text, .. } => assert_eq!(text, "Hi!"),
            other => panic!("unexpected object {other:?}"),
        }
    }

    fn ctrl_v(session: &mut Session) {
        key(
            session,
            Key::Char('v'),
            Some('v'),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
    }

    #[test]
    fn paste_is_inserted_at_the_current_caret_and_normalizes_newlines() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        type_text(&mut session, "Hi");
        key(&mut session, Key::Left, None, Modifiers::default());
        ctrl_v(&mut session);
        let requests = session.take_paste_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].session, session.draft_id().unwrap());
        session.accept_paste(PasteResult {
            request: requests[0],
            text: Ok(" there\r\n".to_owned()),
        });
        assert_eq!(
            session.text_edit.as_ref().unwrap().draft.plain_text(),
            "H there\ni"
        );
    }

    #[test]
    fn out_of_order_paste_results_wait_for_their_sequence() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        type_text(&mut session, "A");
        ctrl_v(&mut session);
        ctrl_v(&mut session);
        let requests = session.take_paste_requests();
        assert_eq!(requests.len(), 2);
        session.accept_paste(PasteResult {
            request: requests[1],
            text: Ok("C".to_owned()),
        });
        assert_eq!(session.text_edit.as_ref().unwrap().draft.plain_text(), "A");
        session.accept_paste(PasteResult {
            request: requests[0],
            text: Ok("B".to_owned()),
        });
        assert_eq!(
            session.text_edit.as_ref().unwrap().draft.plain_text(),
            "ABC"
        );
    }

    #[test]
    fn paste_from_an_ended_draft_is_ignored() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        ctrl_v(&mut session);
        let requests = session.take_paste_requests();
        let old = requests[0];
        key(&mut session, Key::Esc, None, Modifiers::default());
        assert!(session.draft_id().is_none());
        session.accept_paste(PasteResult {
            request: old,
            text: Ok("late".to_owned()),
        });
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn failed_paste_leaves_the_draft_unchanged() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        type_text(&mut session, "Keep");
        ctrl_v(&mut session);
        let request = session.take_paste_requests()[0];
        session.accept_paste(PasteResult {
            request,
            text: Err("clipboard request timed out".to_owned()),
        });
        assert_eq!(
            session.text_edit.as_ref().unwrap().draft.plain_text(),
            "Keep"
        );
    }

    #[test]
    fn committing_empty_replacement_deletes_the_text_object() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 30, y: 30 }, Duration::ZERO);
        type_text(&mut session, "Hello");
        press_enter(&mut session);
        let id = session.document().objects()[0].id;
        let bounds = text_bounds(&mut session, id);

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(
            &mut session,
            Point {
                x: bounds.x + 1,
                y: bounds.y + 1,
            },
            Duration::from_millis(1000),
        );
        click(
            &mut session,
            Point {
                x: bounds.x + 1,
                y: bounds.y + 1,
            },
            Duration::from_millis(1200),
        );
        assert!(session.status().text_editing);
        key(
            &mut session,
            Key::Char('a'),
            Some('a'),
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        );
        key(&mut session, Key::Backspace, None, Modifiers::default());
        press_enter(&mut session);
        assert!(session.document().objects().is_empty());
        session.command(AnnotationCommand::Undo).unwrap();
        assert_eq!(session.document().objects().len(), 1);
    }

    #[test]
    fn unchanged_text_commit_creates_no_history_entry() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        type_text(&mut session, "Same");
        press_enter(&mut session);
        let id = session.document().objects()[0].id;
        let bounds = text_bounds(&mut session, id);
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        let target = Point {
            x: bounds.x + 1,
            y: bounds.y + 1,
        };
        click(&mut session, target, Duration::from_millis(1000));
        click(&mut session, target, Duration::from_millis(1200));
        press_enter(&mut session);
        session.command(AnnotationCommand::Undo).unwrap();
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn escape_cancels_a_draft_before_clearing_selection() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 10, y: 10 }, Duration::ZERO);
        type_text(&mut session, "draft");
        key(&mut session, Key::Esc, None, Modifiers::default());
        assert!(!session.status().text_editing);
        assert!(session.status().editing_requested);
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn presets_cycle_and_reject_values_outside_the_catalog() {
        let mut session = ready_session();
        assert_eq!(session.color(), [255, 0, 0]);
        session
            .command(AnnotationCommand::Color(Preset::Next))
            .unwrap();
        assert_eq!(session.color(), [255, 255, 0]);
        session
            .command(AnnotationCommand::Color(Preset::Prev))
            .unwrap();
        assert_eq!(session.color(), [255, 0, 0]);
        assert!(
            session
                .command(AnnotationCommand::Width(Preset::Value(7)))
                .is_err()
        );
        assert_eq!(session.width_px(), 6);
        assert!(
            session
                .command(AnnotationCommand::TextSize(Preset::Value(0)))
                .is_err()
        );
        assert_eq!(session.text_size_px(), 24);
        session
            .command(AnnotationCommand::Width(Preset::Value(10)))
            .unwrap();
        assert_eq!(session.width_px(), 10);
    }

    #[test]
    fn preset_changes_cancel_a_draft_after_resolving_the_value() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 10, y: 10 }, Duration::ZERO);
        type_text(&mut session, "abc");
        assert!(session.status().text_editing);
        assert!(
            session
                .command(AnnotationCommand::Color(Preset::Value([1, 2, 3])))
                .is_err()
        );
        assert!(session.status().text_editing);
        session
            .command(AnnotationCommand::Width(Preset::Value(2)))
            .unwrap();
        assert!(!session.status().text_editing);
        assert_eq!(session.width_px(), 2);
    }

    #[test]
    fn object_styles_are_copied_at_creation_time() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Width(Preset::Value(2)))
            .unwrap();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, Point { x: 0, y: 0 }, Point { x: 10, y: 0 });
        session
            .command(AnnotationCommand::Width(Preset::Value(10)))
            .unwrap();
        drag(&mut session, Point { x: 0, y: 10 }, Point { x: 10, y: 10 });
        let objects = session.document().objects();
        match (&objects[0].data, &objects[1].data) {
            (
                ObjectData::Line {
                    width_px: first, ..
                },
                ObjectData::Line {
                    width_px: second, ..
                },
            ) => {
                assert_eq!(*first, 2);
                assert_eq!(*second, 10);
            }
            other => panic!("unexpected objects {other:?}"),
        }
    }

    #[test]
    fn delete_removes_the_selected_object_in_one_operation() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, Point { x: 0, y: 0 }, Point { x: 50, y: 0 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        click(
            &mut session,
            Point { x: 25, y: 0 },
            Duration::from_millis(500),
        );
        assert!(session.selection().is_some());
        session.command(AnnotationCommand::Delete).unwrap();
        assert!(session.document().objects().is_empty());
        session.command(AnnotationCommand::Undo).unwrap();
        assert_eq!(session.document().objects().len(), 1);
    }

    #[test]
    fn stale_epochs_cannot_edit_the_document() {
        let mut session = ready_session();
        session.pointer(
            99,
            PointerInput::Down {
                root: Point { x: 1, y: 1 },
                time: Duration::ZERO,
                modifiers: crate::Modifiers::default(),
            },
        );
        session.pointer(
            99,
            PointerInput::Up {
                root: Point { x: 1, y: 1 },
                time: Duration::from_millis(10),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert!(session.document().objects().is_empty());
        session.ownership(None);
        session.pointer(
            1,
            PointerInput::Down {
                root: Point { x: 1, y: 1 },
                time: Duration::from_millis(20),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn shift_enter_inserts_a_newline_and_enter_commits() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 15, y: 15 }, Duration::ZERO);
        type_text(&mut session, "one");
        key(
            &mut session,
            Key::Enter,
            None,
            Modifiers {
                shift: true,
                ..Modifiers::default()
            },
        );
        type_text(&mut session, "two");
        press_enter(&mut session);
        match &session.document().objects()[0].data {
            ObjectData::Text { text, .. } => assert_eq!(text, "one\ntwo"),
            other => panic!("unexpected object {other:?}"),
        }
    }

    #[test]
    fn configured_tool_binding_replaces_the_fixed_letter() {
        let settings = rpresent_config::shortcuts::ShortcutConfig {
            annotation: std::collections::BTreeMap::from([(
                "annotation.tool.line".into(),
                "Ctrl+F8".into(),
            )]),
            ..rpresent_config::shortcuts::ShortcutConfig::default()
        };
        let bindings = settings.validate().unwrap();
        let mut session = ready_session_with_bindings(&bindings.annotation);
        let defaults = default_bindings();
        let fixed_letter = binding_for(&defaults, ShortcutAction::ToolLine);
        press(&mut session, &fixed_letter.expression);
        assert_eq!(session.tool(), AnnotationTool::Arrow);
        let configured = binding_for(&bindings.annotation, ShortcutAction::ToolLine);
        press(&mut session, &configured.expression);
        assert_eq!(session.tool(), AnnotationTool::Line);
    }

    #[test]
    fn tool_letters_remain_text_inside_a_draft() {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 10, y: 10 }, Duration::ZERO);
        type_text(&mut session, "vpletCc[]-=x");
        assert_eq!(session.tool(), AnnotationTool::Text);
        assert!(session.status().text_editing);
        assert_eq!(
            session.text_edit.as_ref().unwrap().draft.plain_text(),
            "vpletCc[]-=x"
        );
    }

    #[test]
    fn default_tool_bindings_select_every_tool() {
        for (expression, tool) in [
            ("v", AnnotationTool::Select),
            ("p", AnnotationTool::Pen),
            ("l", AnnotationTool::Line),
            ("o", AnnotationTool::Ellipse),
            ("e", AnnotationTool::Eraser),
            ("t", AnnotationTool::Text),
        ] {
            let mut session = ready_session();
            press(&mut session, expression);
            assert_eq!(session.tool(), tool, "{expression}");
        }
    }

    #[test]
    fn color_bindings_cycle_in_both_directions() {
        let mut session = ready_session();
        let start = session.color();
        press(&mut session, "c");
        let next = session.color();
        assert_ne!(next, start);
        press(&mut session, "C");
        assert_eq!(session.color(), start);
    }

    #[test]
    fn width_and_text_size_bindings_cycle_in_both_directions() {
        let mut session = ready_session();
        let width = session.width_px();
        press(&mut session, "]");
        assert_ne!(session.width_px(), width);
        press(&mut session, "[");
        assert_eq!(session.width_px(), width);

        let size = session.text_size_px();
        press(&mut session, "=");
        assert_ne!(session.text_size_px(), size);
        press(&mut session, "-");
        assert_eq!(session.text_size_px(), size);
    }

    #[test]
    fn default_clear_binding_erases_and_undo_restores() {
        let mut session = ready_session();
        drag(&mut session, Point { x: 0, y: 0 }, Point { x: 50, y: 0 });
        drag(&mut session, Point { x: 0, y: 10 }, Point { x: 50, y: 10 });
        let ids: Vec<ObjectId> = session
            .document()
            .objects()
            .iter()
            .map(|object| object.id)
            .collect();

        press(&mut session, "x");
        assert!(session.document().objects().is_empty());

        session.command(AnnotationCommand::Undo).unwrap();
        let restored: Vec<ObjectId> = session
            .document()
            .objects()
            .iter()
            .map(|object| object.id)
            .collect();
        assert_eq!(restored, ids);
    }

    #[test]
    fn delete_and_clear_bindings_apply_document_commands() {
        let settings = rpresent_config::shortcuts::ShortcutConfig {
            annotation: std::collections::BTreeMap::from([
                ("annotation.delete".into(), "Delete".into()),
                ("annotation.clear".into(), "Ctrl+F9".into()),
            ]),
            ..rpresent_config::shortcuts::ShortcutConfig::default()
        };
        let bindings = settings.validate().unwrap();
        let mut session = ready_session_with_bindings(&bindings.annotation);
        drag(&mut session, Point { x: 0, y: 0 }, Point { x: 50, y: 0 });
        drag(&mut session, Point { x: 0, y: 10 }, Point { x: 50, y: 10 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        click(
            &mut session,
            Point { x: 25, y: 0 },
            Duration::from_millis(500),
        );
        assert!(session.selection().is_some());
        press(&mut session, "Delete");
        assert_eq!(session.document().objects().len(), 1);
        press(&mut session, "Ctrl+F9");
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn document_undo_and_redo_bindings_ignore_ctrl_y() {
        let mut session = ready_session();
        drag(&mut session, Point { x: 0, y: 0 }, Point { x: 50, y: 0 });
        drag(&mut session, Point { x: 0, y: 10 }, Point { x: 50, y: 10 });
        press(&mut session, "Ctrl+z");
        assert_eq!(session.document().objects().len(), 1);
        press(&mut session, "Ctrl+y");
        assert_eq!(session.document().objects().len(), 1);
        press(&mut session, "Ctrl+Z");
        assert_eq!(session.document().objects().len(), 2);
    }

    #[test]
    fn empty_annotation_bindings_disable_canvas_commands_but_keep_text_editing() {
        let mut session = ready_session_with_bindings(&[]);
        press(&mut session, "l");
        press(&mut session, "]");
        assert_eq!(session.tool(), AnnotationTool::Arrow);
        assert_eq!(session.width_px(), config().stroke_width_px);

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, Point { x: 10, y: 10 }, Duration::ZERO);
        type_text(&mut session, "kept");
        assert_eq!(
            session.text_edit.as_ref().unwrap().draft.plain_text(),
            "kept"
        );
        key(&mut session, Key::Esc, None, Modifiers::default());
        assert!(!session.status().text_editing);
        assert!(session.status().editing_requested);
        key(&mut session, Key::Esc, None, Modifiers::default());
        assert!(!session.status().editing_requested);
    }

    fn rect_session() -> Session {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Rect))
            .unwrap();
        session
    }

    fn rect_bounds(session: &mut Session) -> Rect {
        let object = session.document().objects()[0].clone();
        crate::render::object_paint_bounds(&mut session.text_system, &object)
    }

    #[test]
    fn default_r_shortcut_selects_the_rect_tool() {
        let mut session = ready_session();
        key(
            &mut session,
            Key::Char('r'),
            Some('r'),
            Modifiers::default(),
        );
        assert_eq!(session.tool(), AnnotationTool::Rect);
    }

    #[test]
    fn rect_drag_stores_the_active_style_and_corners() {
        let mut session = rect_session();
        let color = session.color();
        let width_px = session.width_px();
        drag(&mut session, Point { x: 10, y: 20 }, Point { x: 70, y: 60 });
        let object = &session.document().objects()[0];
        assert_eq!(object.origin, Point { x: 10, y: 20 });
        assert_eq!(object.color, color);
        match &object.data {
            ObjectData::Rect {
                end,
                width_px: stored,
            } => {
                assert_eq!(*end, Point { x: 60, y: 40 });
                assert_eq!(*stored, width_px);
            }
            other => panic!("unexpected object {other:?}"),
        }
    }

    #[test]
    fn rect_drags_normalize_in_every_direction() {
        let mut bounds = Vec::new();
        for (x, y) in [(70, 60), (30, 60), (30, 40), (70, 40)] {
            let mut session = rect_session();
            drag(&mut session, Point { x: 50, y: 50 }, Point { x, y });
            bounds.push(rect_bounds(&mut session));
        }
        let sizes: Vec<(u32, u32)> = bounds
            .iter()
            .map(|rect| (rect.width, rect.height))
            .collect();
        assert!(
            sizes.windows(2).all(|pair| pair[0] == pair[1]),
            "outline sizes differ: {sizes:?}"
        );
    }

    #[test]
    fn rect_click_without_movement_inserts_nothing() {
        let mut session = rect_session();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        assert!(session.document().objects().is_empty());
        assert!(!session.document().can_undo());
    }

    #[test]
    fn rect_hits_only_its_edges() {
        let mut session = rect_session();
        drag(&mut session, Point { x: 20, y: 20 }, Point { x: 80, y: 60 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        click(
            &mut session,
            Point { x: 50, y: 40 },
            Duration::from_millis(100),
        );
        assert!(session.selection().is_none());
        click(
            &mut session,
            Point { x: 50, y: 20 },
            Duration::from_millis(300),
        );
        assert!(session.selection().is_some());
    }

    #[test]
    fn zero_width_rect_keeps_its_single_edge() {
        let mut session = rect_session();
        drag(&mut session, Point { x: 20, y: 20 }, Point { x: 20, y: 60 });
        assert_eq!(
            session.document().objects()[0].data,
            ObjectData::Rect {
                end: Point { x: 0, y: 40 },
                width_px: session.width_px(),
            }
        );
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        click(
            &mut session,
            Point { x: 20, y: 40 },
            Duration::from_millis(100),
        );
        assert!(session.selection().is_some());
        click(
            &mut session,
            Point { x: 40, y: 40 },
            Duration::from_millis(300),
        );
        assert!(session.selection().is_none());
    }

    #[test]
    fn eraser_drag_across_a_rect_edge_deletes_it() {
        let mut session = rect_session();
        drag(&mut session, Point { x: 20, y: 20 }, Point { x: 80, y: 60 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        drag(&mut session, Point { x: 40, y: 10 }, Point { x: 40, y: 30 });
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn eraser_drag_through_the_interior_leaves_the_rect() {
        let mut session = rect_session();
        drag(&mut session, Point { x: 20, y: 20 }, Point { x: 80, y: 60 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        drag(&mut session, Point { x: 40, y: 35 }, Point { x: 60, y: 45 });
        assert_eq!(session.document().objects().len(), 1);
    }

    #[test]
    fn pressing_a_rect_edge_then_dragging_moves_it() {
        let mut session = rect_session();
        drag(&mut session, Point { x: 20, y: 20 }, Point { x: 80, y: 60 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        pointer(
            &mut session,
            PointerInput::Down {
                root: Point { x: 50, y: 20 },
                time: Duration::from_millis(100),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            &mut session,
            PointerInput::Up {
                root: Point { x: 55, y: 25 },
                time: Duration::from_millis(180),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert_eq!(
            session.document().objects()[0].origin,
            Point { x: 25, y: 25 }
        );
    }

    fn arrow_session() -> Session {
        let mut session = ready_session();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Arrow))
            .unwrap();
        session
    }

    #[test]
    fn arrow_drag_stores_the_active_style_and_tip() {
        let mut session = arrow_session();
        let color = session.color();
        let width_px = session.width_px();
        drag(&mut session, Point { x: 10, y: 20 }, Point { x: 70, y: 60 });
        let object = &session.document().objects()[0];
        assert_eq!(object.origin, Point { x: 10, y: 20 });
        assert_eq!(object.color, color);
        match &object.data {
            ObjectData::Arrow {
                end,
                width_px: stored,
            } => {
                assert_eq!(*end, Point { x: 60, y: 40 });
                assert_eq!(*stored, width_px);
            }
            other => panic!("unexpected object {other:?}"),
        }
    }

    #[test]
    fn arrow_drags_keep_the_direction() {
        let mut forward = arrow_session();
        drag(&mut forward, Point { x: 10, y: 50 }, Point { x: 80, y: 50 });
        let forward_width = forward.width_px();
        assert_eq!(
            forward.document().objects()[0].data,
            ObjectData::Arrow {
                end: Point { x: 70, y: 0 },
                width_px: forward_width,
            }
        );

        let mut backward = arrow_session();
        drag(
            &mut backward,
            Point { x: 80, y: 50 },
            Point { x: 10, y: 50 },
        );
        let backward_width = backward.width_px();
        assert_eq!(
            backward.document().objects()[0].data,
            ObjectData::Arrow {
                end: Point { x: -70, y: 0 },
                width_px: backward_width,
            }
        );
    }

    #[test]
    fn arrow_click_without_movement_inserts_nothing() {
        let mut session = arrow_session();
        click(&mut session, Point { x: 20, y: 20 }, Duration::ZERO);
        assert!(session.document().objects().is_empty());
        assert!(!session.document().can_undo());
    }

    #[test]
    fn one_pixel_arrow_drag_inserts() {
        let mut session = arrow_session();
        drag(&mut session, Point { x: 20, y: 20 }, Point { x: 21, y: 20 });
        assert_eq!(session.document().objects().len(), 1);
    }

    #[test]
    fn arrow_hits_shaft_and_barbs_but_not_the_open_v() {
        let mut session = arrow_session();
        drag(&mut session, Point { x: 20, y: 50 }, Point { x: 80, y: 50 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();

        click(
            &mut session,
            Point { x: 50, y: 50 },
            Duration::from_millis(100),
        );
        assert!(session.selection().is_some(), "shaft");

        click(
            &mut session,
            Point { x: 30, y: 40 },
            Duration::from_millis(300),
        );
        assert!(session.selection().is_none(), "open V");

        let arrow = session.document().objects()[0].clone();
        let [_, _, barb_a, _] = arrow_outline(Point { x: 60, y: 0 }, session.width_px());
        click(
            &mut session,
            arrow.point(barb_a),
            Duration::from_millis(500),
        );
        assert!(session.selection().is_some(), "barb");
    }

    #[test]
    fn eraser_drag_across_an_arrow_barb_deletes_it() {
        let mut session = arrow_session();
        drag(&mut session, Point { x: 20, y: 50 }, Point { x: 90, y: 50 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        drag(&mut session, Point { x: 75, y: 30 }, Point { x: 75, y: 42 });
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn eraser_drag_across_an_arrow_shaft_deletes_it() {
        let mut session = arrow_session();
        drag(&mut session, Point { x: 20, y: 50 }, Point { x: 80, y: 50 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        drag(&mut session, Point { x: 50, y: 40 }, Point { x: 50, y: 60 });
        assert!(session.document().objects().is_empty());
    }

    #[test]
    fn pressing_an_arrow_shaft_then_dragging_moves_it() {
        let mut session = arrow_session();
        drag(&mut session, Point { x: 20, y: 50 }, Point { x: 80, y: 50 });
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        pointer(
            &mut session,
            PointerInput::Down {
                root: Point { x: 50, y: 50 },
                time: Duration::from_millis(100),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer(
            &mut session,
            PointerInput::Up {
                root: Point { x: 55, y: 55 },
                time: Duration::from_millis(180),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert_eq!(
            session.document().objects()[0].origin,
            Point { x: 25, y: 55 }
        );
    }

    #[test]
    fn default_a_shortcut_selects_the_arrow_tool() {
        let mut session = ready_session();
        key(
            &mut session,
            Key::Char('a'),
            Some('a'),
            Modifiers::default(),
        );
        assert_eq!(session.tool(), AnnotationTool::Arrow);
    }
}
