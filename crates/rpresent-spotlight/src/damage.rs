//! Damage-rectangle geometry for partial spotlight redraws.

use crate::overlay::CursorPosition;
use crate::shape::ShapeGeometry;

/// A changed region as `(x, y, width, height)` in surface pixels.
pub(crate) type DamageRect = (i16, i16, u16, u16);

/// Return the square that contains the spotlight at `cursor`.
///
/// `padding_px` covers the antialiased pixels that tiny-skia writes just
/// outside the geometric radius.
pub(crate) fn spotlight_bounds(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    radius_px: f32,
    padding_px: i32,
) -> DamageRect {
    let radius = radius_px.ceil() as i32 + padding_px;
    let cx = i32::from(cursor.x);
    let cy = i32::from(cursor.y);
    clamp_rect(
        cx - radius,
        cy - radius,
        cx + radius,
        cy + radius,
        surface_size,
    )
}

/// Return the smallest rectangle that contains `a` and `b`.
pub(crate) fn union(a: DamageRect, b: DamageRect, surface_size: (u16, u16)) -> DamageRect {
    if is_empty(a) {
        return b;
    }
    if is_empty(b) {
        return a;
    }
    let (ax0, ay0, ax1, ay1) = corners(a);
    let (bx0, by0, bx1, by1) = corners(b);
    clamp_rect(
        ax0.min(bx0),
        ay0.min(by0),
        ax1.max(bx1),
        ay1.max(by1),
        surface_size,
    )
}

/// Return padded bounds for every painted component, including the center dot.
pub(crate) fn paint_bounds(
    surface_size: (u16, u16),
    cursor: CursorPosition,
    config: &rpresent_config::SpotlightConfig,
    radius_px: u32,
) -> DamageRect {
    let shape_extent = if config.show_spot_shade {
        let Some(geometry) = ShapeGeometry::new(config.shape, radius_px as f32) else {
            return (0, 0, surface_size.0, surface_size.1);
        };
        geometry.extent_px() + config.softness_px as f32
    } else {
        ShapeGeometry::new(config.shape, radius_px as f32)
            .map(|geometry| geometry.extent_px())
            .unwrap_or(radius_px as f32)
    };
    let dot_radius = if config.show_center_dot {
        config.dot_size_px as f32 / 2.0
    } else {
        0.0
    };
    spotlight_bounds(surface_size, cursor, shape_extent.max(dot_radius), 1)
}

fn is_empty(rect: DamageRect) -> bool {
    rect.2 == 0 || rect.3 == 0
}

fn corners(rect: DamageRect) -> (i32, i32, i32, i32) {
    let x0 = i32::from(rect.0);
    let y0 = i32::from(rect.1);
    (x0, y0, x0 + i32::from(rect.2), y0 + i32::from(rect.3))
}

fn clamp_rect(x0: i32, y0: i32, x1: i32, y1: i32, surface_size: (u16, u16)) -> DamageRect {
    let max_x = i32::from(surface_size.0);
    let max_y = i32::from(surface_size.1);
    let x0 = x0.clamp(0, max_x);
    let y0 = y0.clamp(0, max_y);
    let x1 = x1.clamp(0, max_x);
    let y1 = y1.clamp(0, max_y);
    if x1 <= x0 || y1 <= y0 {
        return (x0 as i16, y0 as i16, 0, 0);
    }
    (x0 as i16, y0 as i16, (x1 - x0) as u16, (y1 - y0) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE: (u16, u16) = (400, 300);

    #[test]
    fn bounds_cover_the_spotlight_and_its_antialiased_edge() {
        let rect = spotlight_bounds(SURFACE, CursorPosition { x: 200, y: 150 }, 120.0, 1);
        assert_eq!(rect, (79, 29, 242, 242));
    }

    #[test]
    fn bounds_clamp_at_the_surface_edge() {
        let rect = spotlight_bounds(SURFACE, CursorPosition { x: 0, y: 0 }, 120.0, 1);
        assert_eq!(rect, (0, 0, 121, 121));
    }

    #[test]
    fn union_contains_both_rectangles() {
        let a = (10, 10, 20, 20);
        let b = (25, 50, 30, 10);
        assert_eq!(union(a, b, SURFACE), (10, 10, 45, 50));
    }

    #[test]
    fn union_clamps_to_the_surface() {
        let a = (380, 280, 30, 30);
        let b = (10, 10, 20, 20);
        assert_eq!(union(a, b, SURFACE), (10, 10, 390, 290));
    }

    #[test]
    fn union_ignores_empty_rectangles() {
        let a = (10, 10, 20, 20);
        assert_eq!(union(a, (0, 0, 0, 0), SURFACE), a);
        assert_eq!(union((0, 0, 0, 0), a, SURFACE), a);
    }

    #[test]
    fn movement_bounds_include_a_dot_larger_than_the_spotlight() {
        let config = rpresent_config::SpotlightConfig {
            dot_size_px: 100,
            shape: rpresent_config::SpotShape::Square {
                corner_radius_pct: 0,
            },
            softness_px: 0,
            ..Default::default()
        };
        let old = CursorPosition { x: 200, y: 150 };
        let new = CursorPosition { x: 201, y: 150 };
        let rect = union(
            paint_bounds(SURFACE, old, &config, 25),
            paint_bounds(SURFACE, new, &config, 25),
            SURFACE,
        );
        let previous = crate::render::build_projecteur_frame(SURFACE, old, &config, 25);
        let next = crate::render::build_projecteur_frame(SURFACE, new, &config, 25);
        let (x0, y0, width, height) = rect;
        for y in 0..SURFACE.1 {
            for x in 0..SURFACE.0 {
                let inside = i32::from(x) >= i32::from(x0)
                    && i32::from(x) < i32::from(x0) + i32::from(width)
                    && i32::from(y) >= i32::from(y0)
                    && i32::from(y) < i32::from(y0) + i32::from(height);
                if !inside {
                    assert_eq!(
                        previous.bgra_at(x, y),
                        next.bgra_at(x, y),
                        "changed pixel ({x}, {y}) is outside damage {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn hidden_dot_does_not_expand_the_paint_bounds() {
        let config = rpresent_config::SpotlightConfig {
            dot_size_px: 100,
            show_center_dot: false,
            softness_px: 0,
            ..Default::default()
        };
        assert_eq!(
            paint_bounds(SURFACE, CursorPosition { x: 200, y: 150 }, &config, 25),
            (174, 124, 52, 52)
        );
    }

    #[test]
    fn paint_bounds_include_the_visible_shade_fade() {
        let config = rpresent_config::SpotlightConfig {
            show_border: false,
            show_center_dot: false,
            softness_px: 20,
            ..Default::default()
        };
        assert_eq!(
            paint_bounds(SURFACE, CursorPosition { x: 200, y: 150 }, &config, 40),
            (139, 89, 122, 122)
        );
    }

    #[test]
    fn paint_bounds_do_not_include_softness_when_shade_is_hidden() {
        let config = rpresent_config::SpotlightConfig {
            show_spot_shade: false,
            show_border: false,
            show_center_dot: false,
            softness_px: 80,
            ..Default::default()
        };
        assert_eq!(
            paint_bounds(SURFACE, CursorPosition { x: 200, y: 150 }, &config, 40),
            (159, 109, 82, 82)
        );
    }
}
