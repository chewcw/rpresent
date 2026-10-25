use crate::{AnnotationCommand, AnnotationTool, Point, PointerInput, Preset, Rect, Rgb};

mod layout;
pub(crate) mod paint;

#[cfg(test)]
mod tests;

/// Row height of every toolbar and picker control.
const ROW_PX: u32 = 36;
/// Padding between the bar edge and its controls.
const PADDING_PX: i32 = 8;
/// Gap between adjacent controls and rows.
const GAP_PX: i32 = 6;
/// Distance from the monitor top for first placement.
const TOP_OFFSET_PX: i32 = 12;
/// Conservative advance width per character of the monospace UI font.
const CHAR_PX: u32 = 9;
/// Side length of a color swatch.
const SWATCH_PX: u32 = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayGeometry {
    pub root: Rect,
    pub monitors: Vec<Rect>,
    pub pointer: Point,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerKind {
    Color,
    Width,
    TextSize,
}

impl PickerKind {
    /// A picker only applies to tools that use its preset.
    pub(crate) fn applies_to(self, tool: AnnotationTool) -> bool {
        match self {
            PickerKind::Color => matches!(
                tool,
                AnnotationTool::Pen | AnnotationTool::Line | AnnotationTool::Text
            ),
            PickerKind::Width => matches!(tool, AnnotationTool::Pen | AnnotationTool::Line),
            PickerKind::TextSize => tool == AnnotationTool::Text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Handle,
    Tool(AnnotationTool),
    Picker(PickerKind),
    Color(Rgb),
    Width(u32),
    TextSize(u32),
    PreviousPage,
    NextPage,
    Undo,
    Redo,
    Delete,
    Clear,
    Exit,
}

/// True for targets that only ever appear inside a picker.
fn picker_target(target: Target) -> bool {
    matches!(
        target,
        Target::Color(_)
            | Target::Width(_)
            | Target::TextSize(_)
            | Target::PreviousPage
            | Target::NextPage
    )
}

pub(crate) struct ToolbarValues<'a> {
    pub editing: bool,
    pub tool: AnnotationTool,
    pub color: Rgb,
    pub width_px: u32,
    pub text_size_px: u32,
    pub colors: &'a [Rgb],
    pub widths: &'a [u32],
    pub text_sizes: &'a [u32],
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_selection: bool,
    pub has_content: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolbarControl {
    pub target: Target,
    pub bounds: Rect,
    pub label: String,
    pub enabled: bool,
    pub selected: bool,
    pub swatch: Option<Rgb>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolbarLayout {
    pub bar: Rect,
    pub picker: Option<Rect>,
    pub controls: Vec<ToolbarControl>,
}

impl ToolbarLayout {
    /// Everything the toolbar paints, padded by one pixel for borders.
    pub(crate) fn paint_bounds(&self) -> Rect {
        match &self.picker {
            Some(picker) => self.bar.union(picker).padded(1),
            None => self.bar.padded(1),
        }
    }

    /// Picker controls win over bar controls where the rectangles overlap.
    /// Returns `None` for blank pixels inside the toolbar and for points
    /// outside it; callers distinguish those with the bar and picker rects.
    pub(crate) fn hit(&self, point: Point) -> Option<&ToolbarControl> {
        if let Some(picker) = self.picker
            && picker.contains(point)
        {
            return self.controls.iter().find(|control| {
                picker_target(control.target) && control.bounds.contains(point)
            });
        }
        if self.bar.contains(point) {
            return self
                .controls
                .iter()
                .find(|control| !picker_target(control.target) && control.bounds.contains(point));
        }
        None
    }
}

/// Outcome of one pointer event: whether the event stays inside the
/// toolbar, an optional command to apply, and optional direct damage.
#[derive(Debug, Default)]
pub(crate) struct ToolbarReply {
    pub consumed: bool,
    pub command: Option<AnnotationCommand>,
    pub damage: Option<Rect>,
}

/// One latched pointer sequence. Assigning on press and keeping the
/// assignment until release or cancellation prevents toolbar presses from
/// becoming canvas gestures and vice versa.
#[derive(Clone, Copy)]
enum Sequence {
    Idle,
    Canvas,
    Button(Target),
    Drag { origin: Point, offset: Point },
    Consume,
}

fn command_for(target: Target) -> Option<AnnotationCommand> {
    match target {
        Target::Tool(tool) => Some(AnnotationCommand::Tool(tool)),
        Target::Color(color) => Some(AnnotationCommand::Color(Preset::Value(color))),
        Target::Width(width) => Some(AnnotationCommand::Width(Preset::Value(width))),
        Target::TextSize(size) => Some(AnnotationCommand::TextSize(Preset::Value(size))),
        Target::Undo => Some(AnnotationCommand::Undo),
        Target::Redo => Some(AnnotationCommand::Redo),
        Target::Delete => Some(AnnotationCommand::Delete),
        Target::Clear => Some(AnnotationCommand::Clear),
        Target::Exit => Some(AnnotationCommand::Off),
        Target::Handle | Target::Picker(_) | Target::PreviousPage | Target::NextPage => None,
    }
}

/// Position, picker state, and the last visible layout. All geometry is in
/// root coordinates and all layout math is pure.
pub(crate) struct Toolbar {
    geometry: DisplayGeometry,
    /// Remembered top-left position; initialized on first activation and
    /// preserved while hidden.
    origin: Option<Point>,
    picker: Option<PickerKind>,
    page: usize,
    layout: Option<ToolbarLayout>,
    sequence: Sequence,
}

impl Toolbar {
    pub(crate) fn new(geometry: DisplayGeometry) -> Self {
        Self {
            geometry,
            origin: None,
            picker: None,
            page: 0,
            layout: None,
            sequence: Sequence::Idle,
        }
    }

    /// Rebuild the visible layout and report the damage only when the
    /// displayed layout or state changed. Hidden toolbars keep their
    /// position and report no layout.
    pub(crate) fn sync(&mut self, values: &ToolbarValues<'_>) -> Option<Rect> {
        if let Some(kind) = self.picker
            && !kind.applies_to(values.tool)
        {
            self.picker = None;
            self.page = 0;
        }
        let next = if values.editing {
            Some(self.compute(values))
        } else {
            None
        };
        if next == self.layout {
            return None;
        }
        let damage = match (&self.layout, &next) {
            (Some(previous), Some(next)) => {
                Some(previous.paint_bounds().union(&next.paint_bounds()))
            }
            (Some(previous), None) => Some(previous.paint_bounds()),
            (None, Some(next)) => Some(next.paint_bounds()),
            (None, None) => None,
        };
        self.layout = next;
        damage
    }

    /// Adopt new display geometry, clamping the remembered page index, then
    /// report damage as a normal sync.
    pub(crate) fn set_geometry(
        &mut self,
        geometry: DisplayGeometry,
        values: &ToolbarValues<'_>,
    ) -> Option<Rect> {
        self.geometry = geometry;
        if let Some(kind) = self.picker {
            let origin = self.origin.unwrap_or(self.geometry.pointer);
            let pages = layout::picker_page_count(&self.geometry, origin, values, kind);
            self.page = self.page.min(pages.saturating_sub(1));
        }
        self.sync(values)
    }

    pub(crate) fn layout(&self) -> Option<&ToolbarLayout> {
        self.layout.as_ref()
    }

    fn compute(&mut self, values: &ToolbarValues<'_>) -> ToolbarLayout {
        let origin = match self.origin {
            Some(origin) => origin,
            None => {
                let monitor = layout::monitor_for_point(&self.geometry, self.geometry.pointer);
                let probe = layout::build_layout(
                    &self.geometry,
                    Point {
                        x: monitor.x,
                        y: monitor.y + TOP_OFFSET_PX,
                    },
                    values,
                    None,
                    0,
                );
                let centered = Point {
                    x: monitor.x + ((monitor.width as i32 - probe.bar.width as i32) / 2).max(0),
                    y: monitor.y + TOP_OFFSET_PX,
                };
                self.origin = Some(centered);
                centered
            }
        };
        layout::build_layout(&self.geometry, origin, values, self.picker, self.page)
    }

    /// Latch or advance one pointer sequence. The session has already
    /// validated ownership and the epoch.
    pub(crate) fn pointer(
        &mut self,
        input: PointerInput,
        _values: &ToolbarValues<'_>,
    ) -> ToolbarReply {
        let mut reply = ToolbarReply::default();
        match input {
            PointerInput::Down { root, .. } => self.pointer_down(root, &mut reply),
            PointerInput::Move { root, .. } => self.pointer_move(root, &mut reply),
            PointerInput::Up { root, .. } => self.pointer_up(root, &mut reply),
        }
        reply
    }

    fn pointer_down(&mut self, root: Point, reply: &mut ToolbarReply) {
        let Some(layout) = self.layout.clone() else {
            self.sequence = Sequence::Idle;
            return;
        };
        let inside =
            layout.bar.contains(root) || layout.picker.is_some_and(|picker| picker.contains(root));
        match layout.hit(root) {
            Some(control) if control.enabled => {
                reply.consumed = true;
                match control.target {
                    Target::Handle => {
                        self.close_picker();
                        let origin = self.origin.unwrap_or(root);
                        self.sequence = Sequence::Drag {
                            origin,
                            offset: Point {
                                x: root.x - origin.x,
                                y: root.y - origin.y,
                            },
                        };
                    }
                    Target::Picker(kind) => {
                        // Pressing a different preset's control dismisses the
                        // open picker; pressing the open picker's own control
                        // toggles it on release.
                        if let Some(open) = self.picker
                            && open != kind
                        {
                            self.close_picker();
                        }
                        self.sequence = Sequence::Button(Target::Picker(kind));
                    }
                    target if picker_target(target) => {
                        // Values and page buttons inside the open picker stay
                        // open through the press.
                        self.sequence = Sequence::Button(target);
                    }
                    target => {
                        self.close_picker();
                        self.sequence = Sequence::Button(target);
                    }
                }
            }
            Some(_) => {
                // Disabled controls are consumed without a command.
                self.sequence = Sequence::Consume;
                reply.consumed = true;
            }
            None if inside => {
                // Blank toolbar pixels belong to the toolbar.
                self.sequence = Sequence::Consume;
                reply.consumed = true;
            }
            None if self.picker.is_some() => {
                // An outside click dismisses the picker and is consumed
                // through its release; it never reaches the canvas.
                self.close_picker();
                self.sequence = Sequence::Consume;
                reply.consumed = true;
            }
            None => {
                self.sequence = Sequence::Canvas;
            }
        }
    }

    fn pointer_move(&mut self, root: Point, reply: &mut ToolbarReply) {
        let drag = match self.sequence {
            Sequence::Idle | Sequence::Canvas => None,
            Sequence::Button(_) | Sequence::Consume => {
                reply.consumed = true;
                None
            }
            Sequence::Drag { offset, .. } => {
                reply.consumed = true;
                Some(offset)
            }
        };
        if let Some(offset) = drag {
            self.apply_drag(offset, root);
        }
    }

    fn pointer_up(&mut self, root: Point, reply: &mut ToolbarReply) {
        let sequence = std::mem::replace(&mut self.sequence, Sequence::Idle);
        match sequence {
            Sequence::Idle | Sequence::Canvas => {}
            Sequence::Consume => reply.consumed = true,
            Sequence::Drag { offset, .. } => {
                // Apply the release position even without a preceding move.
                self.apply_drag(offset, root);
                reply.consumed = true;
            }
            Sequence::Button(target) => {
                reply.consumed = true;
                let active = self
                    .layout
                    .as_ref()
                    .and_then(|layout| layout.hit(root))
                    .is_some_and(|control| control.target == target && control.enabled);
                if active {
                    self.activate(target, reply);
                }
            }
        }
    }

    fn activate(&mut self, target: Target, reply: &mut ToolbarReply) {
        match target {
            Target::Picker(kind) => {
                if self.picker == Some(kind) {
                    self.close_picker();
                } else {
                    self.picker = Some(kind);
                    self.page = 0;
                }
            }
            Target::PreviousPage => self.page = self.page.saturating_sub(1),
            Target::NextPage => self.page += 1,
            target => {
                // Selecting a value closes the picker before its preset
                // command is returned. Page controls issue no command.
                if picker_target(target) {
                    self.close_picker();
                }
                reply.command = command_for(target);
            }
        }
    }

    fn apply_drag(&mut self, offset: Point, at: Point) {
        let Some((width, height)) = self
            .layout
            .as_ref()
            .map(|layout| (layout.bar.width, layout.bar.height))
        else {
            return;
        };
        let monitor = layout::monitor_for_point(&self.geometry, at);
        let raw = Point {
            x: at.x - offset.x,
            y: at.y - offset.y,
        };
        self.origin = Some(layout::clamp_origin(raw, (width, height), monitor));
    }

    /// Escape closes an open picker first, otherwise restores an active
    /// drag and consumes the rest of the sequence. Returns whether the key
    /// was consumed and any direct damage.
    pub(crate) fn escape(&mut self, _values: &ToolbarValues<'_>) -> (bool, Option<Rect>) {
        if self.picker.is_some() {
            self.close_picker();
            return (true, None);
        }
        if let Sequence::Drag { origin, .. } = self.sequence {
            self.origin = Some(origin);
            self.sequence = Sequence::Consume;
            return (true, None);
        }
        (false, None)
    }

    /// Cancellation (ownership loss, command cancel, display change) closes
    /// pickers, restores an active drag, and keeps the remaining events of a
    /// toolbar press away from the canvas. The next press starts fresh.
    pub(crate) fn cancel(&mut self, _values: &ToolbarValues<'_>) -> Option<Rect> {
        if self.picker.take().is_some() {
            self.page = 0;
        }
        if let Sequence::Drag { origin, .. } = self.sequence {
            self.origin = Some(origin);
        }
        if matches!(self.sequence, Sequence::Button(_) | Sequence::Drag { .. }) {
            self.sequence = Sequence::Consume;
        }
        None
    }

    fn close_picker(&mut self) {
        if self.picker.take().is_some() {
            self.page = 0;
        }
    }
}
