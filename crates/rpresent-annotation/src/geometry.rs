use crate::document::Point;

/// A screen-space rectangle in root pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }

    pub fn bottom(&self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }

    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x && point.y >= self.y && point.x < self.right() && point.y < self.bottom()
    }

    pub fn union(&self, other: &Rect) -> Rect {
        if self.width == 0 || self.height == 0 {
            return *other;
        }
        if other.width == 0 || other.height == 0 {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let right = self.right().max(other.right());
        let bottom = self.bottom().max(other.bottom());
        Rect {
            x,
            y,
            width: (right - x).max(0) as u32,
            height: (bottom - y).max(0) as u32,
        }
    }

    /// Grow the rectangle by `amount` pixels in every direction.
    pub fn padded(&self, amount: i32) -> Rect {
        Rect {
            x: self.x - amount,
            y: self.y - amount,
            width: self.width.saturating_add(amount.max(0) as u32 * 2),
            height: self.height.saturating_add(amount.max(0) as u32 * 2),
        }
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return None;
        }
        Some(Rect {
            x,
            y,
            width: (right - x) as u32,
            height: (bottom - y) as u32,
        })
    }

    pub(crate) fn edges(&self) -> [(Point, Point); 4] {
        let right = self.right();
        let bottom = self.bottom();
        [
            (
                Point {
                    x: self.x,
                    y: self.y,
                },
                Point {
                    x: right,
                    y: self.y,
                },
            ),
            (
                Point {
                    x: right,
                    y: self.y,
                },
                Point {
                    x: right,
                    y: bottom,
                },
            ),
            (
                Point {
                    x: right,
                    y: bottom,
                },
                Point {
                    x: self.x,
                    y: bottom,
                },
            ),
            (
                Point {
                    x: self.x,
                    y: bottom,
                },
                Point {
                    x: self.x,
                    y: self.y,
                },
            ),
        ]
    }
}

/// The four corners of a rectangle spanned by the origin and `end`, in
/// winding order and closed by repeating the first corner. Both deltas are
/// relative to the origin and may be negative.
pub(crate) fn rect_outline(end: Point) -> [Point; 5] {
    let left = 0.min(end.x);
    let right = 0.max(end.x);
    let top = 0.min(end.y);
    let bottom = 0.max(end.y);
    [
        Point { x: left, y: top },
        Point { x: right, y: top },
        Point {
            x: right,
            y: bottom,
        },
        Point { x: left, y: bottom },
        Point { x: left, y: top },
    ]
}

/// Tail, tip, and two barb endpoints for an arrow whose tip is `end`.
/// Points are relative to the arrow origin. The head is an open V whose
/// length scales with the stroke width.
pub(crate) fn arrow_outline(end: Point, width_px: u32) -> [Point; 4] {
    let tail = Point::default();
    let length = (end.x as f64).hypot(end.y as f64);
    if length == 0.0 {
        return [tail; 4];
    }
    let head_length = ((width_px as f64 * 4.0).max(12.0)).min(length / 2.0);
    let angle = (end.y as f64).atan2(end.x as f64) + std::f64::consts::PI;
    let barb = |rotation: f64| {
        let direction = angle + rotation;
        Point {
            x: end.x + (head_length * direction.cos()).round() as i32,
            y: end.y + (head_length * direction.sin()).round() as i32,
        }
    };
    [
        tail,
        end,
        barb(std::f64::consts::FRAC_PI_6),
        barb(-std::f64::consts::FRAC_PI_6),
    ]
}

fn distance(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    (ax - bx).hypot(ay - by)
}

/// Shortest distance from a point to a line segment.
pub(crate) fn point_segment_distance(point: Point, start: Point, end: Point) -> f64 {
    let (px, py) = (point.x as f64, point.y as f64);
    let (ax, ay) = (start.x as f64, start.y as f64);
    let (bx, by) = (end.x as f64, end.y as f64);
    let dx = bx - ax;
    let dy = by - ay;
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 {
        return distance(px, py, ax, ay);
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0);
    distance(px, py, ax + t * dx, ay + t * dy)
}

