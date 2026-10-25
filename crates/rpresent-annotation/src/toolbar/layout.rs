use super::*;

/// A control before its position in the bar or a picker is known.
struct Spec {
    target: Target,
    label: String,
    enabled: bool,
    selected: bool,
    swatch: Option<Rgb>,
}

impl Spec {
    fn new(target: Target, label: impl Into<String>, enabled: bool) -> Self {
        Self {
            target,
            label: label.into(),
            enabled,
            selected: false,
            swatch: None,
        }
    }

    /// Conservative width for a monospace label plus horizontal padding and
    /// room for a swatch when the control shows one.
    fn width(&self) -> u32 {
        let text = self.label.chars().count() as u32 * CHAR_PX;
        let swatch = if self.swatch.is_some() {
            SWATCH_PX + GAP_PX as u32
        } else {
            0
        };
        text + swatch + (PADDING_PX as u32) * 2
    }
}

fn hex(color: Rgb) -> String {
    format!("#{:02X}{:02X}{:02X}", color[0], color[1], color[2])
}

fn tool_label(tool: AnnotationTool) -> &'static str {
    match tool {
        AnnotationTool::Select => "Select",
        AnnotationTool::Pen => "Pen",
        AnnotationTool::Line => "Line",
        AnnotationTool::Eraser => "Eraser",
        AnnotationTool::Text => "Text",
    }
}

/// The approved control groups in catalog order: drag handle, tools,
/// applicable preset pickers, then document actions.
fn groups(values: &ToolbarValues<'_>, picker: Option<PickerKind>) -> Vec<Vec<Spec>> {
    let handle = Spec::new(Target::Handle, "Drag", true);

    let tools: Vec<Spec> = crate::TOOLS
        .iter()
        .map(|tool| Spec {
            target: Target::Tool(*tool),
            label: tool_label(*tool).into(),
            enabled: true,
            selected: *tool == values.tool,
            swatch: None,
        })
        .collect();

    let mut presets = Vec::new();
    for kind in [PickerKind::Color, PickerKind::Width, PickerKind::TextSize] {
        if !kind.applies_to(values.tool) {
            continue;
        }
        let mut spec = match kind {
            PickerKind::Color => Spec::new(
                Target::Picker(kind),
                format!("Color {}", hex(values.color)),
                true,
            ),
            PickerKind::Width => Spec::new(
                Target::Picker(kind),
                format!("Width {} px", values.width_px),
                true,
            ),
            PickerKind::TextSize => Spec::new(
                Target::Picker(kind),
                format!("Text size {} px", values.text_size_px),
                true,
            ),
        };
        if kind == PickerKind::Color {
            spec.swatch = Some(values.color);
        }
        spec.selected = picker == Some(kind);
        presets.push(spec);
    }

    let actions = vec![
        Spec::new(Target::Undo, "Undo", values.can_undo),
        Spec::new(Target::Redo, "Redo", values.can_redo),
        Spec::new(Target::Delete, "Delete", values.has_selection),
        Spec::new(Target::Clear, "Clear", values.has_content),
        Spec::new(Target::Exit, "Exit", true),
    ];

    vec![vec![handle], tools, presets, actions]
}

/// Configured values for the open picker, in catalog order.
fn picker_specs(values: &ToolbarValues<'_>, kind: PickerKind) -> Vec<Spec> {
    match kind {
        PickerKind::Color => values
            .colors
            .iter()
            .map(|color| Spec {
                target: Target::Color(*color),
                label: hex(*color),
                enabled: true,
                selected: *color == values.color,
                swatch: Some(*color),
            })
            .collect(),
        PickerKind::Width => values
            .widths
            .iter()
            .map(|width| Spec {
                target: Target::Width(*width),
                label: format!("{width} px"),
                enabled: true,
                selected: *width == values.width_px,
                swatch: None,
            })
            .collect(),
        PickerKind::TextSize => values
            .text_sizes
            .iter()
            .map(|size| Spec {
                target: Target::TextSize(*size),
                label: format!("{size} px"),
                enabled: true,
                selected: *size == values.text_size_px,
                swatch: None,
            })
            .collect(),
    }
}

