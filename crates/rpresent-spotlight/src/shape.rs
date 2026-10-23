use std::f32::consts::PI;

use rpresent_config::{SpotShape, SpotlightConfig};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Shader, Transform};

use crate::overlay::{CursorPosition, SpotlightFrame};

pub(crate) struct ShapeGeometry {
    shape: SpotShape,
    radius: f32,
    polygon: Option<Vec<(f32, f32)>>,
}

impl ShapeGeometry {
    pub(crate) fn new(shape: SpotShape, radius_px: f32) -> Option<Self> {
        if !radius_px.is_finite() || radius_px < 0.5 {
            return None;
        }

        let polygon = match shape {
            SpotShape::Circle | SpotShape::Square { .. } => None,
            SpotShape::Star {
                points,
                inner_radius_pct,
            } if points >= 3 => Some(star_vertices(
                radius_px,
                radius_px * inner_radius_pct as f32 / 100.0,
                points,
            )),
            SpotShape::Ngon { sides } if sides >= 3 => Some(ngon_vertices(radius_px, sides)),
            SpotShape::Star { .. } | SpotShape::Ngon { .. } => return None,
        };

        Some(Self {
            shape,
            radius: radius_px,
            polygon,
        })
    }

    pub(crate) fn path_at(&self, center: (f32, f32)) -> Option<Path> {
        let mut builder = PathBuilder::new();
        match self.shape {
            SpotShape::Circle => {
                let rect = Rect::from_xywh(
                    center.0 - self.radius,
                    center.1 - self.radius,
                    self.radius * 2.0,
                    self.radius * 2.0,
                )?;
                builder.push_oval(rect);
            }
            SpotShape::Square { corner_radius_pct } => {
                let corner_radius =
                    (self.radius * corner_radius_pct as f32 / 100.0).min(self.radius);
                add_rounded_rect(
                    &mut builder,
                    center.0 - self.radius,
                    center.1 - self.radius,
                    self.radius * 2.0,
                    self.radius * 2.0,
                    corner_radius,
                );
            }
            SpotShape::Star { .. } | SpotShape::Ngon { .. } => {
                let vertices = self.polygon.as_ref()?;
                add_polygon(&mut builder, vertices, center);
            }
        }
        builder.finish()
    }

    pub(crate) fn signed_distance(&self, local: (f32, f32)) -> f32 {
        match self.shape {
            SpotShape::Circle => local.0.hypot(local.1) - self.radius,
            SpotShape::Square { corner_radius_pct } => rounded_square_distance(
                local.0,
                local.1,
                self.radius,
                (self.radius * corner_radius_pct as f32 / 100.0).min(self.radius),
            ),
            SpotShape::Star { .. } | SpotShape::Ngon { .. } => polygon_distance(
                self.polygon.as_ref().expect("polygon shape has vertices"),
                local,
            ),
        }
    }

    pub(crate) fn extent_px(&self) -> f32 {
        self.radius
    }
}

pub(crate) fn parse_rgb(value: &str) -> (u8, u8, u8) {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 || !hex.is_ascii() {
        return (255, 255, 255);
    }

    let bytes = hex.as_bytes();
    let Some(red) = parse_hex_pair(bytes[0], bytes[1]) else {
        return (255, 255, 255);
    };
    let Some(green) = parse_hex_pair(bytes[2], bytes[3]) else {
        return (255, 255, 255);
    };
    let Some(blue) = parse_hex_pair(bytes[4], bytes[5]) else {
        return (255, 255, 255);
    };
    (red, green, blue)
}

