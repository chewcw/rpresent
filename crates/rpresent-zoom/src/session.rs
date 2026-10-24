use std::time::Duration;

use crate::command::ZoomCommand;
use crate::navigation::{Camera, NavigationConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomRequest {
    Command(ZoomCommand),
    Scroll { delta: f64, root: (i32, i32) },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerInput {
    Down { local: (f64, f64), time: Duration },
    Move { local: (f64, f64), time: Duration },
    Up { time: Duration },
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEffect {
    None,
    Activate,
    Close,
}

pub struct ZoomSession {
    config: NavigationConfig,
    monitor: Option<MonitorRect>,
    camera: Option<Camera>,
    dirty: bool,
}

impl ZoomSession {
    pub fn new(config: NavigationConfig) -> Self {
        Self {
            config,
            monitor: None,
            camera: None,
            dirty: false,
        }
    }

    pub fn request(&mut self, request: ZoomRequest) -> SessionEffect {
        match request {
            ZoomRequest::Command(ZoomCommand::On) if !self.is_active() => SessionEffect::Activate,
            ZoomRequest::Command(ZoomCommand::Off) if self.is_active() => SessionEffect::Close,
            ZoomRequest::Command(ZoomCommand::Toggle) => {
                if self.is_active() {
                    SessionEffect::Close
                } else {
                    SessionEffect::Activate
                }
            }
            ZoomRequest::Command(ZoomCommand::Reset) => {
                if let Some(camera) = self.camera.as_mut() {
                    camera.reset();
                    self.dirty = true;
                }
                SessionEffect::None
            }
            ZoomRequest::Scroll { delta, root } => {
                if let (Some(camera), Some(monitor)) = (&mut self.camera, self.monitor) {
                    let local = (
                        f64::from(root.0) - f64::from(monitor.x),
                        f64::from(root.1) - f64::from(monitor.y),
                    );
                    camera.scroll(delta, local);
                }
                SessionEffect::None
            }
            _ => SessionEffect::None,
        }
    }

    pub fn activated(&mut self, monitor: MonitorRect) {
        self.monitor = Some(monitor);
        self.camera = Some(Camera::new(
            self.config,
            (f64::from(monitor.width), f64::from(monitor.height)),
        ));
        self.dirty = true;
    }

    pub fn closed(&mut self) {
        self.camera = None;
        self.monitor = None;
        self.dirty = false;
    }

    pub fn pointer(&mut self, input: PointerInput) {
        let Some(camera) = self.camera.as_mut() else {
            return;
        };
        let before = camera.translation();
        let dragging_move = matches!(input, PointerInput::Move { .. }) && camera.is_dragging();
        match input {
            PointerInput::Down { local, time } => camera.pointer_down(local, time),
            PointerInput::Move { local, time } => camera.pointer_move(local, time),
            PointerInput::Up { time } => camera.pointer_up(time),
            PointerInput::Cancel => camera.cancel_motion(),
        }
        self.dirty |= dragging_move || before != camera.translation();
    }

    pub fn advance(&mut self, dt: Duration) -> bool {
        let changed = self
            .camera
            .as_mut()
            .is_some_and(|camera| camera.advance(dt));
        self.dirty |= changed;
        changed
    }

    pub fn is_active(&self) -> bool {
        self.camera.is_some()
    }

    pub fn is_animating(&self) -> bool {
        self.camera.as_ref().is_some_and(Camera::is_animating)
    }

    pub fn camera(&self) -> Option<&Camera> {
        self.camera.as_ref()
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn mark_dirty(&mut self) {
        self.dirty |= self.is_active();
    }
}
