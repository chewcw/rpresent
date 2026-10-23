//! Pure Rust spotlight frame compositor, mirroring Projecteur's QML overlay pipeline.
//!
//! Composition order (identical to Projecteur's QML scene):
//!  1. Fill the entire surface with the shade colour (if `show_spot_shade`).
//!  2. Cut a transparent hole at the outer spotlight shape (if `show_spot_shade`).
//!  3. Draw the border ring — outer shape minus inner shape — via EvenOdd fill (if `show_border`).
//!  4. Draw the centre dot (if `show_center_dot`).
//!
//! Output pixels are stored in **BGRA** order (premultiplied), matching the X11 32-bit visual
//! selected by `x11.rs` (`blue_shift=0, green_shift=8, red_shift=16, alpha_shift=24`).

use rpresent_config::{SpotShape, SpotlightConfig};
use tiny_skia::{BlendMode, Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Shader, Transform};

use crate::overlay::{CursorPosition, SpotlightFrame};

/// Minimum spotlight radius in pixels, matching Projecteur's lower bound.
const MIN_SPOT_RADIUS: f32 = 25.0;

// ────────────────────────────────────────────────────────────── helpers ──

/// Compute spotlight radius from screen height and the percentage setting.
/// Formula: `max(floor(height * pct / 100), MIN_SPOT_RADIUS)`.
fn spot_radius_px(surface_height: u16, spot_size_pct: u32) -> f32 {
    (surface_height as f32 * spot_size_pct as f32 / 100.0)
        .floor()
        .max(MIN_SPOT_RADIUS)
}

/// Parse `#RRGGBB` to `(r, g, b)`. Returns white on malformed input.
fn parse_hex(hex: &str) -> (u8, u8, u8) {
    let s = hex.trim_start_matches('#');
    if s.len() < 6 {
        return (255, 255, 255);
    }
    let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(255);
    let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(255);
    let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(255);
    (r, g, b)
}

/// Add the outline for `shape` centred at `(cx, cy)` with outer `radius` to `pb`.
/// Returns `false` and adds nothing if the radius is below the sub-pixel threshold.
fn add_shape(pb: &mut PathBuilder, shape: SpotShape, cx: f32, cy: f32, radius: f32) -> bool {
    if radius < 0.5 {
        return false;
    }
    match shape {
        SpotShape::Circle => {
            let Some(rect) = Rect::from_xywh(cx - radius, cy - radius, radius * 2.0, radius * 2.0)
            else {
                return false;
            };
            pb.push_oval(rect);
        }
        SpotShape::Square { corner_radius_pct } => {
            let cr = (radius * corner_radius_pct as f32 / 100.0).min(radius);
            add_rounded_rect(pb, cx - radius, cy - radius, radius * 2.0, radius * 2.0, cr);
        }
        SpotShape::Star {
            points,
            inner_radius_pct,
        } => {
            if points < 3 {
                return false;
            }
            let inner_r = radius * inner_radius_pct as f32 / 100.0;
            add_star(pb, cx, cy, radius, inner_r, points);
        }
        SpotShape::Ngon { sides } => {
            if sides < 3 {
                return false;
            }
            add_ngon(pb, cx, cy, radius, sides);
        }
    }
    true
}

/// Add a rounded rectangle to `pb`.  Corner arcs use the cubic-bezier kappa approximation.
fn add_rounded_rect(pb: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    // kappa = (4/3) * tan(π/8) ≈ 0.5523 gives a good circular-arc approximation.
    const K: f32 = 0.5523;
    let kr = r * K;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + kr, y, x + w, y + r - kr, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(
        x + w,
        y + h - r + kr,
        x + w - r + kr,
        y + h,
        x + w - r,
        y + h,
    );
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - kr, y + h, x, y + h - r + kr, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - kr, x + r - kr, y, x + r, y);
    pb.close();
}

