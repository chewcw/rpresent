#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NavigationConfig {
    pub min_scale: f32,
    pub max_scale: f32,
    pub drag_friction: f32,
    pub scale_friction: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MouseState {
    pub drag: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: (f32, f32),
    pub velocity: (f32, f32),
    pub scale: f32,
    pub delta_scale: f32,
    pub scale_pivot: (f32, f32),
}

impl Camera {
    pub fn new(scale: f32, position: (f32, f32)) -> Self {
        Self {
            position,
            velocity: (0.0, 0.0),
            scale,
            delta_scale: 0.0,
            scale_pivot: position,
        }
    }

    pub fn world_of_screen(
        &self,
        screen: (f32, f32),
        viewport: (f32, f32),
    ) -> (f32, f32) {
        let center = (viewport.0 * 0.5, viewport.1 * 0.5);
        (
            self.position.0 + (screen.0 - center.0) / self.scale.max(1e-6),
            self.position.1 + (screen.1 - center.1) / self.scale.max(1e-6),
        )
    }

    pub fn update(
        &mut self,
        cfg: &NavigationConfig,
        dt: f32,
        mouse: &MouseState,
        viewport: (f32, f32),
    ) {
        let center = (viewport.0 * 0.5, viewport.1 * 0.5);

        if self.delta_scale.abs() > 1e-4 {
            let safe_scale = self.scale.max(1e-6);
            let p0 = (
                (self.scale_pivot.0 - center.0) / safe_scale,
                (self.scale_pivot.1 - center.1) / safe_scale,
            );

            self.scale = (self.scale + self.delta_scale * dt).clamp(cfg.min_scale, cfg.max_scale);

            let p1 = (
                (self.scale_pivot.0 - center.0) / self.scale.max(1e-6),
                (self.scale_pivot.1 - center.1) / self.scale.max(1e-6),
            );

            self.position.0 += p0.0 - p1.0;
            self.position.1 += p0.1 - p1.1;

            self.delta_scale *= (-cfg.scale_friction * dt).exp();
            if self.delta_scale.abs() < 1e-4 {
                self.delta_scale = 0.0;
            }
        }

        let speed_sq = self.velocity.0 * self.velocity.0 + self.velocity.1 * self.velocity.1;
        let velocity_threshold_sq = 1e-5;
        if !mouse.drag && speed_sq > velocity_threshold_sq {
            self.position.0 += self.velocity.0 * dt;
            self.position.1 += self.velocity.1 * dt;
            let damping = (-cfg.drag_friction * dt).exp();
            self.velocity.0 *= damping;
            self.velocity.1 *= damping;
        }
    }

    pub fn drag_move(&mut self, from: (f32, f32), to: (f32, f32)) {
        let dx = to.0 - from.0;
        let dy = to.1 - from.1;
        let scale = self.scale.max(1e-6);
        let world_dx = -dx / scale;
        let world_dy = -dy / scale;
        self.position.0 += world_dx;
        self.position.1 += world_dy;
        self.velocity.0 = world_dx * 60.0;
        self.velocity.1 = world_dy * 60.0;
    }
}

pub fn apply_follow_cursor(
    camera: &mut Camera,
    pointer: (f32, f32),
    follow_cursor: bool,
    dragging: bool,
) {
    if !follow_cursor {
        return;
    }

    camera.scale_pivot = pointer;
    if !dragging {
        camera.position = pointer;
        camera.velocity = (0.0, 0.0);
    }
}
