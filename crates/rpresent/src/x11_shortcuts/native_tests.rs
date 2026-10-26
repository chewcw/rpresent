use super::*;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rpresent_config::shortcuts::{
    Key, KeyInput, ShortcutConfig, ShortcutEventSink, parse_wheel_modifiers,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use xkeysym::key;

/// Upper bound for fixture startup, event waits, and shutdown.
const DEADLINE: Duration = Duration::from_secs(5);

#[test]
#[ignore = "requires a private X11 display"]
fn global_function_key_registers_and_unregisters() {
    let (connection, screen) = x11rb::connect(None).unwrap();
    let root = connection.setup().roots[screen].root;
    let connection = Arc::new(connection);
    let settings = ShortcutConfig {
        global: std::collections::BTreeMap::from([("zoom.toggle".into(), "Ctrl+F8".into())]),
        ..ShortcutConfig::default()
    };
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
    let settings = ShortcutConfig {
        global: std::collections::BTreeMap::from([
            ("spotlight.toggle".into(), "Ctrl+F8".into()),
            ("zoom.toggle".into(), "Alt+F9".into()),
        ]),
        ..ShortcutConfig::default()
    };
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

#[test]
#[ignore = "requires a private X11 display"]
fn global_letter_and_function_key_emit_normalized_input() {
    let mut fixture = NativeFixture::start("Super+Z", Mods::SUPER);
    match fixture.press_until("Super+Z") {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("Super+Z".parse().unwrap()));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }
    fixture.stop();

    let mut fixture = NativeFixture::start("Ctrl+F8", Mods::CTRL);
    match fixture.press_until("Ctrl+F8") {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("Ctrl+F8".parse().unwrap()));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }
    fixture.stop();
}

#[test]
#[ignore = "requires a private X11 display"]
fn caps_and_num_do_not_change_shortcut_matching() {
    let mut fixture = NativeFixture::start("a", Mods::CTRL);
    fixture.tap_keysym(key::Caps_Lock);
    let caps = fixture.press_until("a");
    fixture.tap_keysym(key::Caps_Lock);
    fixture.tap_keysym(key::Num_Lock);
    let num = fixture.press_until("a");
    fixture.tap_keysym(key::Num_Lock);
    fixture.stop();

    match caps {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("a".parse().unwrap()));
            assert_eq!(event.editor.text, Some('A'));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }
    match num {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("a".parse().unwrap()));
            assert_eq!(event.editor.text, Some('a'));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }
}

#[test]
#[ignore = "requires a private X11 display"]
fn layout_change_moves_the_registered_letter() {
    let mut fixture = NativeFixture::start("a", Mods::CTRL);
    let old = fixture.injector.keycode(key::a);
    match fixture.press_until("a") {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("a".parse().unwrap()));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }

    fixture.layout("fr");
    let new = fixture.injector.keycode(key::a);
    assert_ne!(old, new, "the layout change must move the letter");
    assert!(
        fixture.wait_for_shortcut("a", new),
        "the new layout position did not activate"
    );

    fixture.injector.tap(old, Vec::new());
    fixture.expect_no_shortcut("a");
    fixture.stop();
}

#[test]
#[ignore = "requires a private X11 display"]
fn active_group_change_refreshes_grabs() {
    let mut fixture = NativeFixture::start_with_keymap(&["-layout", "us,fr"], "a", Mods::CTRL);
    let us_position = fixture.injector.keycode(key::a);
    match fixture.press_until("a") {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("a".parse().unwrap()));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }

    fixture.injector.set_group(1);
    if fixture.injector.group() != 1 {
        // Xvfb accepts the lock request but keeps a single effective group.
        eprintln!("skipping active-group refresh: this X server does not switch keyboard groups");
        fixture.stop();
        return;
    }
    let fr_position = fixture.injector.keycode(key::a);
    assert_ne!(us_position, fr_position);
    assert!(
        fixture.wait_for_shortcut("a", fr_position),
        "the active group did not refresh the grabs"
    );

    fixture.injector.tap(us_position, Vec::new());
    fixture.expect_no_shortcut("a");
    fixture.stop();
}

