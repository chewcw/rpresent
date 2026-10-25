use crate::{AnnotationTool, Point, Rect, Rgb};

mod layout;

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
        if let Some(picker) = self.picker {
            if picker.contains(point) {
                return self
                    .controls
                    .iter()
                    .find(|control| picker_target(control.target) && control.bounds.contains(point));
            }
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
}

impl Toolbar {
    pub(crate) fn new(geometry: DisplayGeometry) -> Self {
        Self {
            geometry,
            origin: None,
            picker: None,
            page: 0,
            layout: None,
        }
    }

    /// Rebuild the visible layout and report the damage only when the
    /// displayed layout or state changed. Hidden toolbars keep their
    /// position and report no layout.
    pub(crate) fn sync(&mut self, values: &ToolbarValues<'_>) -> Option<Rect> {
        if let Some(kind) = self.picker {
            if !kind.applies_to(values.tool) {
                self.picker = None;
                self.page = 0;
            }
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
            (Some(previous), Some(next)) => Some(previous.paint_bounds().union(&next.paint_bounds())),
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
}
