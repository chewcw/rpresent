use std::time::Duration;

/// Minimum pan speed in screen pixels per second before inertia stops.
const PAN_VELOCITY_THRESHOLD: f64 = 0.1;
/// Minimum logarithmic zoom velocity per second before zoom motion stops.
const ZOOM_VELOCITY_THRESHOLD: f64 = 1e-6;
/// Motion interval used to estimate release velocity in screen pixels per second.
const RELEASE_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavigationConfig {
    pub initial_scale: f64,
    pub min_scale: f64,
    pub max_scale: f64,
    pub scroll_speed: f64,
    pub drag_friction: f64,
    pub scale_friction: f64,
}

impl Default for NavigationConfig {
    fn default() -> Self {
        Self {
            initial_scale: 2.0,
            min_scale: 1.0,
            max_scale: 6.0,
            scroll_speed: 0.25,
            drag_friction: 6.0,
            scale_friction: 5.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    config: NavigationConfig,
    viewport: (f64, f64),
    scale: f64,
    translation: (f64, f64),
    pan_velocity: (f64, f64),
    zoom_velocity: f64,
    zoom_anchor: (f64, f64),
    dragging: bool,
    last_point: (f64, f64),
    samples: Vec<(Duration, (f64, f64))>,
}

impl Camera {
    pub fn new(config: NavigationConfig, viewport: (f64, f64)) -> Self {
        Self {
            config,
            viewport,
            scale: config.initial_scale,
            translation: Self::centered_translation(&config, viewport),
            pan_velocity: (0.0, 0.0),
            zoom_velocity: 0.0,
            zoom_anchor: (0.0, 0.0),
            dragging: false,
            last_point: (0.0, 0.0),
            samples: Vec::new(),
        }
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn translation(&self) -> (f64, f64) {
        self.translation
    }

    pub fn pan_velocity(&self) -> (f64, f64) {
        self.pan_velocity
    }

    pub fn zoom_velocity(&self) -> f64 {
        self.zoom_velocity
    }

    /// Maps a viewport position to the screenshot coordinate beneath it.
    pub fn image_at(&self, screen: (f64, f64)) -> (f64, f64) {
        (
            (screen.0 - self.translation.0) / self.scale,
            (screen.1 - self.translation.1) / self.scale,
        )
    }

    pub fn scroll(&mut self, delta: f64, anchor: (f64, f64)) {
        if !delta.is_finite() {
            return;
        }
        self.zoom_anchor = anchor;
        if (delta > 0.0 && self.zoom_velocity < 0.0) || (delta < 0.0 && self.zoom_velocity > 0.0) {
            self.zoom_velocity = 0.0;
        }
        self.zoom_velocity += delta * self.config.scroll_speed.ln_1p() * self.config.scale_friction;
    }

    pub fn pointer_down(&mut self, point: (f64, f64), time: Duration) {
        self.dragging = true;
        self.pan_velocity = (0.0, 0.0);
        self.zoom_velocity = 0.0;
        self.last_point = point;
        self.samples.clear();
        self.samples.push((time, point));
    }

    pub fn pointer_move(&mut self, point: (f64, f64), time: Duration) {
        if !self.dragging {
            return;
        }
        let displacement = (point.0 - self.last_point.0, point.1 - self.last_point.1);
        self.translation = (
            self.translation.0 + displacement.0,
            self.translation.1 + displacement.1,
        );
        self.last_point = point;
        match self.samples.last_mut() {
            Some(last) if last.0 == time => last.1 = point,
            _ => self.samples.push((time, point)),
        }
    }

    pub fn pointer_up(&mut self, time: Duration) {
        if !self.dragging {
            return;
        }
        self.pan_velocity = self.release_velocity(time);
        self.dragging = false;
        self.samples.clear();
    }

    pub fn cancel_motion(&mut self) {
        self.dragging = false;
        self.pan_velocity = (0.0, 0.0);
        self.zoom_velocity = 0.0;
    }

    pub fn reset(&mut self) {
        self.scale = self.config.initial_scale;
        self.translation = Self::centered_translation(&self.config, self.viewport);
        self.pan_velocity = (0.0, 0.0);
        self.zoom_velocity = 0.0;
        self.zoom_anchor = (0.0, 0.0);
        self.dragging = false;
        self.last_point = (0.0, 0.0);
        self.samples.clear();
    }

    pub fn advance(&mut self, dt: Duration) -> bool {
        let dt_secs = dt.as_secs_f64();
        let mut changed = false;

        if self.pan_velocity_active() {
            let (distance, velocity) =
                integrate_damped_velocity(self.pan_velocity.0, self.config.drag_friction, dt_secs);
            let (distance_y, velocity_y) =
                integrate_damped_velocity(self.pan_velocity.1, self.config.drag_friction, dt_secs);
            self.translation = (
                self.translation.0 + distance,
                self.translation.1 + distance_y,
            );
            self.pan_velocity = (velocity, velocity_y);
            if distance != 0.0 || distance_y != 0.0 {
                changed = true;
            }
            if !self.pan_velocity_active() {
                self.pan_velocity = (0.0, 0.0);
            }
        }

        if self.zoom_velocity.abs() >= ZOOM_VELOCITY_THRESHOLD {
            let (distance, velocity) =
                integrate_damped_velocity(self.zoom_velocity, self.config.scale_friction, dt_secs);
            let unscaled = self.scale * distance.exp();
            let new_scale = unscaled.clamp(self.config.min_scale, self.config.max_scale);
            let outward = unscaled > self.config.max_scale || unscaled < self.config.min_scale;
            let old_scale = self.scale;
            self.scale = new_scale;
            if new_scale != old_scale {
                let image_anchor = (
                    (self.zoom_anchor.0 - self.translation.0) / old_scale,
                    (self.zoom_anchor.1 - self.translation.1) / old_scale,
                );
                self.translation = (
                    self.zoom_anchor.0 - new_scale * image_anchor.0,
                    self.zoom_anchor.1 - new_scale * image_anchor.1,
                );
                changed = true;
            }
            self.zoom_velocity = if outward || velocity.abs() < ZOOM_VELOCITY_THRESHOLD {
                0.0
            } else {
                velocity
            };
        } else if self.zoom_velocity != 0.0 {
            self.zoom_velocity = 0.0;
        }

        changed
    }

    pub fn is_animating(&self) -> bool {
        self.pan_velocity_active() || self.zoom_velocity.abs() >= ZOOM_VELOCITY_THRESHOLD
    }

    fn centered_translation(config: &NavigationConfig, viewport: (f64, f64)) -> (f64, f64) {
        let half = (viewport.0 / 2.0, viewport.1 / 2.0);
        (
            half.0 - config.initial_scale * half.0,
            half.1 - config.initial_scale * half.1,
        )
    }

    fn pan_velocity_active(&self) -> bool {
        let speed = (self.pan_velocity.0 * self.pan_velocity.0
            + self.pan_velocity.1 * self.pan_velocity.1)
            .sqrt();
        speed >= PAN_VELOCITY_THRESHOLD
    }

    fn release_velocity(&self, release_time: Duration) -> (f64, f64) {
        let start = release_time.saturating_sub(RELEASE_INTERVAL);
        let mut last: Option<(Duration, (f64, f64))> = None;
        let mut previous: Option<(Duration, (f64, f64))> = None;
        for sample in &self.samples {
            if sample.0 < start || sample.0 > release_time {
                continue;
            }
            previous = last;
            last = Some(*sample);
        }
        let Some((time, position)) = last else {
            return (0.0, 0.0);
        };
        let Some((previous_time, previous_position)) = previous else {
            return (0.0, 0.0);
        };
        let elapsed = (time - previous_time).as_secs_f64();
        if elapsed <= 0.0 {
            return (0.0, 0.0);
        }
        (
            (position.0 - previous_position.0) / elapsed,
            (position.1 - previous_position.1) / elapsed,
        )
    }
}

fn integrate_damped_velocity(velocity: f64, friction: f64, dt: f64) -> (f64, f64) {
    let decay = (-friction * dt).exp();
    let distance = velocity * -(-friction * dt).exp_m1() / friction;
    (distance, velocity * decay)
}