/// Add a star polygon with `points` outer tips to `pb`, pointing up.
fn add_star(pb: &mut PathBuilder, cx: f32, cy: f32, outer_r: f32, inner_r: f32, points: u32) {
    use std::f32::consts::PI;
    let n = points as usize;
    let step = PI / n as f32; // alternates outer / inner every half-step
    let start = -PI / 2.0; // first tip points up
    for i in 0..n * 2 {
        let angle = start + i as f32 * step;
        let r = if i % 2 == 0 { outer_r } else { inner_r };
        let (x, y) = (cx + r * angle.cos(), cy + r * angle.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
}

/// Add a regular N-gon to `pb`, with the first vertex pointing up.
fn add_ngon(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, sides: u32) {
    use std::f32::consts::PI;
    let n = sides as usize;
    let step = 2.0 * PI / n as f32;
    let start = -PI / 2.0;
    for i in 0..n {
        let angle = start + i as f32 * step;
        let (x, y) = (cx + r * angle.cos(), cy + r * angle.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
}

// ──────────────────────────────────────────────────── main entry point ──

/// Build a Projecteur-style spotlight frame.
///
/// The returned [`SpotlightFrame`] stores pixels in **BGRA** premultiplied byte order, matching
/// the X11 32-bit visual (`blue_shift=0, red_shift=16, alpha_shift=24`).
pub fn build_projecteur_frame(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    config: &SpotlightConfig,
) -> SpotlightFrame {
    let (width, height) = surface_size;
    let mut pixmap =
        Pixmap::new(width as u32, height as u32).expect("surface dimensions are non-zero");

    let cx = cursor.x as f32;
    let cy = cursor.y as f32;
    let radius = spot_radius_px(height, config.spot_size_pct);
    let transform = Transform::identity();

    // ── 1. Shade layer ────────────────────────────────────────────────────
    if config.show_spot_shade {
        let (sr, sg, sb) = parse_hex(&config.shade_color);
        let a = (config.shade_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        pixmap.fill(Color::from_rgba8(sr, sg, sb, a));

        // Cut the transparent spotlight hole at the outer shape.
        let mut pb = PathBuilder::new();
        if add_shape(&mut pb, config.shape, cx, cy, radius)
            && let Some(path) = pb.finish()
        {
            let paint = Paint {
                blend_mode: BlendMode::Clear,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        }
    }

    // ── 2. Border ring ────────────────────────────────────────────────────
    if config.show_border {
        let inner_r = radius * (1.0 - config.border_size_pct.min(99) as f32 / 100.0);
        let mut pb = PathBuilder::new();
        // Two concentric sub-paths + EvenOdd fill rule = ring between them.
        let has_outer = add_shape(&mut pb, config.shape, cx, cy, radius);
        let has_inner = add_shape(&mut pb, config.shape, cx, cy, inner_r);
        if has_outer
            && has_inner
            && let Some(path) = pb.finish()
        {
            let (br, bg, bb) = parse_hex(&config.border_color);
            let a = (config.border_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            let paint = Paint {
                shader: Shader::SolidColor(Color::from_rgba8(br, bg, bb, a)),
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
        }
    }

    // ── 3. Centre dot ─────────────────────────────────────────────────────
    if config.show_center_dot {
        let dot_r = config.dot_size_px as f32 / 2.0;
        if let Some(rect) = Rect::from_xywh(cx - dot_r, cy - dot_r, dot_r * 2.0, dot_r * 2.0) {
            let mut pb = PathBuilder::new();
            pb.push_oval(rect);
            if let Some(path) = pb.finish() {
                let (dr, dg, db) = parse_hex(&config.dot_color);
                let a = (config.dot_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                let paint = Paint {
                    shader: Shader::SolidColor(Color::from_rgba8(dr, dg, db, a)),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
        }
    }

    // ── Emit BGRA (swap R↔B from tiny-skia's RGBA) ───────────────────────
    let mut bgra = pixmap.data().to_vec();
    for chunk in bgra.chunks_exact_mut(4) {
        chunk.swap(0, 2); // [R, G, B, A] → [B, G, R, A]
    }

    SpotlightFrame {
        width,
        height,
        pixels: bgra,
    }
}