pub(super) fn clamp_origin(origin: Point, size: (u32, u32), monitor: Rect) -> Point {
    let max_x = monitor.right().saturating_sub(size.0 as i32).max(monitor.x);
    let max_y = monitor.bottom().saturating_sub(size.1 as i32).max(monitor.y);
    Point {
        x: origin.x.clamp(monitor.x, max_x),
        y: origin.y.clamp(monitor.y, max_y),
    }
}

/// The monitor that contains the point, or the nearest monitor when the
/// point falls in a gap. Empty monitor rectangles are ignored and root
/// bounds apply when discovery returned nothing.
pub(super) fn monitor_for_point(geometry: &DisplayGeometry, point: Point) -> Rect {
    let monitors: Vec<Rect> = geometry
        .monitors
        .iter()
        .copied()
        .filter(|monitor| monitor.width > 0 && monitor.height > 0)
        .collect();
    if monitors.is_empty() {
        return geometry.root;
    }
    if let Some(found) = monitors.iter().find(|monitor| monitor.contains(point)) {
        return *found;
    }
    let mut best_index = 0;
    let mut best_distance = i64::MAX;
    for (index, monitor) in monitors.iter().enumerate() {
        let nearest_x = (point.x as i64).clamp(monitor.x as i64, (monitor.right() - 1) as i64);
        let nearest_y = (point.y as i64).clamp(monitor.y as i64, (monitor.bottom() - 1) as i64);
        let dx = point.x as i64 - nearest_x;
        let dy = point.y as i64 - nearest_y;
        let distance = dx * dx + dy * dy;
        if distance < best_distance {
            best_distance = distance;
            best_index = index;
        }
    }
    monitors[best_index]
}

fn row_width(row: &[Spec]) -> i32 {
    if row.is_empty() {
        return 0;
    }
    row.iter().map(|spec| spec.width() as i32).sum::<i32>() + GAP_PX * (row.len() as i32 - 1)
}

/// Place each group on the current row when it fits; otherwise start a new
/// row, splitting oversized groups at control boundaries.
fn wrap(groups: Vec<Vec<Spec>>, max_width: i32) -> Vec<Vec<Spec>> {
    let mut rows: Vec<Vec<Spec>> = vec![Vec::new()];
    for group in groups {
        if group.is_empty() {
            continue;
        }
        let group_width = row_width(&group);
        let used = row_width(rows.last().unwrap());
        let needed = if used == 0 {
            group_width
        } else {
            used + GAP_PX + group_width
        };
        if needed <= max_width {
            rows.last_mut().unwrap().extend(group);
            continue;
        }
        if !rows.last().unwrap().is_empty() {
            rows.push(Vec::new());
        }
        for spec in group {
            let used = row_width(rows.last().unwrap());
            let needed = if used == 0 {
                spec.width() as i32
            } else {
                used + GAP_PX + spec.width() as i32
            };
            if needed > max_width && !rows.last().unwrap().is_empty() {
                rows.push(Vec::new());
            }
            rows.last_mut().unwrap().push(spec);
        }
    }
    rows.retain(|row| !row.is_empty());
    rows
}

