use rpresent::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};
use rpresent::input::{ActiveMode, InputAction, InputBindings, InputRouter};
use rpresent_config::ShortcutConfig;

#[test]
fn shortcut_bindings_follow_the_config() {
    let config = ShortcutConfig {
        spotlight_mode: "Alt+1".to_owned(),
        annotation_mode: "Alt+2".to_owned(),
        zoom_mode: "Alt+3".to_owned(),
        wheel_modifier: "Alt".to_owned(),
    };

    let bindings = InputBindings::from(&config);

    assert_eq!(bindings.spotlight_mode, "Alt+1");
    assert_eq!(bindings.annotation_mode, "Alt+2");
    assert_eq!(bindings.zoom_mode, "Alt+3");
    assert_eq!(bindings.wheel_modifier, "Alt");
}

#[test]
fn mode_shortcuts_toggle_and_update_the_active_mode() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_key("Ctrl+1"),
        Some(AppCommand::Spotlight(SpotlightCommand::Toggle))
    );
    assert_eq!(router.active_mode(), ActiveMode::Spotlight);

    assert_eq!(
        router.handle_key("Ctrl+2"),
        Some(AppCommand::Annotation(AnnotationCommand::Toggle))
    );
    assert_eq!(router.active_mode(), ActiveMode::Annotation);

    assert_eq!(
        router.handle_key("Ctrl+3"),
        Some(AppCommand::Zoom(ZoomCommand::Toggle))
    );
    assert_eq!(router.active_mode(), ActiveMode::Zoom);
}

#[test]
fn spotlight_wheel_requires_the_active_mode_and_modifier() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert_eq!(router.handle_wheel("Shift", 1.0, 0, 0), None);

    assert!(router.handle_key("Ctrl+2").is_some());
    assert_eq!(router.handle_wheel("Ctrl", 1.0, 0, 0), None);

    assert!(router.handle_key("Ctrl+1").is_some());
    assert_eq!(
        router.handle_wheel("Ctrl", 1.0, 0, 0),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}

#[test]
fn spotlight_wheel_accepts_ctrl_aliases() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+1").is_some());
    assert_eq!(
        router.handle_wheel("Control", 1.0, 0, 0),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}

#[test]
fn spotlight_wheel_is_blocked_outside_spotlight_mode_even_with_ctrl() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+2").is_some());
    assert_eq!(router.handle_wheel("Control", 1.0, 0, 0), None);
}

#[test]
fn spotlight_wheel_accepts_control_as_configured_modifier() {
    let config = ShortcutConfig {
        spotlight_mode: "Ctrl+1".to_owned(),
        annotation_mode: "Ctrl+2".to_owned(),
        zoom_mode: "Ctrl+3".to_owned(),
        wheel_modifier: "Control".to_owned(), // configured as "Control", not "Ctrl"
    };
    let bindings = InputBindings::from(&config);
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+1").is_some());
    assert_eq!(
        router.handle_wheel("Ctrl", 1.0, 0, 0),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}

#[test]
fn zoom_wheel_emits_zoom_wheel_action_in_zoom_mode() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+3").is_some());
    assert_eq!(
        router.handle_wheel("Ctrl", -1.0, 100, 200),
        Some(InputAction::ZoomWheel {
            delta: -1.0,
            cursor_x: 100,
            cursor_y: 200
        })
    );
}

#[test]
fn zoom_wheel_without_modifier_emits_nothing() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+3").is_some());
    assert_eq!(router.handle_wheel("Shift", 1.0, 0, 0), None);
}

#[test]
fn spotlight_mode_still_emits_spotlight_wheel_not_zoom_wheel() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+1").is_some());
    assert_eq!(
        router.handle_wheel("Ctrl", 1.0, 50, 60),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}

#[test]
fn annotation_mode_emits_nothing_on_wheel() {
    let bindings = InputBindings::from(&ShortcutConfig::default());
    let mut router = InputRouter::new(bindings, ActiveMode::Spotlight);

    assert!(router.handle_key("Ctrl+2").is_some());
    assert_eq!(router.handle_wheel("Ctrl", 1.0, 0, 0), None);
}
