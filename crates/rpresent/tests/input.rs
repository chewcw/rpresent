use std::time::Duration;

use rpresent::command::{AnnotationCommand, AppCommand, SpotlightCommand, ZoomCommand};
use rpresent::input::{
    ActiveMode, InputAction, InputBindings, InputCoordinator, InputRouter, InputStatus,
    PointerOwner, RawPointer,
};
use rpresent_config::shortcuts::{KeyInput, Mods, ShortcutConfig};

#[test]
fn shortcut_bindings_follow_the_config() {
    let config = ShortcutConfig {
        global: std::collections::BTreeMap::from([
            ("spotlight.toggle".into(), "Alt+1".into()),
            ("annotation.toggle".into(), "Alt+2".into()),
            ("zoom.toggle".into(), "Alt+3".into()),
        ]),
        annotation: std::collections::BTreeMap::new(),
        wheel: rpresent_config::shortcuts::WheelConfig {
            modifiers: "Alt".into(),
        },
    };

    let bindings = config.validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);

    assert_eq!(input_bindings.global.len(), 3);
    assert_eq!(input_bindings.wheel_modifier, Mods::ALT);
}

#[test]
fn mode_shortcuts_toggle_and_update_the_active_mode() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let mut router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_key("Ctrl+1".parse::<KeyInput>().unwrap()),
        Some(AppCommand::Spotlight(SpotlightCommand::Toggle))
    );
    assert_eq!(router.active_mode(), ActiveMode::Spotlight);

    assert_eq!(
        router.handle_key("Ctrl+2".parse::<KeyInput>().unwrap()),
        Some(AppCommand::Annotation(AnnotationCommand::Toggle))
    );
    assert_eq!(router.active_mode(), ActiveMode::Annotation);

    assert_eq!(
        router.handle_key("Ctrl+3".parse::<KeyInput>().unwrap()),
        Some(AppCommand::Zoom(ZoomCommand::Toggle))
    );
    assert_eq!(router.active_mode(), ActiveMode::Annotation);
}

#[test]
fn spotlight_wheel_requires_the_configured_modifier() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_wheel_for_owner(Mods::CTRL, 1.0, 0, 0, PointerOwner::Spotlight),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
    assert_eq!(
        router.handle_wheel_for_owner(Mods::SHIFT, 1.0, 0, 0, PointerOwner::Spotlight),
        None
    );
}

#[test]
fn spotlight_wheel_is_blocked_outside_spotlight_mode_even_with_ctrl() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let mut router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert!(
        router
            .handle_key("Ctrl+2".parse::<KeyInput>().unwrap())
            .is_some()
    );
    assert_eq!(
        router.handle_wheel_for_owner(Mods::CTRL, 1.0, 0, 0, PointerOwner::Spotlight),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}

#[test]
fn zoom_wheel_emits_zoom_wheel_action_in_zoom_mode() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_wheel_for_owner(Mods::CTRL, -1.0, 100, 200, PointerOwner::Zoom),
        Some(InputAction::ZoomWheel {
            delta: -1.0,
            cursor_x: 100,
            cursor_y: 200,
        })
    );
}

#[test]
fn zoom_wheel_without_modifier_emits_nothing() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_wheel_for_owner(Mods::SHIFT, 1.0, 0, 0, PointerOwner::Zoom),
        None
    );
}

#[test]
fn spotlight_mode_still_emits_spotlight_wheel_not_zoom_wheel() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_wheel_for_owner(Mods::CTRL, 1.0, 50, 60, PointerOwner::Spotlight),
        Some(InputAction::SpotlightWheel { delta: 1.0 })
    );
}

#[test]
fn annotation_mode_emits_nothing_on_wheel() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let input_bindings = InputBindings::from(&bindings);
    let router = InputRouter::new(input_bindings, ActiveMode::Spotlight);

    assert_eq!(
        router.handle_wheel_for_owner(Mods::CTRL, 1.0, 0, 0, PointerOwner::Annotation),
        None
    );
}

#[test]
fn active_zoom_owns_wheel_even_when_the_previous_tool_was_spotlight() {
    let input = coordinator(InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: false,
        annotation_ready: false,
    });

    assert_eq!(
        input.handle_wheel(Mods::CTRL, -1.0, 100, 200),
        Some(InputAction::ZoomWheel {
            delta: -1.0,
            cursor_x: 100,
            cursor_y: 200,
        })
    );
}

