use rpresent::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};
use rpresent::input::{ActiveMode, InputAction, InputBindings, InputRouter};
use rpresent_config::ShortcutConfig;

#[test]
fn shortcut_bindings_follow_the_config() {
    let config = ShortcutConfig {
        spotlight_mode: "Alt+1".to_owned(),
        annotation_mode: "Alt+2".to_owned(),
        zoom_mode: "Alt+3".to_owned(),
        spotlight_wheel_modifier: "Alt".to_owned(),
    };

    let bindings = InputBindings::from(&config);

    assert_eq!(bindings.spotlight_mode, "Alt+1");
    assert_eq!(bindings.annotation_mode, "Alt+2");
    assert_eq!(bindings.zoom_mode, "Alt+3");
    assert_eq!(bindings.spotlight_wheel_modifier, "Alt");
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

    assert_eq!(router.handle_wheel("Shift", 1.0), None);

    assert!(router.handle_key("Ctrl+2").is_some());
    assert_eq!(router.handle_wheel("Ctrl", 1.0), None);

    assert!(router.handle_key("Ctrl+1").is_some());
    assert_eq!(
        router.handle_wheel("Ctrl", 1.0),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}
