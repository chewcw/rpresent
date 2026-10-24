use cosmic_text::Color as TextColor;
use tiny_skia::{FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::document::{Object, ObjectData, Point};
use crate::error::OverlayError;
use crate::geometry::Rect;
use crate::session::{Gesture, Session, TEXT_BOX_PADDING_PX, TextEdit};
use crate::text::TextSystem;

/// Create a premultiplied RGBA framebuffer that covers the desktop.
pub struct Renderer {
    pixmap: Pixmap,
    mask: Mask,
}

impl Renderer {
    pub fn new(width: u32, height: u32) -> Result<Renderer, OverlayError> {
        let pixmap = Pixmap::new(width, height).ok_or_else(|| {
            OverlayError::Runtime("failed to allocate annotation framebuffer".into())
        })?;
        let mask = Mask::new(width, height).ok_or_else(|| {
            OverlayError::Runtime("failed to allocate annotation damage mask".into())
        })?;
        Ok(Self { pixmap, mask })
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    pub fn pixels(&self) -> &[u8] {
        self.pixmap.data()
    }

    pub fn full_bounds(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: self.pixmap.width(),
            height: self.pixmap.height(),
        }
    }

    pub fn redraw(&mut self, session: &mut Session, damage: Rect) -> Result<(), OverlayError> {
        let Some(clip) = damage.intersect(&self.full_bounds()) else {
            return Ok(());
        };
        clear_region(&mut self.pixmap, clip);
        self.reset_mask(clip);

        let objects = session.document().objects().to_vec();
        let hidden: Vec<_> = match &session.gesture {
            Some(Gesture::Erase { hidden, .. }) => hidden.clone(),
            _ => Vec::new(),
        };
        let editing = session.text_edit.as_ref().and_then(|edit| edit.object);
        let moved = match &session.gesture {
            Some(Gesture::Move { id, offset, .. }) => Some((*id, *offset)),
            _ => None,
        };

        for object in &objects {
            if hidden.contains(&object.id) || editing == Some(object.id) {
                continue;
            }
            if matches!(moved, Some((id, _)) if id == object.id) {
                continue;
            }
            let bounds = object_paint_bounds(&mut session.text_system, object);
            if bounds.intersect(&clip).is_none() {
                continue;
            }
            self.draw_object(&mut session.text_system, object, &clip);
        }

        if let Some((id, offset)) = moved
            && let Some(object) = objects.iter().find(|object| object.id == id)
        {
            let mut shifted = object.clone();
            shifted.origin.x += offset.x;
            shifted.origin.y += offset.y;
            let bounds = object_paint_bounds(&mut session.text_system, &shifted);
            if bounds.intersect(&clip).is_some() {
                self.draw_object(&mut session.text_system, &shifted, &clip);
            }
        }

        match &session.gesture {
            Some(Gesture::Pen { origin, points }) => {
                let width = session.width_px();
                draw_polyline(
                    &mut self.pixmap,
                    &self.mask,
                    *origin,
                    points,
                    width,
                    session.color(),
                    &clip,
                );
            }
            Some(Gesture::Line { origin, end }) => {
                let width = session.width_px();
                draw_polyline(
                    &mut self.pixmap,
                    &self.mask,
                    *origin,
                    &[Point::default(), *end],
                    width,
                    session.color(),
                    &clip,
                );
            }
            _ => {}
        }

        if session.text_edit.is_none()
            && let Some(id) = session.selection()
            && let Some(object) = objects.iter().find(|object| object.id == id)
        {
            let bounds = object_paint_bounds(&mut session.text_system, object).padded(1);
            draw_selection(&mut self.pixmap, &self.mask, bounds, &clip);
        }

        if let Some(edit) = session.text_edit.as_mut() {
            let bounds = edit
                .draft
                .bounds(edit.origin, &mut session.text_system)
                .padded(TEXT_BOX_PADDING_PX);
            if bounds.intersect(&clip).is_some() {
                draw_text_edit(
                    &mut self.pixmap,
                    &self.mask,
                    edit,
                    &mut session.text_system,
                    bounds,
                    &clip,
                );
            }
        }

        Ok(())
    }

    fn reset_mask(&mut self, clip: Rect) {
        self.mask.clear();
        let Some(rect) = tiny_skia::Rect::from_xywh(
            clip.x as f32,
            clip.y as f32,
            clip.width as f32,
            clip.height as f32,
        ) else {
            return;
        };
        let mut builder = PathBuilder::new();
        builder.push_rect(rect);
        if let Some(path) = builder.finish() {
            self.mask
                .fill_path(&path, FillRule::Winding, false, Transform::identity());
        }
    }

    fn draw_object(&mut self, text_system: &mut TextSystem, object: &Object, clip: &Rect) {
        match &object.data {
            ObjectData::Pen { points, width_px } => draw_polyline(
                &mut self.pixmap,
                &self.mask,
                object.origin,
                points,
                *width_px,
                object.color,
                clip,
            ),
            ObjectData::Line { end, width_px } => draw_polyline(
                &mut self.pixmap,
                &self.mask,
                object.origin,
                &[Point::default(), *end],
                *width_px,
                object.color,
                clip,
            ),
            ObjectData::Text { .. } => {
                let [red, green, blue] = object.color;
                let color = TextColor::rgb(red, green, blue);
                let pixmap = &mut self.pixmap;
                text_system.draw_object(
                    object,
                    color,
                    |x, y, width, height, red, green, blue, alpha| {
                        blend_rect(
                            pixmap,
                            x + object.origin.x,
                            y + object.origin.y,
                            width,
                            height,
                            red,
                            green,
                            blue,
                            alpha,
                            clip,
                        );
                    },
                );
            }
        }
    }
}

/// Paint bounds of an object, including antialiasing room.
pub(crate) fn object_paint_bounds(text_system: &mut TextSystem, object: &Object) -> Rect {
    match &object.data {
        ObjectData::Pen { points, width_px } => stroke_bounds(object.origin, points, *width_px),
        ObjectData::Line { end, width_px } => {
            stroke_bounds(object.origin, &[Point::default(), *end], *width_px)
        }
        ObjectData::Text { .. } => text_system.bounds(object).padded(2),
    }
}

pub(crate) fn stroke_bounds(origin: Point, points: &[Point], width_px: u32) -> Rect {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for point in points {
        let absolute = Point {
            x: origin.x + point.x,
            y: origin.y + point.y,
        };
        min_x = min_x.min(absolute.x);
        min_y = min_y.min(absolute.y);
        max_x = max_x.max(absolute.x);
        max_y = max_y.max(absolute.y);
    }
    if points.is_empty() {
        min_x = origin.x;
        max_x = origin.x;
        min_y = origin.y;
        max_y = origin.y;
    }
    let pad = width_px.div_ceil(2) as i32 + 2;
    Rect {
        x: min_x - pad,
        y: min_y - pad,
        width: (max_x - min_x).max(0) as u32 + pad as u32 * 2,
        height: (max_y - min_y).max(0) as u32 + pad as u32 * 2,
    }
}

/// Clear a region to transparent.
pub(crate) fn clear_region(pixmap: &mut Pixmap, region: Rect) {
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let x0 = region.x.clamp(0, width);
    let y0 = region.y.clamp(0, height);
    let x1 = region.right().clamp(0, width);
    let y1 = region.bottom().clamp(0, height);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let row_bytes = width as usize * 4;
    let data = pixmap.data_mut();
    for y in y0..y1 {
        let start = y as usize * row_bytes + x0 as usize * 4;
        let end = y as usize * row_bytes + x1 as usize * 4;
        data[start..end].fill(0);
    }
}

fn draw_polyline(
    pixmap: &mut Pixmap,
    mask: &Mask,
    origin: Point,
    points: &[Point],
    width_px: u32,
    color: [u8; 3],
    clip: &Rect,
) {
    if points.is_empty() {
        return;
    }
    let absolute: Vec<(f32, f32)> = points
        .iter()
        .map(|point| ((origin.x + point.x) as f32, (origin.y + point.y) as f32))
        .collect();
    let degenerate = absolute.windows(2).all(|pair| pair[0] == pair[1]);
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    paint.anti_alias = true;
    let radius = (width_px as f32 / 2.0).max(0.5);
    if degenerate {
        let mut builder = PathBuilder::new();
        builder.push_circle(absolute[0].0, absolute[0].1, radius);
        if let Some(path) = builder.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                FillRule::Winding,
                Transform::identity(),
                Some(mask),
            );
        }
        return;
    }
    let mut builder = PathBuilder::new();
    let mut first = true;
    for (x, y) in &absolute {
        if first {
            builder.move_to(*x, *y);
            first = false;
        } else {
            builder.line_to(*x, *y);
        }
    }
    let Some(path) = builder.finish() else {
        return;
    };
    let stroke = Stroke {
        width: width_px.max(1) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), Some(mask));
    let _ = clip;
}