/// Build the bar and its controls at a clamped origin. Picker state only
/// affects which preset control is highlighted.
fn build_bar(
    geometry: &DisplayGeometry,
    origin: Point,
    values: &ToolbarValues<'_>,
    picker: Option<PickerKind>,
) -> (Rect, Vec<ToolbarControl>) {
    let monitor = monitor_for_point(geometry, origin);
    let max_width = (monitor.width as i32 - 2 * PADDING_PX).max(1);
    let rows = wrap(groups(values, picker), max_width);
    let widest = rows.iter().map(|row| row_width(row)).max().unwrap_or(0);
    let height = 2 * PADDING_PX
        + rows.len() as i32 * ROW_PX as i32
        + (rows.len().max(1) as i32 - 1) * GAP_PX;
    let origin = clamp_origin(
        origin,
        ((2 * PADDING_PX + widest.max(0)) as u32, height.max(0) as u32),
        monitor,
    );
    let bar = Rect {
        x: origin.x,
        y: origin.y,
        width: (2 * PADDING_PX + widest.max(0)) as u32,
        height: height.max(0) as u32,
    };

    let mut controls = Vec::new();
    let mut y = origin.y + PADDING_PX;
    for row in rows {
        let mut x = origin.x + PADDING_PX;
        for spec in row {
            let width = spec.width();
            controls.push(ToolbarControl {
                target: spec.target,
                bounds: Rect {
                    x,
                    y,
                    width,
                    height: ROW_PX,
                },
                label: spec.label,
                enabled: spec.enabled,
                selected: spec.selected,
                swatch: spec.swatch,
            });
            x += width as i32 + GAP_PX;
        }
        y += ROW_PX as i32 + GAP_PX;
    }
    (bar, controls)
}

fn picker_columns(cell_width: u32, monitor: Rect) -> usize {
    let inner = (monitor.width as i32 - 2 * PADDING_PX).max(cell_width as i32);
    (((inner + GAP_PX) / (cell_width as i32 + GAP_PX)) as usize).max(1)
}

fn picker_rows(budget: i32, reserve_page: bool) -> usize {
    let page_extra = if reserve_page {
        ROW_PX as i32 + GAP_PX
    } else {
        0
    };
    let inner = budget - 2 * PADDING_PX - page_extra + GAP_PX;
    (inner / (ROW_PX as i32 + GAP_PX)).max(1) as usize
}

