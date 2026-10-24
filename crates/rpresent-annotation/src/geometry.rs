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