fn draw_selection(pixmap: &mut Pixmap, mask: &Mask, bounds: Rect, clip: &Rect) {
    if bounds.intersect(clip).is_none() {
        return;
    }
    let Some(rect) = tiny_skia::Rect::from_xywh(
        bounds.x as f32 - 0.5,
        bounds.y as f32 - 0.5,
        bounds.width as f32 + 1.0,
        bounds.height as f32 + 1.0,
    ) else {
        return;
    };
    let mut builder = PathBuilder::new();
    builder.push_rect(rect);
    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(30, 160, 255, 220);
    paint.anti_alias = true;
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), Some(mask));
}

fn draw_text_edit(
    pixmap: &mut Pixmap,
    mask: &Mask,
    edit: &mut TextEdit,
    text_system: &mut TextSystem,
    bounds: Rect,
    clip: &Rect,
) {
    let [red, green, blue] = edit.color;
    let luminance = 0.299 * red as f32 + 0.587 * green as f32 + 0.114 * blue as f32;
    let [background, foreground] = if luminance > 140.0 {
        [[0u8, 0u8, 0u8], [255u8, 255u8, 255u8]]
    } else {
        [[255u8, 255u8, 255u8], [0u8, 0u8, 0u8]]
    };

    fill_clipped(pixmap, bounds, background, 235, clip);
    draw_border(pixmap, mask, bounds, foreground, clip);

    let text_color = TextColor::rgb(red, green, blue);
    let cursor_color = TextColor::rgb(foreground[0], foreground[1], foreground[2]);
    let selection_color = TextColor::rgba(foreground[0], foreground[1], foreground[2], 90);
    let selected_text_color = TextColor::rgb(background[0], background[1], background[2]);
    let origin = edit.origin;
    edit.draft.draw(
        text_system,
        text_color,
        cursor_color,
        selection_color,
        selected_text_color,
        |x, y, width, height, red, green, blue, alpha| {
            blend_rect(
                pixmap,
                x + origin.x,
                y + origin.y,
                width,
                height,
                red,
                green,
                blue,
                alpha,
                clip,
            );
        },
    );
}

