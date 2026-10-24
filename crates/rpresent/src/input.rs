use rpresent_config::ShortcutConfig;

use std::time::Duration;

use crate::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};
use crate::native_input::NativeEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Spotlight,
    Annotation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputBindings {
    pub spotlight_mode: String,
    pub annotation_mode: String,
    pub zoom_mode: String,
    pub wheel_modifier: String,
}

impl From<&ShortcutConfig> for InputBindings {
    fn from(config: &ShortcutConfig) -> Self {
        Self {
            spotlight_mode: config.spotlight_mode.clone(),
            annotation_mode: config.annotation_mode.clone(),
            zoom_mode: config.zoom_mode.clone(),
            wheel_modifier: config.wheel_modifier.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    SpotlightWheel {
        delta: f32,
    },
    ZoomWheel {
        delta: f32,
        cursor_x: i16,
        cursor_y: i16,
    },
}

#[derive(Debug)]
pub enum AppEvent {
    Command(crate::command::AppCommand),
    Input(InputAction),
    Native(NativeEvent),
    SpotlightRuntime(rpresent_spotlight::SpotlightEvent),
    ZoomRuntime(rpresent_zoom::ZoomEvent),
    IpcFailed(String),
}

/// Feature facts the input coordinator needs. Callers keep requested activation
/// separate from these runtime facts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InputStatus {
    pub zoom_ready: bool,
    pub zoom_active: bool,
    pub annotation_enabled: bool,
    pub annotation_ready: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PointerOwner {
    #[default]
    Desktop,
    Zoom,
    Annotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPointer {
    Down { root: (i32, i32), time: Duration },
    Move { root: (i32, i32), time: Duration },
    Up { root: (i32, i32), time: Duration },
}

pub fn preferred_pointer_owner(status: InputStatus) -> PointerOwner {
    if status.annotation_enabled && status.annotation_ready {
        PointerOwner::Annotation
    } else if status.zoom_ready {
        PointerOwner::Zoom
    } else {
        PointerOwner::Desktop
    }
}

/// Decides which layer receives a left-button gesture and routes the configured
/// wheel gesture. The native thread never makes these decisions.
pub struct InputCoordinator {
    router: InputRouter,
    status: InputStatus,
    owner: PointerOwner,
    epoch: u64,
    button_down: bool,
    cancelled: bool,
}

impl InputCoordinator {
    pub fn new(bindings: InputBindings) -> Self {
        Self {
            router: InputRouter::new(bindings, ActiveMode::Spotlight),
            status: InputStatus::default(),
            owner: PointerOwner::Desktop,
            epoch: 0,
            button_down: false,
            cancelled: false,
        }
    }

    pub fn set_status(&mut self, status: InputStatus) {
        self.status = status;
    }

    pub fn preferred_owner(&self) -> PointerOwner {
        preferred_pointer_owner(self.status)
    }

    /// Record a confirmed native ownership acquisition. Only call this after the
    /// native grab succeeded; the epoch identifies events taken while it holds.
    pub fn confirm_owner(&mut self, owner: PointerOwner, epoch: u64) {
        self.owner = owner;
        self.epoch = epoch;
        self.button_down = false;
        self.cancelled = false;
    }

    /// Whether `owner` currently holds the confirmed epoch and still accepts
    /// input. Local keyboard events use the same confirmation as the pointer.
    pub fn owns_epoch(&self, owner: PointerOwner, epoch: u64) -> bool {
        self.owner == owner && self.epoch == epoch && self.owner_accepts_gestures()
    }

    /// Drop confirmed ownership before the native layer changes its grab.
    /// Events stay rejected until another native confirmation arrives.
    pub fn revoke_owner(&mut self) {
        if self.button_down && self.owner != PointerOwner::Desktop {
            self.cancelled = true;
        }
        self.owner = PointerOwner::Desktop;
        self.button_down = false;
    }

    /// Cancel the in-flight gesture, if any. Returns the owner that lost it.
    pub fn cancel_gesture(&mut self) -> Option<PointerOwner> {
        let was_active = self.button_down && self.owner != PointerOwner::Desktop;
        self.button_down = false;
        if was_active {
            self.cancelled = true;
            Some(self.owner)
        } else {
            None
        }
    }

    pub fn handle_pointer(
        &mut self,
        epoch: u64,
        input: RawPointer,
    ) -> Option<(PointerOwner, RawPointer)> {
        if epoch != self.epoch {
            return None;
        }

        match input {
            RawPointer::Down { .. } => {
                self.cancelled = false;
                if self.button_down || !self.owner_accepts_gestures() {
                    return None;
                }
                self.button_down = true;
                Some((self.owner, input))
            }
            RawPointer::Move { .. } => {
                if self.cancelled || !self.button_down {
                    return None;
                }
                Some((self.owner, input))
            }
            RawPointer::Up { .. } => {
                if self.cancelled || !self.button_down {
                    return None;
                }
                self.button_down = false;
                Some((self.owner, input))
            }
        }
    }

    fn owner_accepts_gestures(&self) -> bool {
        match self.owner {
            PointerOwner::Desktop => false,
            PointerOwner::Annotation => {
                self.status.annotation_enabled && self.status.annotation_ready
            }
            PointerOwner::Zoom => self.status.zoom_ready && self.status.zoom_active,
        }
    }

    pub fn handle_key(&mut self, shortcut: &str) -> Option<AppCommand> {
        self.router.handle_key(shortcut)
    }

    pub fn handle_wheel(&self, modifier: &str, delta: f32, x: i16, y: i16) -> Option<InputAction> {
        self.router
            .handle_wheel_with_zoom(modifier, delta, x, y, self.status.zoom_active)
    }

    pub fn active_mode(&self) -> ActiveMode {
        self.router.active_mode()
    }
}

pub struct InputRouter {
    bindings: InputBindings,
    active_mode: ActiveMode,
}

impl InputRouter {
    pub fn new(bindings: InputBindings, active_mode: ActiveMode) -> Self {
        Self {
            bindings,
            active_mode,
        }
    }

    pub fn active_mode(&self) -> ActiveMode {
        self.active_mode
    }

    pub fn handle_key(&mut self, shortcut: &str) -> Option<AppCommand> {
        if shortcut == self.bindings.spotlight_mode {
            self.active_mode = ActiveMode::Spotlight;
            return Some(AppCommand::Spotlight(SpotlightCommand::Toggle));
        }

        if shortcut == self.bindings.annotation_mode {
            self.active_mode = ActiveMode::Annotation;
            return Some(AppCommand::Annotation(AnnotationCommand::Toggle));
        }

        if shortcut == self.bindings.zoom_mode {
            return Some(AppCommand::Zoom(ZoomCommand::Toggle));
        }

        None
    }

    pub fn handle_wheel(
        &self,
        modifier: &str,
        delta: f32,
        cursor_x: i16,
        cursor_y: i16,
    ) -> Option<InputAction> {
        self.handle_wheel_with_zoom(modifier, delta, cursor_x, cursor_y, false)
    }

    pub fn handle_wheel_with_zoom(
        &self,
        modifier: &str,
        delta: f32,
        cursor_x: i16,
        cursor_y: i16,
        zoom_active: bool,
    ) -> Option<InputAction> {
        let active_modifier = normalize_modifier(modifier);
        // wheel_modifier governs both spotlight and zoom wheel resize/zoom
        let configured_modifier = normalize_modifier(&self.bindings.wheel_modifier);

        if zoom_active && active_modifier == configured_modifier {
            Some(InputAction::ZoomWheel {
                delta,
                cursor_x,
                cursor_y,
            })
        } else if self.active_mode == ActiveMode::Spotlight
            && active_modifier == configured_modifier
        {
            Some(InputAction::SpotlightWheel { delta })
        } else {
            None
        }
    }
}

fn normalize_modifier(raw: &str) -> &str {
    match raw {
        "Control" => "Ctrl",
        other => other,
    }
}