fn cross(ox: f64, oy: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    (ax - ox) * (by - oy) - (ay - oy) * (bx - ox)
}

fn segments_intersect(a1: Point, a2: Point, b1: Point, b2: Point) -> bool {
    let (ax1, ay1) = (a1.x as f64, a1.y as f64);
    let (ax2, ay2) = (a2.x as f64, a2.y as f64);
    let (bx1, by1) = (b1.x as f64, b1.y as f64);
    let (bx2, by2) = (b2.x as f64, b2.y as f64);
    let d1 = cross(bx1, by1, bx2, by2, ax1, ay1);
    let d2 = cross(bx1, by1, bx2, by2, ax2, ay2);
    let d3 = cross(ax1, ay1, ax2, ay2, bx1, by1);
    let d4 = cross(ax1, ay1, ax2, ay2, bx2, by2);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    let on_segment = |px: f64, py: f64, qx: f64, qy: f64, rx: f64, ry: f64| {
        rx <= px.max(qx) && rx >= px.min(qx) && ry <= py.max(qy) && ry >= py.min(qy)
    };
    (d1 == 0.0 && on_segment(bx1, by1, bx2, by2, ax1, ay1))
        || (d2 == 0.0 && on_segment(bx1, by1, bx2, by2, ax2, ay2))
        || (d3 == 0.0 && on_segment(ax1, ay1, ax2, ay2, bx1, by1))
        || (d4 == 0.0 && on_segment(ax1, ay1, ax2, ay2, bx2, by2))
}

/// Shortest distance between two line segments.
pub(crate) fn segment_segment_distance(a1: Point, a2: Point, b1: Point, b2: Point) -> f64 {
    if segments_intersect(a1, a2, b1, b2) {
        return 0.0;
    }
    point_segment_distance(a1, b1, b2)
        .min(point_segment_distance(a2, b1, b2))
        .min(point_segment_distance(b1, a1, a2))
        .min(point_segment_distance(b2, a1, a2))
}

/// Shortest distance from a point to a rectangle; zero inside.
pub(crate) fn point_rect_distance(point: Point, rect: &Rect) -> f64 {
    if rect.contains(point) {
        return 0.0;
    }
    rect.edges()
        .map(|(start, end)| point_segment_distance(point, start, end))
        .into_iter()
        .fold(f64::INFINITY, f64::min)
}

/// Shortest distance from a segment to a rectangle; zero on any overlap.
pub(crate) fn segment_rect_distance(start: Point, end: Point, rect: &Rect) -> f64 {
    if rect.contains(start) || rect.contains(end) {
        return 0.0;
    }
    rect.edges()
        .map(|(edge_start, edge_end)| segment_segment_distance(start, end, edge_start, edge_end))
        .into_iter()
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance(a: Point, b: Point) -> f64 {
        ((a.x - b.x) as f64).hypot((a.y - b.y) as f64)
    }

    #[test]
    fn arrow_outline_places_tail_tip_and_barbs() {
        let end = Point { x: 80, y: 0 };
        let [tail, tip, barb_a, barb_b] = arrow_outline(end, 6);
        assert_eq!(tail, Point::default());
        assert_eq!(tip, end);
        assert!(barb_a.x < tip.x);
        assert!(barb_b.x < tip.x);
        assert!(barb_a.y < tip.y);
        assert!(barb_b.y > tip.y);
    }

    #[test]
    fn arrow_outline_scales_the_head_with_stroke_width() {
        let end = Point { x: 200, y: 0 };
        let thin = arrow_outline(end, 2);
        let thick = arrow_outline(end, 10);
        assert!(distance(thick[2], end) > distance(thin[2], end));
    }

    #[test]
    fn arrow_outline_caps_a_short_head_at_half_the_shaft() {
        let end = Point { x: 10, y: 0 };
        let [_, tip, barb, _] = arrow_outline(end, 10);
        assert!((distance(barb, tip) - 5.0).abs() < 0.5);
    }

    #[test]
    fn arrow_outline_collapses_at_zero_length() {
        assert_eq!(
            arrow_outline(Point::default(), 6),
            [Point::default(); 4]
        );
    }
}