fn fill_clipped(pixmap: &mut Pixmap, rect: Rect, color: [u8; 3], alpha: u8, clip: &Rect) {
    let Some(visible) = rect.intersect(clip) else {
        return;
    };
    blend_rect(
        pixmap,
        visible.x,
        visible.y,
        visible.width,
        visible.height,
        color[0],
        color[1],
        color[2],
        alpha,
        clip,
    );
}

fn draw_border(pixmap: &mut Pixmap, mask: &Mask, bounds: Rect, color: [u8; 3], clip: &Rect) {
    if bounds.intersect(clip).is_none() {
        return;
    }
    let Some(rect) = tiny_skia::Rect::from_xywh(
        bounds.x as f32 + 0.5,
        bounds.y as f32 + 0.5,
        (bounds.width as f32 - 1.0).max(0.0),
        (bounds.height as f32 - 1.0).max(0.0),
    ) else {
        return;
    };
    let mut builder = PathBuilder::new();
    builder.push_rect(rect);
    let Some(path) = builder.finish() else {
        return;
    };
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), Some(mask));
}

#[allow(clippy::too_many_arguments)]
fn blend_rect(
    pixmap: &mut Pixmap,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
    clip: &Rect,
) {
    if alpha == 0 {
        return;
    }
    let pixmap_width = pixmap.width() as i32;
    let pixmap_height = pixmap.height() as i32;
    let start_x = x.max(clip.x).max(0);
    let start_y = y.max(clip.y).max(0);
    let end_x = (x + width as i32).min(clip.right()).min(pixmap_width);
    let end_y = (y + height as i32).min(clip.bottom()).min(pixmap_height);
    if end_x <= start_x || end_y <= start_y {
        return;
    }
    let source_alpha = alpha as u32;
    let inverse = 255 - source_alpha;
    let source_red = red as u32 * source_alpha / 255;
    let source_green = green as u32 * source_alpha / 255;
    let source_blue = blue as u32 * source_alpha / 255;
    let row_bytes = pixmap_width as usize * 4;
    let data = pixmap.data_mut();
    for py in start_y..end_y {
        let row = py as usize * row_bytes;
        for px in start_x..end_x {
            let offset = row + px as usize * 4;
            let destination_alpha = data[offset + 3] as u32;
            data[offset] = (source_red + data[offset] as u32 * inverse / 255).min(255) as u8;
            data[offset + 1] =
                (source_green + data[offset + 1] as u32 * inverse / 255).min(255) as u8;
            data[offset + 2] =
                (source_blue + data[offset + 2] as u32 * inverse / 255).min(255) as u8;
            data[offset + 3] = (source_alpha + destination_alpha * inverse / 255).min(255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::Color;

    #[test]
    fn transparent_patch_replaces_deleted_ink() {
        let mut frame = tiny_skia::Pixmap::new(32, 32).unwrap();
        frame.fill(Color::from_rgba8(255, 0, 0, 255));
        clear_region(
            &mut frame,
            Rect {
                x: 8,
                y: 8,
                width: 8,
                height: 8,
            },
        );
        assert_eq!(frame.pixel(10, 10).unwrap().alpha(), 0);
        assert_eq!(frame.pixel(0, 0).unwrap().alpha(), 255);
    }
}

#[cfg(test)]
mod parity_tests {
    use super::*;
    use crate::{AnnotationCommand, AnnotationTool, PointerInput};
    use rpresent_config::AnnotationConfig;
    use std::time::Duration;

    use crate::document::Point as CanvasPoint;

    #[test]
    #[ignore = "release-mode rendering benchmark"]
    fn render_bench() {
        use std::time::Instant;

        for (width, height) in [(1920, 1080), (3840, 2160)] {
            let mut session = Session::new(&AnnotationConfig::default()).unwrap();
            session.command(AnnotationCommand::On).unwrap();
            session.ownership(Some(1));

            session
                .command(AnnotationCommand::Tool(AnnotationTool::Pen))
                .unwrap();
            for stroke in 0..24 {
                let y = 40 + stroke * 24;
                session.pointer(
                    1,
                    PointerInput::Down {
                        root: Point { x: 40, y },
                        time: Duration::from_millis(stroke as u64 * 10),
                    },
                );
                session.pointer(
                    1,
                    PointerInput::Move {
                        root: Point {
                            x: width as i32 - 40,
                            y,
                        },
                        time: Duration::from_millis(stroke as u64 * 10 + 5),
                    },
                );
                session.pointer(
                    1,
                    PointerInput::Up {
                        root: Point {
                            x: width as i32 - 40,
                            y,
                        },
                        time: Duration::from_millis(stroke as u64 * 10 + 6),
                    },
                );
            }

            session
                .command(AnnotationCommand::Tool(AnnotationTool::Line))
                .unwrap();
            for line in 0..8 {
                let x = 120 + line * 80;
                session.pointer(
                    1,
                    PointerInput::Down {
                        root: Point { x, y: 60 },
                        time: Duration::from_millis(600 + line as u64),
                    },
                );
                session.pointer(
                    1,
                    PointerInput::Up {
                        root: Point {
                            x,
                            y: height as i32 - 80,
                        },
                        time: Duration::from_millis(700 + line as u64),
                    },
                );
            }

            session
                .command(AnnotationCommand::Tool(AnnotationTool::Text))
                .unwrap();
            session.pointer(
                1,
                PointerInput::Down {
                    root: Point { x: 80, y: 80 },
                    time: Duration::from_millis(1000),
                },
            );
            if let Some(edit) = session.text_edit.as_mut() {
                edit.draft
                    .insert("Annotation benchmark", &mut session.text_system);
            }
            session.key(
                1,
                crate::KeyInput {
                    keysym: 0xff0d,
                    text: None,
                    modifiers: crate::Modifiers::default(),
                },
            );

            let objects = session.document().objects().len();
            let mut renderer = Renderer::new(width, height).unwrap();
            let whole = renderer.full_bounds();
            let started = Instant::now();
            renderer.redraw(&mut session, whole).unwrap();
            let full = started.elapsed();

            session
                .command(AnnotationCommand::Tool(AnnotationTool::Pen))
                .ok();
            session.take_damage();
            session.pointer(
                1,
                PointerInput::Down {
                    root: Point { x: 60, y: 60 },
                    time: Duration::from_millis(2000),
                },
            );
            session.pointer(
                1,
                PointerInput::Up {
                    root: Point { x: 60, y: 60 },
                    time: Duration::from_millis(2001),
                },
            );
            if let Some(damage) = session.take_damage() {
                let started = Instant::now();
                renderer.redraw(&mut session, damage).unwrap();
                eprintln!(
                    "{}x{}: {objects} objects, full {:?}, partial {:?} ({}x{} px)",
                    width,
                    height,
                    full,
                    started.elapsed(),
                    damage.width,
                    damage.height,
                );
            }
        }
    }

    #[test]
    fn partial_redraw_matches_full_scene() {
        let mut session = Session::new(&AnnotationConfig::default()).unwrap();
        session.command(AnnotationCommand::On).unwrap();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        session.ownership(Some(1));
        for (start, end) in [((10, 20), (80, 20)), ((40, 10), (40, 80))] {
            session.pointer(
                1,
                PointerInput::Down {
                    root: CanvasPoint {
                        x: start.0,
                        y: start.1,
                    },
                    time: Duration::ZERO,
                },
            );
            session.pointer(
                1,
                PointerInput::Up {
                    root: CanvasPoint { x: end.0, y: end.1 },
                    time: Duration::from_millis(10),
                },
            );
        }
        let mut partial = Renderer::new(100, 100).unwrap();
        let whole = partial.full_bounds();
        partial.redraw(&mut session, whole).unwrap();
        session.take_damage();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        session.pointer(
            1,
            PointerInput::Down {
                root: CanvasPoint { x: 20, y: 20 },
                time: Duration::from_millis(100),
            },
        );
        session.pointer(
            1,
            PointerInput::Up {
                root: CanvasPoint { x: 20, y: 40 },
                time: Duration::from_millis(120),
            },
        );
        let damage = session.take_damage().unwrap();
        partial.redraw(&mut session, damage).unwrap();
        let mut reference = Renderer::new(100, 100).unwrap();
        reference.redraw(&mut session, whole).unwrap();
        assert_eq!(partial.pixels(), reference.pixels());
    }

    fn click(session: &mut Session, x: i32, y: i32, millis: u64) {
        session.pointer(
            1,
            PointerInput::Down {
                root: CanvasPoint { x, y },
                time: Duration::from_millis(millis),
            },
        );
        session.pointer(
            1,
            PointerInput::Up {
                root: CanvasPoint { x, y },
                time: Duration::from_millis(millis + 10),
            },
        );
    }

    fn drag(session: &mut Session, from: (i32, i32), to: (i32, i32)) {
        session.pointer(
            1,
            PointerInput::Down {
                root: CanvasPoint {
                    x: from.0,
                    y: from.1,
                },
                time: Duration::from_millis(0),
            },
        );
        session.pointer(
            1,
            PointerInput::Move {
                root: CanvasPoint { x: to.0, y: to.1 },
                time: Duration::from_millis(5),
            },
        );
        session.pointer(
            1,
            PointerInput::Up {
                root: CanvasPoint { x: to.0, y: to.1 },
                time: Duration::from_millis(10),
            },
        );
    }

    fn press(session: &mut Session, keysym: u32, text: Option<char>) {
        session.key(
            1,
            crate::KeyInput {
                keysym,
                text,
                modifiers: crate::Modifiers::default(),
            },
        );
    }

    fn assert_parity(
        session: &mut Session,
        partial: &mut Renderer,
        reference: &mut Renderer,
        whole: Rect,
        label: &str,
    ) {
        if let Some(damage) = session.take_damage() {
            partial.redraw(session, damage).expect("partial redraw");
        }
        reference.redraw(session, whole).expect("full redraw");
        assert_eq!(partial.pixels(), reference.pixels(), "{label}");
    }

    #[test]
    fn extended_partial_redraws_match_full_scene() {
        let mut session = Session::new(&AnnotationConfig::default()).unwrap();
        session.command(AnnotationCommand::On).unwrap();
        session.ownership(Some(1));
        let mut partial = Renderer::new(100, 100).unwrap();
        let whole = partial.full_bounds();
        partial.redraw(&mut session, whole).unwrap();
        session.take_damage();
        let mut reference = Renderer::new(100, 100).unwrap();

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        click(&mut session, 12, 14, 100);
        assert_parity(
            &mut session,
            &mut partial,
            &mut reference,
            whole,
            "draft opened",
        );
        for character in "Hi".chars() {
            press(&mut session, character as u32, Some(character));
            assert_parity(&mut session, &mut partial, &mut reference, whole, "typing");
        }
        press(&mut session, 0xff0d, None);
        assert_parity(&mut session, &mut partial, &mut reference, whole, "commit");

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        let object = session.document().objects()[0].clone();
        let bounds = crate::render::object_paint_bounds(&mut session.text_system, &object);
        click(&mut session, bounds.x + 3, bounds.y + 3, 1000);
        assert_parity(
            &mut session,
            &mut partial,
            &mut reference,
            whole,
            "selection",
        );
        drag(
            &mut session,
            (bounds.x + 3, bounds.y + 3),
            (bounds.x + 15, bounds.y + 25),
        );
        assert_parity(&mut session, &mut partial, &mut reference, whole, "moved");

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Eraser))
            .unwrap();
        let moved = session.document().objects()[0].clone();
        let moved_bounds = crate::render::object_paint_bounds(&mut session.text_system, &moved);
        drag(
            &mut session,
            (moved_bounds.x - 10, moved_bounds.y + 4),
            (moved_bounds.right() + 10, moved_bounds.y + 4),
        );
        assert_parity(&mut session, &mut partial, &mut reference, whole, "erased");

        session.command(AnnotationCommand::Undo).unwrap();
        assert_parity(&mut session, &mut partial, &mut reference, whole, "undo");

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Text))
            .unwrap();
        let restored = session.document().objects()[0].clone();
        let restored_bounds =
            crate::render::object_paint_bounds(&mut session.text_system, &restored);
        click(
            &mut session,
            restored_bounds.x + 2,
            restored_bounds.y + 2,
            2000,
        );
        click(
            &mut session,
            restored_bounds.x + 2,
            restored_bounds.y + 2,
            2200,
        );
        assert_parity(
            &mut session,
            &mut partial,
            &mut reference,
            whole,
            "reopened",
        );
        press(&mut session, 0xff1b, None);
        assert_parity(
            &mut session,
            &mut partial,
            &mut reference,
            whole,
            "cancelled draft",
        );

        session
            .command(AnnotationCommand::Tool(AnnotationTool::Line))
            .unwrap();
        drag(&mut session, (150, 10), (250, 10));
        assert_parity(
            &mut session,
            &mut partial,
            &mut reference,
            whole,
            "off-screen object",
        );

        session.command(AnnotationCommand::Clear).unwrap();
        assert_parity(&mut session, &mut partial, &mut reference, whole, "cleared");
    }

    #[test]
    fn unchanged_state_produces_no_damage() {
        let mut session = Session::new(&AnnotationConfig::default()).unwrap();
        session.command(AnnotationCommand::On).unwrap();
        session.ownership(Some(1));
        let mut renderer = Renderer::new(100, 100).unwrap();
        let whole = renderer.full_bounds();
        renderer.redraw(&mut session, whole).unwrap();
        session.take_damage();
        session
            .command(AnnotationCommand::Tool(AnnotationTool::Select))
            .unwrap();
        assert!(session.take_damage().is_none());
        session
            .command(AnnotationCommand::Color(crate::Preset::Next))
            .unwrap();
        assert!(session.take_damage().is_none());
    }
}
