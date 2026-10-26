use super::*;
use rpresent_config::shortcuts::ShortcutConfig;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;

#[test]
#[ignore = "requires a private X11 display"]
fn global_function_key_registers_and_unregisters() {
    let (connection, screen) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[screen].root;
    let connection = Arc::new(connection);
    let mut settings = ShortcutConfig::default();
    settings.global = std::collections::BTreeMap::from([("zoom.toggle".into(), "Ctrl+F8".into())]);
    let bindings = settings.validate().unwrap();
    let mut keys = NativeKeys::new().unwrap();
    let grab = keys.grab_candidates("Ctrl+F8".parse().unwrap()).unwrap()[0];
    let mut backend = X11ShortcutBackend::new(connection, root).unwrap();
    backend
        .register(&bindings.global, bindings.wheel, Arc::new(|_| {}))
        .unwrap();
    backend.unregister().unwrap();
    let (other, _) = x11rb::connect(None).unwrap();
    other
        .grab_key(
            false,
            root,
            grab.modifiers,
            grab.keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .unwrap()
        .check()
        .unwrap();
}

#[test]
#[ignore = "requires a private X11 display"]
fn partial_registration_failure_releases_prior_grabs() {
    let (connection, screen) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[screen].root;
    let connection = Arc::new(connection);
    let mut settings = ShortcutConfig::default();
    settings.global = std::collections::BTreeMap::from([
        ("spotlight.toggle".into(), "Ctrl+F8".into()),
        ("zoom.toggle".into(), "Alt+F9".into()),
    ]);
    let bindings = settings.validate().unwrap();
    let mut keys = NativeKeys::new().unwrap();
    let ctrl_f8 = keys.grab_candidates("Ctrl+F8".parse().unwrap()).unwrap()[0];
    let alt_f9 = keys.grab_candidates("Alt+F9".parse().unwrap()).unwrap()[0];
    let (other, _) = x11rb::connect(None).unwrap();
    other
        .grab_key(
            false,
            root,
            alt_f9.modifiers,
            alt_f9.keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .unwrap()
        .check()
        .unwrap();
    let mut backend = X11ShortcutBackend::new(connection, root).unwrap();
    let error = backend
        .register(&bindings.global, bindings.wheel, Arc::new(|_| {}))
        .unwrap_err();
    assert!(error.to_string().contains("zoom.toggle"), "{error}");
    other
        .grab_key(
            false,
            root,
            ctrl_f8.modifiers,
            ctrl_f8.keycode,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
        )
        .unwrap()
        .check()
        .unwrap();
}
