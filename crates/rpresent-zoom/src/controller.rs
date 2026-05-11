use crate::command::ZoomCommand;

#[derive(Debug, Clone, PartialEq)]
pub struct ZoomRuntimeConfig {
    pub initial_scale: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub scroll_speed: f32,
    pub pan_friction: f32,
    pub scale_friction: f32,
    pub follow_cursor: bool,
    pub capture_target: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ZoomRuntimeMessage {
    Configure(ZoomRuntimeConfig),
    SetEnabled(bool),
    Scroll { delta: f32, pivot: (f32, f32) },
    DragStart { x: f32, y: f32 },
    DragMove { x: f32, y: f32 },
    DragEnd,
    Reset,
}

pub struct ZoomController {
    enabled: bool,
    runtime: ZoomRuntimeConfig,
}

impl ZoomController {
    pub fn from_config(config: rpresent_config::ZoomConfig) -> Self {
        Self {
            enabled: config.enabled_on_start,
            runtime: ZoomRuntimeConfig {
                initial_scale: config.initial_scale,
                min_scale: config.min_scale,
                max_scale: config.max_scale,
                scroll_speed: config.scroll_speed,
                pan_friction: config.pan_friction,
                scale_friction: config.scale_friction,
                follow_cursor: config.follow_cursor,
                capture_target: config.capture_target.clone(),
            },
        }
    }

    pub fn initial_messages(&self) -> [ZoomRuntimeMessage; 2] {
        [
            ZoomRuntimeMessage::Configure(self.runtime.clone()),
            ZoomRuntimeMessage::SetEnabled(self.enabled),
        ]
    }

    pub fn apply(&mut self, command: ZoomCommand) -> Option<ZoomRuntimeMessage> {
        match command {
            ZoomCommand::On => self.set_enabled(true),
            ZoomCommand::Off => self.set_enabled(false),
            ZoomCommand::Toggle => self.set_enabled(!self.enabled),
            ZoomCommand::Reset => self.reset(),
        }
    }

    pub fn zoom_by_scroll(&mut self, delta: f32, pivot: (f32, f32)) -> Option<ZoomRuntimeMessage> {
        if !self.enabled {
            return None;
        }

        Some(ZoomRuntimeMessage::Scroll { delta, pivot })
    }

    pub fn drag_start(&mut self, cursor: (f32, f32)) -> Option<ZoomRuntimeMessage> {
        if !self.enabled {
            return None;
        }

        Some(ZoomRuntimeMessage::DragStart {
            x: cursor.0,
            y: cursor.1,
        })
    }

    pub fn drag_move(&mut self, cursor: (f32, f32)) -> Option<ZoomRuntimeMessage> {
        if !self.enabled {
            return None;
        }

        Some(ZoomRuntimeMessage::DragMove {
            x: cursor.0,
            y: cursor.1,
        })
    }

    pub fn drag_end(&mut self) -> Option<ZoomRuntimeMessage> {
        if !self.enabled {
            return None;
        }

        Some(ZoomRuntimeMessage::DragEnd)
    }

    pub fn reset(&mut self) -> Option<ZoomRuntimeMessage> {
        Some(ZoomRuntimeMessage::Reset)
    }

    fn set_enabled(&mut self, enabled: bool) -> Option<ZoomRuntimeMessage> {
        let previous = self.enabled;
        let changed = self.enabled != enabled;
        self.enabled = enabled;
        if changed {
            tracing::debug!(from = previous, to = enabled, "zoom enabled state changed");
            Some(ZoomRuntimeMessage::SetEnabled(enabled))
        } else {
            None
        }
    }
}