#[test]
#[ignore = "requires a private X11 display"]
fn combined_wheel_requires_the_complete_modifier_set() {
    let mut fixture = NativeFixture::start("Ctrl+F8", Mods::SUPER | Mods::SHIFT);
    fixture.wheel("Super+Shift+Ctrl");
    fixture.expect_no_wheel();
    fixture.wheel("Super+Shift");
    match fixture.next() {
        ShortcutEvent::Wheel(event) => {
            assert_eq!(event.modifiers, Mods::SUPER | Mods::SHIFT);
            assert_eq!(event.delta, 1.0);
        }
        event => panic!("expected wheel event, received {event:?}"),
    }
    fixture.stop();
}

#[test]
#[ignore = "requires a private X11 display"]
fn shutdown_releases_grabs_and_pending_wheel() {
    let mut fixture = NativeFixture::start("Ctrl+F8", Mods::CTRL);
    match fixture.wheel_until("Ctrl") {
        ShortcutEvent::Wheel(_) => {}
        event => panic!("expected wheel event, received {event:?}"),
    }
    fixture.stop();

    let mut fixture = NativeFixture::start("Ctrl+F8", Mods::CTRL);
    match fixture.wheel_until("Ctrl") {
        ShortcutEvent::Wheel(_) => {}
        event => panic!("the pending wheel grab was not released: {event:?}"),
    }
    match fixture.press_until("Ctrl+F8") {
        ShortcutEvent::Keyboard(event) => {
            assert_eq!(event.shortcut, Some("Ctrl+F8".parse().unwrap()));
        }
        event => panic!("expected keyboard event, received {event:?}"),
    }
    fixture.stop();
}

enum Control {
    Shutdown,
}

/// Owns one shortcut backend on its own native thread and injects input from
/// the test thread through XTEST. The backend and its Xlib display are created
/// inside the native thread and never cross thread boundaries.
struct NativeFixture {
    control: mpsc::Sender<Control>,
    wake: UnixStream,
    events: mpsc::Receiver<ShortcutEvent>,
    join: Option<JoinHandle<()>>,
    observed: Vec<ShortcutEvent>,
    injector: Injector,
}

impl NativeFixture {
    fn start(binding: &str, wheel: Mods) -> Self {
        Self::start_with_keymap(&["-layout", "us"], binding, wheel)
    }

    fn start_with_keymap(keymap: &[&str], binding: &str, wheel: Mods) -> Self {
        set_keymap(keymap);
        let (control, control_rx) = mpsc::channel();
        let (wake, wake_reader) = UnixStream::pair().expect("fixture wake pair");
        wake_reader
            .set_nonblocking(true)
            .expect("fixture wake reads stay non-blocking");
        let (events, event_rx) = mpsc::channel();
        let (ready, ready_rx) = mpsc::channel();
        let expression = binding.to_owned();
        let join = std::thread::spawn(move || {
            run_native(&expression, wheel, &control_rx, &wake_reader, events, ready);
        });
        ready_rx
            .recv_timeout(DEADLINE)
            .expect("the fixture native thread did not start");
        wake.set_nonblocking(true)
            .expect("fixture wake writes stay non-blocking");
        Self {
            control,
            wake,
            events: event_rx,
            join: Some(join),
            observed: Vec::new(),
            injector: Injector::new(),
        }
    }

    fn key(&mut self, expression: &str) {
        self.injector.tap_expression(expression);
    }

    fn tap_keysym(&mut self, keysym: u32) {
        self.injector.tap(self.injector.keycode(keysym), Vec::new());
    }

    fn layout(&mut self, layout: &str) {
        set_keymap(&["-layout", layout]);
    }

    fn wheel(&mut self, modifiers: &str) {
        let modifiers = parse_wheel_modifiers(modifiers).expect("fixture wheel modifiers parse");
        self.injector.wheel(modifiers);
    }

    fn next(&mut self) -> ShortcutEvent {
        match self.events.recv_timeout(DEADLINE) {
            Ok(event) => {
                self.observed.push(event.clone());
                event
            }
            Err(_) => panic!(
                "no shortcut event before the deadline; observed {:?}",
                self.observed
            ),
        }
    }

    fn poll_event(&mut self, wait: Duration) -> Option<ShortcutEvent> {
        match self.events.recv_timeout(wait) {
            Ok(event) => {
                self.observed.push(event.clone());
                Some(event)
            }
            Err(_) => None,
        }
    }

