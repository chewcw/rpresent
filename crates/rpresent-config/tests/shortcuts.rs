use rpresent_config::shortcuts::{KeyInput, Mods, ShortcutConfig};

#[test]
fn explicit_scope_replaces_defaults() {
    use rpresent_config::shortcuts::ShortcutDispatcher;

    let settings: ShortcutConfig =
        toml::from_str("[global]\n\"zoom.toggle\" = \"Ctrl+F8\"\n[annotation]\n").unwrap();
    let bindings = settings.validate().unwrap();
    assert_eq!(bindings.global.len(), 1);
    assert_eq!(
        bindings.global[0].input,
        "Ctrl+F8".parse::<KeyInput>().unwrap()
    );
    assert!(bindings.annotation.is_empty());
    assert_eq!(bindings.wheel, Mods::CTRL);

    let mut dispatcher = ShortcutDispatcher::new(&bindings.annotation);
    assert_eq!(dispatcher.dispatch("x".parse().unwrap()), None);
}

#[test]
fn wheel_uses_package_modifier_names() {
    use rpresent_config::shortcuts::parse_wheel_modifiers;
    assert_eq!(
        parse_wheel_modifiers("Super+Shift").unwrap(),
        Mods::SUPER | Mods::SHIFT
    );
    assert_eq!(parse_wheel_modifiers("Control").unwrap(), Mods::CTRL);
    assert!(parse_wheel_modifiers("Meta+Shift").is_err());
    assert!(parse_wheel_modifiers("Super+").is_err());
    assert!(parse_wheel_modifiers("").is_err());
}

#[test]
fn absent_tables_use_scope_defaults() {
    let settings: ShortcutConfig = toml::from_str("").unwrap();
    let bindings = settings.validate().unwrap();
    assert_eq!(bindings.global.len(), 3);
    assert_eq!(bindings.annotation.len(), 17);
    assert_eq!(bindings.wheel, Mods::CTRL);
    assert_eq!(bindings.global[0].action.name(), "annotation.toggle");
    assert_eq!(bindings.global[1].action.name(), "spotlight.toggle");
    assert_eq!(bindings.global[2].action.name(), "zoom.toggle");
    assert_eq!(
        bindings
            .global
            .iter()
            .find(|binding| binding.action.name() == "spotlight.toggle")
            .map(|binding| binding.expression.as_str()),
        Some("Ctrl+Alt+1")
    );
    assert_eq!(
        bindings
            .annotation
            .iter()
            .find(|binding| binding.action.name() == "annotation.tool.rect")
            .map(|binding| binding.expression.as_str()),
        Some("r")
    );
    assert_eq!(
        bindings
            .annotation
            .iter()
            .find(|binding| binding.action.name() == "annotation.tool.arrow")
            .map(|binding| binding.expression.as_str()),
        Some("a")
    );
    assert_eq!(
        bindings
            .annotation
            .iter()
            .find(|binding| binding.action.name() == "annotation.clear")
            .map(|binding| binding.expression.as_str()),
        Some("x")
    );
}

#[test]
fn explicit_empty_tables_disable_each_scope() {
    let settings: ShortcutConfig = toml::from_str("[global]\n[annotation]\n").unwrap();
    let bindings = settings.validate().unwrap();
    assert!(bindings.global.is_empty());
    assert!(bindings.annotation.is_empty());
    assert_eq!(bindings.wheel, Mods::CTRL);
}

#[test]
fn unknown_action_is_rejected_with_its_path() {
    let settings: ShortcutConfig =
        toml::from_str("[global]\n\"bogus.action\" = \"Ctrl+1\"\n").unwrap();
    let error = settings.validate().unwrap_err();
    assert_eq!(error.field, "shortcuts.global.bogus.action");
    assert_eq!(error.value, "Ctrl+1");
    assert_eq!(error.reason, "unknown action");
}

#[test]
fn action_from_the_other_scope_is_rejected() {
    let settings: ShortcutConfig =
        toml::from_str("[global]\n\"annotation.undo\" = \"Ctrl+z\"\n").unwrap();
    let error = settings.validate().unwrap_err();
    assert_eq!(error.field, "shortcuts.global.annotation.undo");
    assert_eq!(error.reason, "action is not valid in this scope");

    let settings: ShortcutConfig =
        toml::from_str("[annotation]\n\"zoom.toggle\" = \"Ctrl+3\"\n").unwrap();
    let error = settings.validate().unwrap_err();
    assert_eq!(error.field, "shortcuts.annotation.zoom.toggle");
    assert_eq!(error.reason, "action is not valid in this scope");
}

#[test]
fn duplicate_parsed_aliases_are_rejected_within_a_scope() {
    let settings: ShortcutConfig = toml::from_str(
        "[global]\n\"spotlight.toggle\" = \"Ctrl+1\"\n\"zoom.toggle\" = \"Control+1\"\n",
    )
    .unwrap();
    let error = settings.validate().unwrap_err();
    assert_eq!(error.field, "shortcuts.global.zoom.toggle");
    assert_eq!(error.value, "Control+1");
    assert!(error.reason.contains("spotlight.toggle"), "{error}");
}

#[test]
fn identical_bindings_in_different_scopes_are_valid() {
    let settings: ShortcutConfig = toml::from_str(
        "[global]\n\"spotlight.toggle\" = \"Ctrl+1\"\n[annotation]\n\"annotation.tool.pen\" = \"Ctrl+1\"\n",
    )
    .unwrap();
    let bindings = settings.validate().unwrap();
    assert_eq!(bindings.global.len(), 1);
    assert_eq!(bindings.annotation.len(), 1);
}

