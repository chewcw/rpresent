use crate::command::ZoomCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct ZoomState {
    pub enabled: bool,
    pub scale: f32,
    pub pivot: (f32, f32),
    pub follow_cursor: bool,
    pub capture_target: String,
}

pub struct ZoomController {
    state: ZoomState,
    initial_scale: f32,
    min_scale: f32,
    max_scale: f32,
    scroll_speed: f32,
    pan_friction: f32,
}

impl ZoomController {
    pub fn from_config(config: &rpresent_config::ZoomConfig) -> Self {
        Self {
            state: ZoomState {
                enabled: config.enabled_on_start,
                scale: config.initial_scale,
                pivot: (0.0, 0.0),
                follow_cursor: config.follow_cursor,
                capture_target: config.capture_target.clone(),
            },
            initial_scale: config.initial_scale,
            min_scale: config.min_scale,
            max_scale: config.max_scale,
            scroll_speed: config.scroll_speed,
            pan_friction: config.pan_friction,
        }
    }

    pub fn state(&self) -> ZoomState {
        self.state.clone()
    }

    pub fn apply(&mut self, command: ZoomCommand) -> bool {
        match command {
            ZoomCommand::On => self.set_enabled(true),
            ZoomCommand::Off => self.set_enabled(false),
            ZoomCommand::Toggle => self.set_enabled(!self.state.enabled),
            ZoomCommand::Reset => self.reset(),
        }
    }

    pub fn zoom_by_scroll(&mut self, delta: f32, pivot: (f32, f32)) -> bool {
        let previous_pivot = self.state.pivot;
        let previous_scale = self.state.scale;
        self.state.pivot = pivot;
        let changed = self.set_scale(self.state.scale * (1.0 + delta * self.scroll_speed));
        if previous_pivot != self.state.pivot
            || (previous_scale - self.state.scale).abs() > f32::EPSILON
        {
            tracing::trace!(
                from_pivot = ?previous_pivot,
                to_pivot = ?self.state.pivot,
                from_scale = previous_scale,
                to_scale = self.state.scale,
                delta,
                "zoom state changed from scroll"
            );
        }
        changed
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) -> bool {
        let previous_pivot = self.state.pivot;
        self.state.pivot.0 += dx * self.pan_friction;
        self.state.pivot.1 += dy * self.pan_friction;
        if previous_pivot != self.state.pivot {
            tracing::trace!(from_pivot = ?previous_pivot, to_pivot = ?self.state.pivot, dx, dy, "zoom pivot changed");
        }
        true
    }

    pub fn reset(&mut self) -> bool {
        let previous_pivot = self.state.pivot;
        let previous_scale = self.state.scale;
        self.state.scale = self.initial_scale;
        self.state.pivot = (0.0, 0.0);
        if previous_pivot != self.state.pivot
            || (previous_scale - self.state.scale).abs() > f32::EPSILON
        {
            tracing::debug!(
                from_pivot = ?previous_pivot,
                to_pivot = ?self.state.pivot,
                from_scale = previous_scale,
                to_scale = self.state.scale,
                "zoom state reset"
            );
        }
        true
    }

    fn set_enabled(&mut self, enabled: bool) -> bool {
        let previous = self.state.enabled;
        let changed = self.state.enabled != enabled;
        self.state.enabled = enabled;
        if changed {
            tracing::debug!(from = previous, to = enabled, "zoom enabled state changed");
        }
        changed
    }

    fn set_scale(&mut self, scale: f32) -> bool {
        let clamped = scale.clamp(self.min_scale, self.max_scale);
        let changed = (clamped - self.state.scale).abs() > f32::EPSILON;
        self.state.scale = clamped;
        changed
    }
}
