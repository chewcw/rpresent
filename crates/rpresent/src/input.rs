use std::time::Duration;

use rpresent_config::shortcuts::{
    Binding, KeyInput, Mods, ShortcutAction, ShortcutDispatcher, ValidatedShortcuts,
};

use crate::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};
use crate::native_input::NativeEvent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Spotlight,
    Annotation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputBindings {
    pub global: Vec<Binding>,
    pub wheel_modifier: Mods,
}

impl From<&ValidatedShortcuts> for InputBindings {
    fn from(bindings: &ValidatedShortcuts) -> Self {
        Self {
            global: bindings.global.clone(),
            wheel_modifier: bindings.wheel,
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
        cursor_x: i32,
        cursor_y: i32,
    },
}

#[derive(Debug)]
pub enum AppEvent {
    Command(crate::command::AppCommand),
    Input(InputAction),
    Native(NativeEvent),
    AnnotationRuntime(rpresent_annotation::AnnotationEvent),
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
    pub spotlight_active: bool,
    pub annotation_enabled: bool,
    pub annotation_ready: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PointerOwner {
    #[default]
    Desktop,
    Spotlight,
    Zoom,
    Annotation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPointer {
    Down {
        root: (i32, i32),
        time: Duration,
        modifiers: Mods,
    },
    Move {
        root: (i32, i32),
        time: Duration,
        modifiers: Mods,
    },
    Up {
        root: (i32, i32),
        time: Duration,
        modifiers: Mods,
    },
}

pub fn preferred_pointer_owner(status: InputStatus) -> PointerOwner {
    if status.annotation_enabled && status.annotation_ready {
        PointerOwner::Annotation
    } else if status.spotlight_active {
        PointerOwner::Spotlight
    } else if status.zoom_ready && status.zoom_active {
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

    pub fn confirmed_owner(&self, epoch: u64) -> Option<PointerOwner> {
        (epoch == self.epoch && self.owner != PointerOwner::Desktop).then_some(self.owner)
    }

    pub fn confirmed_target(&self) -> PointerOwner {
        self.owner
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
        if !self.owner_accepts_gestures() {
            self.cancel_gesture();
            return None;
        }

        match input {
            RawPointer::Down { .. } => {
                self.cancelled = false;
                if self.button_down {
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
        self.owner != PointerOwner::Desktop && self.owner == self.preferred_owner()
    }

    pub fn handle_key(&mut self, input: KeyInput) -> Option<AppCommand> {
        self.router.handle_key(input)
    }

    pub fn handle_wheel(&self, modifiers: Mods, delta: f32, x: i32, y: i32) -> Option<InputAction> {
        self.router
            .handle_wheel_for_owner(modifiers, delta, x, y, self.preferred_owner())
    }

    pub fn matches_wheel_modifier(&self, modifiers: Mods) -> bool {
        self.router.matches_wheel_modifier(modifiers)
    }

    pub fn active_mode(&self) -> ActiveMode {
        self.router.active_mode()
    }
}

pub struct InputRouter {
    bindings: InputBindings,
    dispatcher: ShortcutDispatcher,
    active_mode: ActiveMode,
}

impl InputRouter {
    pub fn new(bindings: InputBindings, active_mode: ActiveMode) -> Self {
        let dispatcher = ShortcutDispatcher::new(&bindings.global);
        Self {
            bindings,
            dispatcher,
            active_mode,
        }
    }

    pub fn active_mode(&self) -> ActiveMode {
        self.active_mode
    }

    pub fn handle_key(&mut self, input: KeyInput) -> Option<AppCommand> {
        match self.dispatcher.dispatch(input)? {
            ShortcutAction::SpotlightToggle => {
                self.active_mode = ActiveMode::Spotlight;
                Some(AppCommand::Spotlight(SpotlightCommand::Toggle))
            }
            ShortcutAction::AnnotationToggle => {
                self.active_mode = ActiveMode::Annotation;
                Some(AppCommand::Annotation(AnnotationCommand::Toggle))
            }
            ShortcutAction::ZoomToggle => Some(AppCommand::Zoom(ZoomCommand::Toggle)),
            _ => None,
        }
    }

    pub fn matches_wheel_modifier(&self, modifiers: Mods) -> bool {
        modifiers == self.bindings.wheel_modifier
    }

    pub fn handle_wheel_for_owner(
        &self,
        modifiers: Mods,
        delta: f32,
        cursor_x: i32,
        cursor_y: i32,
        owner: PointerOwner,
    ) -> Option<InputAction> {
        if !self.matches_wheel_modifier(modifiers) {
            return None;
        }
        match owner {
            PointerOwner::Spotlight => Some(InputAction::SpotlightWheel { delta }),
            PointerOwner::Zoom => Some(InputAction::ZoomWheel {
                delta,
                cursor_x,
                cursor_y,
            }),
            PointerOwner::Annotation | PointerOwner::Desktop => None,
        }
    }
}
