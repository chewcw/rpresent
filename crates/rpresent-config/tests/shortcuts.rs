use rpresent_config::shortcuts::{KeyInput, Mods, ShortcutConfig};

#[test]
fn explicit_scope_replaces_defaults() {
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
}

#[test]
fn wheel_uses_package_modifier_names() {
    use rpresent_config::shortcuts::parse_wheel_modifiers;
    assert_eq!(
        parse_wheel_modifiers("Super+Shift").unwrap(),
        Mods::SUPER | Mods::SHIFT
    );
    assert!(parse_wheel_modifiers("Meta+Shift").is_err());
    assert!(parse_wheel_modifiers("Super+").is_err());
    assert!(parse_wheel_modifiers("").is_err());
}

#[test]
fn absent_tables_use_scope_defaults() {
    let settings: ShortcutConfig = toml::from_str("").unwrap();
    let bindings = settings.validate().unwrap();
    assert_eq!(bindings.global.len(), 3);
    assert_eq!(bindings.annotation.len(), 14);
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
        Some("Ctrl+1")
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