pub(crate) fn draw_decorations(
    pixmap: &mut Pixmap,
    cursor: CursorPosition,
    config: &SpotlightConfig,
    radius_px: u32,
    transform: Transform,
) {
    let center = (f32::from(cursor.x), f32::from(cursor.y));

    if config.show_border
        && let Some(outer) = ShapeGeometry::new(config.shape, radius_px as f32)
        && let Some(inner) = ShapeGeometry::new(
            config.shape,
            radius_px as f32 * (1.0 - config.border_size_pct.min(99) as f32 / 100.0),
        )
        && let (Some(outer_path), Some(inner_path)) = (outer.path_at(center), inner.path_at(center))
    {
        let mut builder = PathBuilder::new();
        builder.push_path(&outer_path);
        builder.push_path(&inner_path);
        if let Some(path) = builder.finish() {
            let (red, green, blue) = parse_rgb(&config.border_color);
            let alpha = (config.border_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            let paint = Paint {
                shader: Shader::SolidColor(Color::from_rgba8(red, green, blue, alpha)),
                anti_alias: true,
                ..Paint::default()
            };
            pixmap.fill_path(&path, &paint, FillRule::EvenOdd, transform, None);
        }
    }

    if config.show_center_dot {
        let dot_radius = config.dot_size_px as f32 / 2.0;
        if let Some(rect) = Rect::from_xywh(
            center.0 - dot_radius,
            center.1 - dot_radius,
            dot_radius * 2.0,
            dot_radius * 2.0,
        ) {
            let mut builder = PathBuilder::new();
            builder.push_oval(rect);
            if let Some(path) = builder.finish() {
                let (red, green, blue) = parse_rgb(&config.dot_color);
                let alpha = (config.dot_opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                let paint = Paint {
                    shader: Shader::SolidColor(Color::from_rgba8(red, green, blue, alpha)),
                    anti_alias: true,
                    ..Paint::default()
                };
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
        }
    }
}

pub(crate) fn build_circle_decorations(
    radius_px: u32,
    config: &SpotlightConfig,
) -> Option<(SpotlightFrame, i16)> {
    if !config.show_border && !config.show_center_dot {
        return None;
    }

    let extent = (if config.show_border {
        radius_px as f32
    } else {
        0.0
    })
    .max(if config.show_center_dot {
        config.dot_size_px as f32 / 2.0
    } else {
        0.0
    })
    .ceil() as u16
        + 1;
    let side = extent.saturating_mul(2).saturating_add(1);
    let mut pixmap = Pixmap::new(u32::from(side), u32::from(side))?;
    let center = CursorPosition {
        x: extent as i16,
        y: extent as i16,
    };
    draw_decorations(
        &mut pixmap,
        center,
        config,
        radius_px,
        Transform::identity(),
    );

    let mut pixels = pixmap.data().to_vec();
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    Some((
        SpotlightFrame {
            width: side,
            height: side,
            pixels,
        },
        extent as i16,
    ))
}

fn parse_hex_pair(high: u8, low: u8) -> Option<u8> {
    Some(hex_digit(high)? * 16 + hex_digit(low)?)
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn add_polygon(builder: &mut PathBuilder, vertices: &[(f32, f32)], center: (f32, f32)) {
    for (index, &(x, y)) in vertices.iter().enumerate() {
        let point = (center.0 + x, center.1 + y);
        if index == 0 {
            builder.move_to(point.0, point.1);
        } else {
            builder.line_to(point.0, point.1);
        }
    }
    builder.close();
}

fn add_rounded_rect(builder: &mut PathBuilder, x: f32, y: f32, w: f32, h: f32, r: f32) {
    const K: f32 = 0.5523;
    let kr = r * K;
    builder.move_to(x + r, y);
    builder.line_to(x + w - r, y);
    builder.cubic_to(x + w - r + kr, y, x + w, y + r - kr, x + w, y + r);
    builder.line_to(x + w, y + h - r);
    builder.cubic_to(
        x + w,
        y + h - r + kr,
        x + w - r + kr,
        y + h,
        x + w - r,
        y + h,
    );
    builder.line_to(x + r, y + h);
    builder.cubic_to(x + r - kr, y + h, x, y + h - r + kr, x, y + h - r);
    builder.line_to(x, y + r);
    builder.cubic_to(x, y + r - kr, x + r - kr, y, x + r, y);
    builder.close();
}

fn star_vertices(outer_radius: f32, inner_radius: f32, points: u32) -> Vec<(f32, f32)> {
    let step = PI / points as f32;
    let start = -PI / 2.0;
    (0..points as usize * 2)
        .map(|index| {
            let angle = start + index as f32 * step;
            let radius = if index % 2 == 0 {
                outer_radius
            } else {
                inner_radius
            };
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect()
}

fn ngon_vertices(radius: f32, sides: u32) -> Vec<(f32, f32)> {
    let step = 2.0 * PI / sides as f32;
    let start = -PI / 2.0;
    (0..sides as usize)
        .map(|index| {
            let angle = start + index as f32 * step;
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect()
}

fn rounded_square_distance(x: f32, y: f32, radius: f32, corner_radius: f32) -> f32 {
    let qx = x.abs() - (radius - corner_radius);
    let qy = y.abs() - (radius - corner_radius);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - corner_radius
}

fn polygon_distance(vertices: &[(f32, f32)], p: (f32, f32)) -> f32 {
    let mut nearest = f32::INFINITY;
    let mut winding = 0_i32;
    for index in 0..vertices.len() {
        let a = vertices[index];
        let b = vertices[(index + 1) % vertices.len()];
        let v = (b.0 - a.0, b.1 - a.1);
        let w = (p.0 - a.0, p.1 - a.1);
        let length_squared = v.0 * v.0 + v.1 * v.1;
        let t = if length_squared == 0.0 {
            0.0
        } else {
            ((w.0 * v.0 + w.1 * v.1) / length_squared).clamp(0.0, 1.0)
        };
        nearest = nearest.min((w.0 - t * v.0).hypot(w.1 - t * v.1));
        let cross = v.0 * w.1 - v.1 * w.0;
        if a.1 <= p.1 && b.1 > p.1 && cross > 0.0 {
            winding += 1;
        } else if a.1 > p.1 && b.1 <= p.1 && cross < 0.0 {
            winding -= 1;
        }
    }
    if winding == 0 { nearest } else { -nearest }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rgb_accepts_hash_and_plain_hex() {
        assert_eq!(parse_rgb("#bada55"), (186, 218, 85));
        assert_eq!(parse_rgb("102030"), (16, 32, 48));
    }

    #[test]
    fn parse_rgb_returns_white_for_malformed_or_non_ascii_input() {
        assert_eq!(parse_rgb("#12"), (255, 255, 255));
        assert_eq!(parse_rgb("#gggggg"), (255, 255, 255));
        assert_eq!(parse_rgb("éééééé"), (255, 255, 255));
    }

    #[test]
    fn polygon_distance_is_negative_inside_and_positive_outside() {
        let geometry = ShapeGeometry::new(SpotShape::Ngon { sides: 4 }, 10.0).unwrap();
        assert!(geometry.signed_distance((0.0, 0.0)) < 0.0);
        assert!(geometry.signed_distance((20.0, 0.0)) > 0.0);
    }

    #[test]
    fn circle_decorations_use_an_integer_center_offset() {
        let config = SpotlightConfig::default();
        let (frame, offset) = build_circle_decorations(40, &config).unwrap();
        assert_eq!(offset, 41);
        assert_eq!((frame.width, frame.height), (83, 83));
        assert!(
            build_circle_decorations(
                40,
                &SpotlightConfig {
                    show_border: false,
                    show_center_dot: false,
                    ..config
                }
            )
            .is_none()
        );
    }
}