#[test]
fn inactive_zoom_leaves_wheel_with_the_previous_tool() {
    let input = coordinator(InputStatus::default());

    assert_eq!(input.preferred_owner(), PointerOwner::Desktop);
    assert_eq!(input.handle_wheel(Mods::CTRL, 1.0, 100, 200), None);
}

fn coordinator(status: InputStatus) -> InputCoordinator {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let mut input = InputCoordinator::new(InputBindings::from(&bindings));
    input.set_status(status);
    input
}

fn zoom_only() -> InputStatus {
    InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: false,
        annotation_ready: false,
    }
}

fn down(x: i32, y: i32) -> RawPointer {
    RawPointer::Down {
        root: (x, y),
        time: Duration::from_millis(1),
    }
}

fn moved(x: i32, y: i32) -> RawPointer {
    RawPointer::Move {
        root: (x, y),
        time: Duration::from_millis(2),
    }
}

fn up() -> RawPointer {
    RawPointer::Up {
        root: (0, 0),
        time: Duration::from_millis(3),
    }
}

#[test]
fn annotation_owns_drawing_but_active_zoom_keeps_the_wheel() {
    let input = coordinator(InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: true,
        annotation_ready: true,
    });

    assert_eq!(input.preferred_owner(), PointerOwner::Annotation);
    assert_eq!(input.handle_wheel(Mods::CTRL, 1.0, 40, 50), None);
}

#[test]
fn an_unavailable_annotation_backend_leaves_drawing_to_zoom() {
    let mut input = coordinator(InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: true,
        annotation_ready: false,
    });

    assert_eq!(input.preferred_owner(), PointerOwner::Zoom);
    input.confirm_owner(PointerOwner::Zoom, 1);
    assert_eq!(
        input.handle_pointer(1, down(10, 10)),
        Some((PointerOwner::Zoom, down(10, 10)))
    );
}

#[test]
fn local_keyboard_events_require_the_confirmed_owner_epoch() {
    let bindings = ShortcutConfig::default().validate().unwrap();
    let mut input = InputCoordinator::new(InputBindings::from(&bindings));
    input.set_status(InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: true,
        annotation_ready: true,
    });
    input.confirm_owner(PointerOwner::Annotation, 9);
    assert!(input.owns_epoch(PointerOwner::Annotation, 9));
    assert!(!input.owns_epoch(PointerOwner::Annotation, 8));
    input.confirm_owner(PointerOwner::Zoom, 10);
    assert!(!input.owns_epoch(PointerOwner::Annotation, 9));
}

#[test]
fn prepared_zoom_does_not_accept_a_gesture_before_it_commits() {
    let mut input = coordinator(InputStatus {
        zoom_ready: true,
        zoom_active: false,
        spotlight_active: false,
        annotation_enabled: false,
        annotation_ready: false,
    });
    input.confirm_owner(PointerOwner::Zoom, 1);

    assert_eq!(input.preferred_owner(), PointerOwner::Desktop);
    assert_eq!(input.handle_pointer(1, down(10, 10)), None);
}

#[test]
fn annotation_mode_keeps_the_wheel_inactive_while_zoom_is_inactive() {
    let mut input = coordinator(InputStatus::default());

    assert!(
        input
            .handle_key("Ctrl+2".parse::<KeyInput>().unwrap())
            .is_some()
    );
    assert_eq!(input.handle_wheel(Mods::CTRL, 1.0, 0, 0), None);
}

#[test]
fn gesture_transfer_drops_events_from_obsolete_epochs() {
    let mut input = coordinator(zoom_only());
    input.confirm_owner(PointerOwner::Zoom, 7);

    assert_eq!(
        input.handle_pointer(7, down(10, 20)),
        Some((PointerOwner::Zoom, down(10, 20)))
    );
    assert_eq!(input.cancel_gesture(), Some(PointerOwner::Zoom));

    assert_eq!(input.handle_pointer(7, moved(11, 21)), None);
    assert_eq!(input.handle_pointer(7, up()), None);

    input.set_status(InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: true,
        annotation_ready: true,
    });
    input.confirm_owner(PointerOwner::Annotation, 8);

    assert_eq!(input.handle_pointer(7, down(30, 40)), None);
    assert_eq!(
        input.handle_pointer(8, down(30, 40)),
        Some((PointerOwner::Annotation, down(30, 40)))
    );
    assert_eq!(
        input.handle_pointer(8, up()),
        Some((PointerOwner::Annotation, up()))
    );
}

