//! Pure Rust spotlight frame compositor, mirroring Projecteur's QML overlay pipeline.
//!
//! Composition order:
//!  1. Fill the damage rectangle with the shade colour and outward fade.
//!  2. Draw the border ring.
//!  3. Draw the centre dot.
//!
//! Output pixels are stored in **BGRA** order (premultiplied), matching the X11 32-bit visual
//! selected by `x11.rs`.

use rpresent_config::SpotlightConfig;
use tiny_skia::{BlendMode, FillRule, Paint, Pixmap, Transform};

use crate::overlay::{CursorPosition, SpotlightFrame};
use crate::shape::{ShapeGeometry, draw_decorations, parse_rgb};

pub(crate) fn shade_factor(distance: f32, softness_px: u32) -> f32 {
    if softness_px == 0 {
        return if distance > 0.0 { 1.0 } else { 0.0 };
    }
    (distance / softness_px as f32).clamp(0.0, 1.0)
}

/// Build a full-surface Projecteur-style spotlight frame.
///
/// The returned [`SpotlightFrame`] stores pixels in **BGRA** premultiplied byte order.
pub fn build_projecteur_frame(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    config: &SpotlightConfig,
    radius_px: u32,
) -> SpotlightFrame {
    let (width, height) = surface_size;
    build_projecteur_frame_rect((0, 0, width, height), cursor, config, radius_px)
}

/// Build the spotlight pixels for `damage_rect` only.
///
/// `damage_rect` is `(x, y, width, height)` in surface pixels. The returned frame is exactly
/// the size of that rectangle, so the caller blits it at the rectangle origin.
pub fn build_projecteur_frame_rect(
    damage_rect: (i16, i16, u16, u16),
    cursor: CursorPosition,
    config: &SpotlightConfig,
    radius_px: u32,
) -> SpotlightFrame {
    let (rect_x, rect_y, width, height) = damage_rect;
    assert!(
        width > 0 && height > 0,
        "damage rectangle must have non-zero dimensions"
    );
    let mut pixmap = Pixmap::new(u32::from(width), u32::from(height))
        .expect("damage rectangle dimensions are non-zero");
    let transform = Transform::from_translate(-f32::from(rect_x), -f32::from(rect_y));

    if config.show_spot_shade {
        let geometry = ShapeGeometry::new(config.shape, radius_px as f32);
        if config.softness_px == 0 {
            fill_hard_shade(&mut pixmap, config);
            if let Some(geometry) = geometry
                && let Some(path) = geometry.path_at((f32::from(cursor.x), f32::from(cursor.y)))
            {
                let paint = Paint {
                    blend_mode: BlendMode::Clear,
                    ..Paint::default()
                };
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
        } else if let Some(geometry) = geometry {
            fill_soft_shade(&mut pixmap, damage_rect, cursor, config, geometry);
        } else {
            fill_hard_shade(&mut pixmap, config);
        }
    }

    draw_decorations(&mut pixmap, cursor, config, radius_px, transform);

    let mut bgra = pixmap.data().to_vec();
    for chunk in bgra.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    SpotlightFrame {
        width,
        height,
        pixels: bgra,
    }
}

fn fill_hard_shade(pixmap: &mut Pixmap, config: &SpotlightConfig) {
    let (red, green, blue) = parse_rgb(&config.shade_color);
    let alpha = (config.shade_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
    for pixel in pixmap.data_mut().chunks_exact_mut(4) {
        pixel[0] = premultiply(red, alpha);
        pixel[1] = premultiply(green, alpha);
        pixel[2] = premultiply(blue, alpha);
        pixel[3] = alpha;
    }
}

fn fill_soft_shade(
    pixmap: &mut Pixmap,
    damage_rect: (i16, i16, u16, u16),
    cursor: CursorPosition,
    config: &SpotlightConfig,
    geometry: ShapeGeometry,
) {
    let (red, green, blue) = parse_rgb(&config.shade_color);
    let opacity = config.shade_opacity.clamp(0.0, 1.0);
    let (rect_x, rect_y, width, height) = damage_rect;
    let pixels = pixmap.data_mut();
    for y in 0..height {
        for x in 0..width {
            let global_x = i32::from(rect_x) + i32::from(x);
            let global_y = i32::from(rect_y) + i32::from(y);
            let local = (
                global_x as f32 - f32::from(cursor.x),
                global_y as f32 - f32::from(cursor.y),
            );
            let factor = shade_factor(geometry.signed_distance(local), config.softness_px);
            let alpha = (opacity * factor * 255.0).round() as u8;
            let index = (y as usize * width as usize + x as usize) * 4;
            pixels[index] = premultiply(red, alpha);
            pixels[index + 1] = premultiply(green, alpha);
            pixels[index + 2] = premultiply(blue, alpha);
            pixels[index + 3] = alpha;
        }
    }
}

fn premultiply(channel: u8, alpha: u8) -> u8 {
    ((u16::from(channel) * u16::from(alpha) + 127) / 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shade_factor_has_a_hard_edge_when_softness_is_zero() {
        assert_eq!(shade_factor(-1.0, 0), 0.0);
        assert_eq!(shade_factor(0.0, 0), 0.0);
        assert_eq!(shade_factor(1.0, 0), 1.0);
    }

    #[test]
    fn shade_factor_fades_outward_over_the_requested_width() {
        assert_eq!(shade_factor(-1.0, 20), 0.0);
        assert_eq!(shade_factor(0.0, 20), 0.0);
        assert_eq!(shade_factor(10.0, 20), 0.5);
        assert_eq!(shade_factor(20.0, 20), 1.0);
    }
}
