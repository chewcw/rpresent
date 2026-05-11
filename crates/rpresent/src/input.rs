use rpresent_config::ShortcutConfig;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Spotlight,
    Annotation,
    Zoom,
}

#[derive(Debug)]
struct ActiveModeStateInner {
    mode: ActiveMode,
    generation: u64,
    applied_generation: u64,
    sync_wait_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct ActiveModeState {
    inner: Arc<(Mutex<ActiveModeStateInner>, Condvar)>,
}

impl ActiveModeState {
    pub fn new(initial: ActiveMode) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(ActiveModeStateInner {
                    mode: initial,
                    generation: 0,
                    applied_generation: 0,
                    sync_wait_enabled: false,
                }),
                Condvar::new(),
            )),
        }
    }

    pub fn get(&self) -> ActiveMode {
        self.snapshot().0
    }

    pub fn snapshot(&self) -> (ActiveMode, u64) {
        let (lock, _) = &*self.inner;
        let state = lock.lock().unwrap();
        (state.mode, state.generation)
    }

    pub fn set(&self, mode: ActiveMode) {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().unwrap();
        if state.mode != mode {
            state.mode = mode;
            state.generation = state.generation.wrapping_add(1);
            condition.notify_all();

            if state.sync_wait_enabled {
                while state.applied_generation < state.generation {
                    let (next_state, wait_result) = condition
                        .wait_timeout(state, Duration::from_millis(250))
                        .unwrap();
                    state = next_state;

                    if wait_result.timed_out() {
                        tracing::warn!(
                            mode = ?mode,
                            generation = state.generation,
                            applied_generation = state.applied_generation,
                            "mode change sync timed out"
                        );
                        break;
                    }
                }
            }
        }
    }

    pub fn enable_sync_wait(&self) {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().unwrap();
        if !state.sync_wait_enabled {
            state.sync_wait_enabled = true;
            condition.notify_all();
        }
    }

    pub fn mark_applied(&self, generation: u64) {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().unwrap();
        if generation > state.applied_generation {
            state.applied_generation = generation;
            condition.notify_all();
        }
    }

    pub fn wait_for_change(&self, generation: u64) -> (ActiveMode, u64) {
        let (lock, condition) = &*self.inner;
        let mut state = lock.lock().unwrap();
        while state.generation == generation {
            state = condition.wait(state).unwrap();
        }
        (state.mode, state.generation)
    }
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
    SpotlightWheel { delta: f32 },
    ZoomWheel { delta: f32, cursor_x: i16, cursor_y: i16 },
    ZoomDragStart { cursor_x: i16, cursor_y: i16 },
    ZoomDragMove { cursor_x: i16, cursor_y: i16 },
    ZoomDragEnd,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    Command(crate::command::AppCommand),
    Input(InputAction),
}

pub struct InputRouter {
    bindings: InputBindings,
    active_mode: ActiveModeState,
}

impl InputRouter {
    pub fn new(bindings: InputBindings, active_mode: ActiveModeState) -> Self {
        Self {
            bindings,
            active_mode,
        }
    }

    pub fn active_mode(&self) -> ActiveMode {
        self.active_mode.get()
    }

    pub fn handle_key(&mut self, shortcut: &str) -> Option<AppCommand> {
        if shortcut == self.bindings.spotlight_mode {
            self.active_mode.set(ActiveMode::Spotlight);
            return Some(AppCommand::Spotlight(SpotlightCommand::Toggle));
        }

        if shortcut == self.bindings.annotation_mode {
            self.active_mode.set(ActiveMode::Annotation);
            return Some(AppCommand::Annotation(AnnotationCommand::Toggle));
        }

        if shortcut == self.bindings.zoom_mode {
            self.active_mode.set(ActiveMode::Zoom);
            return Some(AppCommand::Zoom(ZoomCommand::Toggle));
        }

        None
    }

    pub fn handle_wheel(&self, modifier: &str, delta: f32, cursor_x: i16, cursor_y: i16) -> Option<InputAction> {
        let active_mode = self.active_mode.get();

        match active_mode {
            ActiveMode::Zoom => Some(InputAction::ZoomWheel { delta, cursor_x, cursor_y }),
            ActiveMode::Spotlight => {
                let active_modifier = normalize_modifier(modifier);
                let configured_modifier = normalize_modifier(&self.bindings.wheel_modifier);

                if active_modifier == configured_modifier {
                    Some(InputAction::SpotlightWheel { delta })
                } else {
                    None
                }
            }
            ActiveMode::Annotation => None,
        }
    }

    pub fn handle_drag_start(&self, cursor_x: i16, cursor_y: i16) -> Option<InputAction> {
        if self.active_mode.get() == ActiveMode::Zoom {
            Some(InputAction::ZoomDragStart { cursor_x, cursor_y })
        } else {
            None
        }
    }

    pub fn handle_drag_move(&self, cursor_x: i16, cursor_y: i16) -> Option<InputAction> {
        if self.active_mode.get() == ActiveMode::Zoom {
            Some(InputAction::ZoomDragMove { cursor_x, cursor_y })
        } else {
            None
        }
    }

    pub fn handle_drag_end(&self) -> Option<InputAction> {
        if self.active_mode.get() == ActiveMode::Zoom {
            Some(InputAction::ZoomDragEnd)
        } else {
            None
        }
    }
}

fn normalize_modifier(raw: &str) -> &str {
    match raw {
        "Control" => "Ctrl",
        "Meta" => "Super",
        other => other,
    }
}