#[test]
fn a_release_never_starts_a_gesture() {
    let mut input = coordinator(zoom_only());
    input.confirm_owner(PointerOwner::Zoom, 1);

    assert_eq!(input.handle_pointer(1, up()), None);
    assert_eq!(input.handle_pointer(1, moved(5, 5)), None);
}

#[test]
fn a_failed_grab_does_not_confirm_the_new_owner() {
    let mut input = coordinator(zoom_only());
    input.confirm_owner(PointerOwner::Zoom, 1);

    input.set_status(InputStatus {
        zoom_ready: true,
        zoom_active: true,
        spotlight_active: false,
        annotation_enabled: true,
        annotation_ready: true,
    });

    // The grab for annotation failed, so no confirmation arrived: the old
    // epoch and owner still govern delivery.
    assert_eq!(input.preferred_owner(), PointerOwner::Annotation);
    assert_eq!(input.handle_pointer(2, down(10, 10)), None);
    assert_eq!(input.cancel_gesture(), None);
}

#[test]
fn every_status_combination_selects_a_defined_owner_and_wheel_route() {
    for bits in 0..32u8 {
        let status = InputStatus {
            zoom_ready: bits & 1 != 0,
            zoom_active: bits & 2 != 0,
            annotation_enabled: bits & 4 != 0,
            annotation_ready: bits & 8 != 0,
            spotlight_active: bits & 16 != 0,
        };
        let input = coordinator(status);

        let expected_owner = match (
            status.annotation_enabled && status.annotation_ready,
            status.spotlight_active,
            status.zoom_ready && status.zoom_active,
        ) {
            (true, _, _) => PointerOwner::Annotation,
            (false, true, _) => PointerOwner::Spotlight,
            (false, false, true) => PointerOwner::Zoom,
            _ => PointerOwner::Desktop,
        };
        assert_eq!(input.preferred_owner(), expected_owner, "bits {bits}");

        let expected_wheel = match expected_owner {
            PointerOwner::Spotlight => Some(InputAction::SpotlightWheel { delta: 1.0 }),
            PointerOwner::Zoom => Some(InputAction::ZoomWheel {
                delta: 1.0,
                cursor_x: -100,
                cursor_y: 200,
            }),
            _ => None,
        };
        assert_eq!(
            input.handle_wheel(Mods::CTRL, 1.0, -100, 200),
            expected_wheel,
            "bits {bits}"
        );
        assert_eq!(input.handle_wheel(Mods::SHIFT, 1.0, 0, 0), None);
    }
}

#[test]
fn spotlight_preempts_an_existing_zoom_drag_before_owner_confirmation() {
    let mut input = coordinator(zoom_only());
    input.confirm_owner(PointerOwner::Zoom, 7);
    assert!(input.handle_pointer(7, down(1, 2)).is_some());
    input.set_status(InputStatus {
        spotlight_active: true,
        ..zoom_only()
    });
    assert_eq!(input.handle_pointer(7, moved(30, 40)), None);
    assert_eq!(input.handle_pointer(7, up()), None);
    assert_eq!(input.confirmed_owner(7), Some(PointerOwner::Zoom));
    assert!(!input.owns_epoch(PointerOwner::Zoom, 7));
}

#[test]
fn returning_to_zoom_requires_a_new_press() {
    let mut input = coordinator(InputStatus {
        spotlight_active: true,
        ..zoom_only()
    });
    input.confirm_owner(PointerOwner::Spotlight, 8);
    assert!(input.handle_pointer(8, down(1, 2)).is_some());
    input.set_status(zoom_only());
    input.revoke_owner();
    input.confirm_owner(PointerOwner::Zoom, 9);
    assert_eq!(input.handle_pointer(8, moved(30, 40)), None);
    assert_eq!(input.handle_pointer(9, moved(30, 40)), None);
    assert_eq!(input.handle_pointer(9, up()), None);
    assert_eq!(
        input.handle_pointer(9, down(30, 40)),
        Some((PointerOwner::Zoom, down(30, 40)))
    );
}