#[test]
fn package_shift_rules_apply_to_scope_values() {
    let settings: ShortcutConfig =
        toml::from_str("[annotation]\n\"annotation.redo\" = \"Ctrl+Z\"\n").unwrap();
    assert_eq!(settings.validate().unwrap().annotation.len(), 1);

    for invalid in ["Ctrl+Shift+z", "Ctrl+x Ctrl+s", "Meta+z", "Ctrl+"] {
        let settings: ShortcutConfig = toml::from_str(&format!(
            "[annotation]\n\"annotation.redo\" = \"{invalid}\"\n"
        ))
        .unwrap();
        let error = settings.validate().unwrap_err();
        assert_eq!(
            error.field, "shortcuts.annotation.annotation.redo",
            "{invalid}"
        );
        assert_eq!(error.value, invalid);
    }
}

#[test]
fn invalid_wheel_tables_are_rejected() {
    for (table, value) in [
        ("[wheel]\nmodifiers = \"Meta\"\n", "Meta"),
        ("[wheel]\nmodifiers = \"\"\n", ""),
        ("[wheel]\nmodifiers = \"Super+\"\n", "Super+"),
        ("[wheel]\nmodifiers = \"+\"\n", "+"),
    ] {
        let settings: ShortcutConfig = toml::from_str(table).unwrap();
        let error = settings.validate().unwrap_err();
        assert_eq!(error.field, "shortcuts.wheel.modifiers");
        assert_eq!(error.value, value, "{table}");
    }
}

#[test]
fn repeated_wheel_modifiers_are_idempotent() {
    use rpresent_config::shortcuts::parse_wheel_modifiers;
    assert_eq!(
        parse_wheel_modifiers("Ctrl+Ctrl+Shift").unwrap(),
        Mods::CTRL | Mods::SHIFT
    );
    assert_eq!(parse_wheel_modifiers(" ctrl ").unwrap(), Mods::CTRL);
}

#[test]
fn uppercase_and_lowercase_dispatch_distinct_actions() {
    use rpresent_config::shortcuts::{ShortcutAction, ShortcutDispatcher};
    let bindings = ShortcutConfig::default().validate().unwrap();
    let mut dispatcher = ShortcutDispatcher::new(&bindings.annotation);
    assert_eq!(
        dispatcher.dispatch("Ctrl+z".parse().unwrap()),
        Some(ShortcutAction::AnnotationUndo)
    );
    assert_eq!(
        dispatcher.dispatch("Ctrl+Z".parse().unwrap()),
        Some(ShortcutAction::AnnotationRedo)
    );
    assert_eq!(
        dispatcher.dispatch("x".parse().unwrap()),
        Some(ShortcutAction::AnnotationClear)
    );
    assert_eq!(dispatcher.dispatch("Alt+z".parse().unwrap()), None);
}

mod fake_backend {
    use rpresent_config::shortcuts::{
        BackendError, Binding, Mods, ShortcutBackend, ShortcutEventSink,
    };

    #[derive(Default)]
    pub struct FakeBackend {
        pub registered: Vec<Binding>,
        pub wheel: Mods,
        pub sink: Option<ShortcutEventSink>,
        pub fail: bool,
    }

    impl ShortcutBackend for FakeBackend {
        fn register(
            &mut self,
            global: &[Binding],
            wheel: Mods,
            events: ShortcutEventSink,
        ) -> Result<(), BackendError> {
            if self.fail {
                return Err(BackendError {
                    message: "registration conflict".into(),
                });
            }
            self.registered = global.to_vec();
            self.wheel = wheel;
            self.sink = Some(events);
            Ok(())
        }

        fn resolve_wheel(&mut self, _token: u64, _replay: bool) -> Result<(), BackendError> {
            Ok(())
        }

        fn unregister(&mut self) -> Result<(), BackendError> {
            self.registered.clear();
            self.sink = None;
            Ok(())
        }
    }
}

#[test]
fn fake_backend_contract_registers_fails_and_cleans_up() {
    use rpresent_config::shortcuts::{
        EditorInput, Key, KeyboardEvent, ShortcutBackend, ShortcutEvent, ShortcutEventSink,
    };
    use std::sync::Arc;

    let bindings = ShortcutConfig::default().validate().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let sink: ShortcutEventSink = Arc::new(move |event| {
        sender.send(event).expect("receiver stays alive");
    });

    let mut fake = fake_backend::FakeBackend::default();
    {
        let backend: &mut dyn ShortcutBackend = &mut fake;
        backend
            .register(&bindings.global, bindings.wheel, Arc::clone(&sink))
            .unwrap();
    }
    assert_eq!(fake.registered, bindings.global);
    assert_eq!(fake.wheel, Mods::CTRL);
    assert!(fake.sink.is_some());

    let event = ShortcutEvent::Keyboard(KeyboardEvent {
        epoch: Some(7),
        shortcut: Some("Ctrl+Z".parse().unwrap()),
        editor: EditorInput {
            key: Key::Char('Z'),
            text: Some('Z'),
            modifiers: Mods::CTRL | Mods::SHIFT,
        },
    });
    (fake.sink.as_ref().unwrap())(event.clone());
    assert_eq!(receiver.recv().unwrap(), event);

    {
        let backend: &mut dyn ShortcutBackend = &mut fake;
        backend.resolve_wheel(7, true).unwrap();
        backend.unregister().unwrap();
    }
    assert!(fake.registered.is_empty());
    assert!(fake.sink.is_none());
    {
        let backend: &mut dyn ShortcutBackend = &mut fake;
        backend.unregister().unwrap();
    }
    assert!(fake.registered.is_empty());

    fake.fail = true;
    let error = {
        let backend: &mut dyn ShortcutBackend = &mut fake;
        backend
            .register(&bindings.global, bindings.wheel, sink)
            .unwrap_err()
    };
    assert_eq!(error.message, "registration conflict");
    assert!(fake.registered.is_empty());
}
