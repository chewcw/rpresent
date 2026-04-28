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
    pub fn from_config(config: rpresent_config::ZoomConfig) -> Self {
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
        self.state.pivot = pivot;
        self.set_scale(self.state.scale * (1.0 + delta * self.scroll_speed))
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) -> bool {
        self.state.pivot.0 += dx * self.pan_friction;
        self.state.pivot.1 += dy * self.pan_friction;
        true
    }

    pub fn reset(&mut self) -> bool {
        self.state.scale = self.initial_scale;
        self.state.pivot = (0.0, 0.0);
        true
    }

    fn set_enabled(&mut self, enabled: bool) -> bool {
        let changed = self.state.enabled != enabled;
        self.state.enabled = enabled;
        changed
    }

    fn set_scale(&mut self, scale: f32) -> bool {
        let clamped = scale.clamp(self.min_scale, self.max_scale);
        let changed = (clamped - self.state.scale).abs() > f32::EPSILON;
        self.state.scale = clamped;
        changed
    }
}
