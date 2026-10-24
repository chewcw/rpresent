use crate::{AnnotationTool, Point, Rect, Rgb};

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