/// Lay out the bounded picker for the current page. Returns the picker
/// rectangle, its controls, and the total page count.
fn picker_plan(
    anchor: Rect,
    bar: Rect,
    monitor: Rect,
    entries: &[Spec],
    page: usize,
) -> (Rect, Vec<ToolbarControl>, usize) {
    let cell_width = entries
        .iter()
        .map(Spec::width)
        .max()
        .unwrap_or(ROW_PX)
        .max(1);
    let columns = picker_columns(cell_width, monitor);
    let page_rows = ROW_PX as i32 + GAP_PX;

    let budget_below = monitor.bottom() - anchor.bottom() - GAP_PX - PADDING_PX;
    let budget_above = bar.y - GAP_PX - monitor.y - PADDING_PX;
    let minimum = 2 * PADDING_PX + ROW_PX as i32;
    let (budget, place_below) = if budget_below >= minimum {
        (budget_below, true)
    } else if budget_above >= minimum {
        (budget_above, false)
    } else {
        (budget_below.max(budget_above).max(minimum), true)
    };

    let fit_rows = picker_rows(budget, false);
    let first_capacity = fit_rows * columns;
    let pages = if entries.is_empty() || first_capacity == 0 {
        1
    } else {
        entries.len().div_ceil(first_capacity)
    };
    let (_rows, capacity, pages) = if pages <= 1 {
        (fit_rows, first_capacity, 1)
    } else {
        let rows = picker_rows(budget, true);
        let capacity = rows * columns;
        let pages = entries.len().div_ceil(capacity.max(1));
        (rows, capacity.max(1), pages.max(1))
    };
    let page = page.min(pages - 1);

    let start = (page * capacity).min(entries.len());
    let end = (start + capacity).min(entries.len());
    let visible = &entries[start..end];
    let used_rows = if columns == 0 {
        1
    } else {
        visible.len().div_ceil(columns).max(1)
    };
    let show_pages = pages > 1;

    let width = (2 * PADDING_PX as u32 + columns as u32 * cell_width + (columns as u32 - 1) * GAP_PX as u32)
        .min(monitor.width.max(1));
    let height = 2 * PADDING_PX as u32
        + used_rows as u32 * ROW_PX
        + (used_rows as u32 - 1) * GAP_PX as u32
        + if show_pages {
            page_rows as u32
        } else {
            0
        };

    let x = anchor
        .x
        .clamp(monitor.x, monitor.right().saturating_sub(width as i32));
    let y = if place_below {
        anchor.bottom() + GAP_PX
    } else {
        bar.y - GAP_PX - height as i32
    };
    let y = y.clamp(monitor.y, monitor.bottom().saturating_sub(height as i32).max(monitor.y));
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    let mut controls = Vec::new();
    for (index, spec) in visible.iter().enumerate() {
        let column = index % columns.max(1);
        let row = index / columns.max(1);
        controls.push(ToolbarControl {
            target: spec.target,
            bounds: Rect {
                x: rect.x + PADDING_PX + column as i32 * (cell_width as i32 + GAP_PX),
                y: rect.y + PADDING_PX + row as i32 * (ROW_PX as i32 + GAP_PX),
                width: cell_width,
                height: ROW_PX,
            },
            label: spec.label.clone(),
            enabled: spec.enabled,
            selected: spec.selected,
            swatch: spec.swatch,
        });
    }

    if show_pages {
        let row_y = rect.bottom() - PADDING_PX - ROW_PX as i32;
        let previous = Spec::new(Target::PreviousPage, "Previous", true);
        let next = Spec::new(Target::NextPage, "Next", true);
        controls.push(ToolbarControl {
            target: Target::PreviousPage,
            bounds: Rect {
                x: rect.x + PADDING_PX,
                y: row_y,
                width: previous.width(),
                height: ROW_PX,
            },
            label: previous.label,
            enabled: page > 0,
            selected: false,
            swatch: None,
        });
        controls.push(ToolbarControl {
            target: Target::NextPage,
            bounds: Rect {
                x: rect.right() - PADDING_PX - next.width() as i32,
                y: row_y,
                width: next.width(),
                height: ROW_PX,
            },
            label: next.label,
            enabled: page + 1 < pages,
            selected: false,
            swatch: None,
        });
    }

    (rect, controls, pages)
}

pub(super) fn build_layout(
    geometry: &DisplayGeometry,
    origin: Point,
    values: &ToolbarValues<'_>,
    picker: Option<PickerKind>,
    page: usize,
) -> ToolbarLayout {
    let monitor = monitor_for_point(geometry, origin);
    let (bar, mut controls) = build_bar(geometry, origin, values, picker);

    let picker_rect = match picker {
        Some(kind) if kind.applies_to(values.tool) => {
            let anchor = controls
                .iter()
                .find(|control| control.target == Target::Picker(kind))
                .map(|control| control.bounds);
            anchor.and_then(|anchor| {
                let entries = picker_specs(values, kind);
                let (rect, picker_controls, _) =
                    picker_plan(anchor, bar, monitor, &entries, page);
                controls.extend(picker_controls);
                Some(rect)
            })
        }
        _ => None,
    };

    ToolbarLayout {
        bar,
        picker: picker_rect,
        controls,
    }
}

/// Total pages for the open picker under the current geometry. Used to clamp
/// the remembered page index after geometry changes.
pub(super) fn picker_page_count(
    geometry: &DisplayGeometry,
    origin: Point,
    values: &ToolbarValues<'_>,
    kind: PickerKind,
) -> usize {
    if !kind.applies_to(values.tool) {
        return 1;
    }
    let monitor = monitor_for_point(geometry, origin);
    let (bar, controls) = build_bar(geometry, origin, values, Some(kind));
    let Some(anchor) = controls
        .iter()
        .find(|control| control.target == Target::Picker(kind))
        .map(|control| control.bounds)
    else {
        return 1;
    };
    let entries = picker_specs(values, kind);
    picker_plan(anchor, bar, monitor, &entries, 0).2
}
