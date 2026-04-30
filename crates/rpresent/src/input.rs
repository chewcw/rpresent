use rpresent_config::ShortcutConfig;

use crate::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveMode {
    Spotlight,
    Annotation,
    Zoom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputBindings {
    pub spotlight_mode: String,
    pub annotation_mode: String,
    pub zoom_mode: String,
    pub spotlight_wheel_modifier: String,
}

impl From<&ShortcutConfig> for InputBindings {
    fn from(config: &ShortcutConfig) -> Self {
        Self {
            spotlight_mode: config.spotlight_mode.clone(),
            annotation_mode: config.annotation_mode.clone(),
            zoom_mode: config.zoom_mode.clone(),
            spotlight_wheel_modifier: config.spotlight_wheel_modifier.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputAction {
    SpotlightWheel { delta: f32 },
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppEvent {
    Command(crate::command::AppCommand),
    Input(InputAction),
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
            self.active_mode = ActiveMode::Zoom;
            return Some(AppCommand::Zoom(ZoomCommand::Toggle));
        }

        None
    }

    pub fn handle_wheel(&self, modifier: &str, delta: f32) -> Option<InputAction> {
        let active_modifier = normalize_modifier(modifier);
        let configured_modifier = normalize_modifier(&self.bindings.spotlight_wheel_modifier);

        if self.active_mode == ActiveMode::Spotlight && active_modifier == configured_modifier {
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