    /// Inject `expression` at the physical `keycode` until the matching
    /// shortcut arrives, so layout refreshes need no sleep-based proof.
    fn wait_for_shortcut(&mut self, expression: &str, keycode: u8) -> bool {
        let expected: KeyInput = expression.parse().expect("fixture shortcut parses");
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            self.injector.tap(keycode, Vec::new());
            if let Some(ShortcutEvent::Keyboard(event)) =
                self.poll_event(Duration::from_millis(200))
                && event.shortcut == Some(expected)
            {
                return true;
            }
        }
        false
    }

    fn expect_no_shortcut(&mut self, expression: &str) {
        let expected: KeyInput = expression.parse().expect("fixture shortcut parses");
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut remaining = deadline;
        while let Some(wait) = remaining.checked_duration_since(Instant::now()) {
            let Some(event) = self.poll_event(wait) else {
                return;
            };
            if let ShortcutEvent::Keyboard(keyboard) = &event {
                assert_ne!(
                    keyboard.shortcut,
                    Some(expected),
                    "the stale position still activates {expression}"
                );
            }
            remaining = deadline;
        }
    }

    fn expect_no_wheel(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut remaining = deadline;
        while let Some(wait) = remaining.checked_duration_since(Instant::now()) {
            let Some(event) = self.poll_event(wait) else {
                return;
            };
            assert!(
                !matches!(event, ShortcutEvent::Wheel(_)),
                "unexpected wheel event {event:?}"
            );
            remaining = deadline;
        }
    }

    /// Inject `expression` until its normalized input arrives. A keymap refresh
    /// can release grabs while a key is in flight, so a single injection is not
    /// a reliable proof; the deadline still bounds the wait.
    fn press_until(&mut self, expression: &str) -> ShortcutEvent {
        let expected: KeyInput = expression.parse().expect("fixture shortcut parses");
        let deadline = Instant::now() + DEADLINE;
        loop {
            while self.events.try_recv().is_ok() {}
            self.key(expression);
            let window = Instant::now() + Duration::from_millis(100);
            while let Some(event) =
                self.poll_event(window.saturating_duration_since(Instant::now()))
            {
                if let ShortcutEvent::Keyboard(keyboard) = &event
                    && keyboard.shortcut == Some(expected)
                {
                    return event;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no event for {expression} before the deadline; observed {:?}",
                self.observed
            );
        }
    }

    /// Inject a wheel gesture until the backend reports one, for the same
    /// refresh-window reason as `press_until`.
    fn wheel_until(&mut self, modifiers: &str) -> ShortcutEvent {
        let deadline = Instant::now() + DEADLINE;
        loop {
            while self.events.try_recv().is_ok() {}
            self.wheel(modifiers);
            let window = Instant::now() + Duration::from_millis(100);
            while let Some(event) =
                self.poll_event(window.saturating_duration_since(Instant::now()))
            {
                if matches!(event, ShortcutEvent::Wheel(_)) {
                    return event;
                }
            }
            assert!(
                Instant::now() < deadline,
                "no wheel event for {modifiers} before the deadline; observed {:?}",
                self.observed
            );
        }
    }

    fn stop(&mut self) {
        let Some(join) = self.join.take() else {
            return;
        };
        let _ = self.control.send(Control::Shutdown);
        let _ = self.wake.write_all(&[1]);
        let _ = join.join();
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_native(
    expression: &str,
    wheel: Mods,
    control: &mpsc::Receiver<Control>,
    wake: &UnixStream,
    events: mpsc::Sender<ShortcutEvent>,
    ready: mpsc::Sender<()>,
) {
    let (connection, screen) = x11rb::connect(None).expect("fixture connects to its X display");
    let root = connection.setup().roots[screen].root;
    let connection = Arc::new(connection);
    let settings = ShortcutConfig {
        global: BTreeMap::from([("zoom.toggle".to_owned(), expression.to_owned())]),
        ..ShortcutConfig::default()
    };
    let bindings = settings.validate().expect("fixture binding is valid");
    let mut backend =
        X11ShortcutBackend::new(Arc::clone(&connection), root).expect("fixture backend starts");
    let sink: ShortcutEventSink = Arc::new(move |event| {
        let _ = events.send(event);
    });
    backend
        .register(&bindings.global, wheel, sink)
        .expect("fixture binding registers");
    connection
        .flush()
        .expect("fixture flushes its X connection");
    // Selecting XKB events can make the server announce the keyboard, which
    // refreshes the grabs. Settle those notifications before accepting input;
    // press_until absorbs any refresh that still overlaps an injected key.
    for _ in 0..2 {
        connection
            .get_input_focus()
            .expect("fixture round trip")
            .reply()
            .expect("fixture round trip reply");
        while let Some(event) = connection
            .poll_for_event()
            .expect("fixture polls its X connection")
        {
            backend
                .handle_event(&event, None)
                .expect("fixture handles startup events");
        }
        connection
            .flush()
            .expect("fixture flushes its X connection");
    }
    ready.send(()).expect("fixture startup is retained");

    let x11_fd = connection.stream().as_raw_fd();
    loop {
        if let Ok(Control::Shutdown) = control.try_recv() {
            backend.unregister().expect("fixture unregisters");
            return;
        }
        while let Some(event) = connection
            .poll_for_event()
            .expect("fixture polls its X connection")
        {
            backend
                .handle_event(&event, None)
                .expect("fixture handles X events");
        }
        connection
            .flush()
            .expect("fixture flushes its X connection");

        let mut descriptors = [
            libc::pollfd {
                fd: x11_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, 100) };
        assert!(result >= 0, "fixture poll failed");
        let mut bytes = [0u8; 64];
        let mut reader = wake;
        while let Ok(count) = reader.read(&mut bytes) {
            if count == 0 {
                break;
            }
        }
    }
}

/// XTEST injection and keymap queries for the fixture's private display.
struct Injector {
    connection: RustConnection,
    root: Window,
    min_keycode: u8,
    max_keycode: u8,
}

impl Injector {
    fn new() -> Self {
        let (connection, screen) =
            x11rb::connect(None).expect("injector connects to its X display");
        let setup = connection.setup();
        let root = setup.roots[screen].root;
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        connection
            .xtest_get_version(2, 2)
            .expect("XTEST version request")
            .reply()
            .expect("the display supports XTEST");
        connection
            .xkb_use_extension(1, 0)
            .expect("XKB version request")
            .reply()
            .expect("the display supports XKB");
        Self {
            connection,
            root,
            min_keycode,
            max_keycode,
        }
    }

    fn keycode(&self, keysym: u32) -> u8 {
        self.keycode_option(keysym)
            .unwrap_or_else(|| panic!("no keycode produces keysym {keysym:#x}"))
    }

    fn keycode_option(&self, keysym: u32) -> Option<u8> {
        let count = self.max_keycode - self.min_keycode + 1;
        let mapping = self
            .connection
            .get_keyboard_mapping(self.min_keycode, count)
            .expect("keymap request")
            .reply()
            .expect("keymap reply");
        let per_keycode = usize::from(mapping.keysyms_per_keycode);
        mapping
            .keysyms
            .chunks(per_keycode)
            .enumerate()
            .find_map(|(index, symbols)| {
                (symbols.first() == Some(&keysym)).then(|| self.min_keycode + index as u8)
            })
    }

    fn tap_expression(&self, expression: &str) {
        let input: KeyInput = expression.parse().expect("fixture key expression parses");
        let (keysym, shifted) = match input.key() {
            Key::Char(character) => (
                u32::from(character.to_ascii_lowercase()),
                character.is_ascii_uppercase(),
            ),
            other => (named_keysym(other), false),
        };
        let mut modifiers = modifier_keysyms(input.mods());
        if shifted {
            modifiers.push(key::Shift_L);
        }
        self.tap(self.keycode(keysym), modifiers);
    }

    fn tap(&self, keycode: u8, modifiers: Vec<u32>) {
        let modifier_codes: Vec<u8> = modifiers
            .iter()
            .map(|keysym| self.keycode(*keysym))
            .collect();
        for modifier in &modifier_codes {
            self.key_event(KEY_PRESS_EVENT, *modifier);
        }
        self.key_event(KEY_PRESS_EVENT, keycode);
        self.key_event(KEY_RELEASE_EVENT, keycode);
        for modifier in modifier_codes.iter().rev() {
            self.key_event(KEY_RELEASE_EVENT, *modifier);
        }
        self.sync();
    }

    fn wheel(&self, modifiers: Mods) {
        let modifier_codes: Vec<u8> = modifier_keysyms(modifiers)
            .iter()
            .map(|keysym| self.keycode(*keysym))
            .collect();
        for modifier in &modifier_codes {
            self.key_event(KEY_PRESS_EVENT, *modifier);
        }
        self.button_event(BUTTON_PRESS_EVENT, 4);
        self.button_event(BUTTON_RELEASE_EVENT, 4);
        for modifier in modifier_codes.iter().rev() {
            self.key_event(KEY_RELEASE_EVENT, *modifier);
        }
        self.sync();
    }

    /// Lock the active keyboard group, which changes the effective group on
    /// the same keymap and so exercises the refresh path without a new map.
    fn set_group(&self, group: u8) {
        self.connection
            .xkb_latch_lock_state(
                u16::from(xkb::ID::USE_CORE_KBD),
                ModMask::from(0_u8),
                ModMask::from(0_u8),
                true,
                xkb::Group::from(group),
                ModMask::from(0_u8),
                false,
                0,
            )
            .expect("group lock request")
            .check()
            .expect("group lock accepted");
        self.sync();
    }

    fn group(&self) -> u8 {
        self.connection
            .xkb_get_state(u16::from(xkb::ID::USE_CORE_KBD))
            .expect("group request")
            .reply()
            .expect("group reply")
            .group
            .into()
    }

    fn key_event(&self, kind: u8, keycode: u8) {
        self.connection
            .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, self.root, 0, 0, 0)
            .expect("XTEST key request")
            .check()
            .expect("XTEST key request accepted");
    }

    fn button_event(&self, kind: u8, button: u8) {
        self.connection
            .xtest_fake_input(kind, button, x11rb::CURRENT_TIME, self.root, 0, 0, 0)
            .expect("XTEST button request")
            .check()
            .expect("XTEST button request accepted");
    }

    fn sync(&self) {
        x11rb::wrapper::ConnectionExt::sync(&self.connection).expect("injector syncs");
    }
}

fn modifier_keysyms(modifiers: Mods) -> Vec<u32> {
    [
        (Mods::CTRL, key::Control_L),
        (Mods::ALT, key::Alt_L),
        (Mods::SUPER, key::Super_L),
        (Mods::SHIFT, key::Shift_L),
    ]
    .into_iter()
    .filter_map(|(flag, keysym)| modifiers.contains(flag).then_some(keysym))
    .collect()
}

fn named_keysym(logical: Key) -> u32 {
    match logical {
        Key::Enter => xkeysym::key::Return,
        Key::Esc => xkeysym::key::Escape,
        Key::Tab => xkeysym::key::Tab,
        Key::Backspace => xkeysym::key::BackSpace,
        Key::Delete => xkeysym::key::Delete,
        Key::Home => xkeysym::key::Home,
        Key::End => xkeysym::key::End,
        Key::PageUp => xkeysym::key::Prior,
        Key::PageDown => xkeysym::key::Next,
        Key::Left => xkeysym::key::Left,
        Key::Right => xkeysym::key::Right,
        Key::Up => xkeysym::key::Up,
        Key::Down => xkeysym::key::Down,
        Key::F1 => xkeysym::key::F1,
        Key::F2 => xkeysym::key::F2,
        Key::F3 => xkeysym::key::F3,
        Key::F4 => xkeysym::key::F4,
        Key::F5 => xkeysym::key::F5,
        Key::F6 => xkeysym::key::F6,
        Key::F7 => xkeysym::key::F7,
        Key::F8 => xkeysym::key::F8,
        Key::F9 => xkeysym::key::F9,
        Key::F10 => xkeysym::key::F10,
        Key::F11 => xkeysym::key::F11,
        Key::F12 => xkeysym::key::F12,
        Key::NumLock => xkeysym::key::Num_Lock,
        Key::Insert => xkeysym::key::Insert,
        other => panic!("the fixture cannot inject {other:?}"),
    }
}

fn set_keymap(arguments: &[&str]) {
    let status = std::process::Command::new("setxkbmap")
        .args(arguments)
        .status()
        .expect("setxkbmap runs on the fixture display");
    assert!(status.success(), "setxkbmap {arguments:?} failed: {status}");
}
