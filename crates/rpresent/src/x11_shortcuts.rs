use std::collections::HashSet;
use std::fmt::Display;
use std::sync::Arc;

use rpresent_config::shortcuts::{
    BackendError, Binding, Mods, ShortcutBackend, ShortcutEvent, ShortcutEventSink, WheelEvent,
};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xkb::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    Allow, ButtonIndex, ConnectionExt as _, EventMask, GrabMode, ModMask, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::native_keys::{NativeGrab, NativeKeys};

const BUTTON_WHEEL_UP: u8 = 4;
const BUTTON_WHEEL_DOWN: u8 = 5;

/// X11 shortcut backend. It owns the passive keyboard and wheel grabs on the
/// root window and normalizes native input into portable events.
pub(crate) struct X11ShortcutBackend {
    connection: Arc<RustConnection>,
    root: Window,
    keys: NativeKeys,
    global: Vec<Binding>,
    wheel: Mods,
    events: Option<ShortcutEventSink>,
    acquired_keys: Vec<NativeGrab>,
    acquired_buttons: Vec<(ButtonIndex, ModMask)>,
    pending_wheel: Option<(u64, u32)>,
}

impl X11ShortcutBackend {
    pub(crate) fn new(connection: Arc<RustConnection>, root: Window) -> Result<Self, BackendError> {
        let keys = NativeKeys::new().map_err(backend_error)?;
        let backend = Self {
            connection,
            root,
            keys,
            global: Vec::new(),
            wheel: Mods::NONE,
            events: None,
            acquired_keys: Vec::new(),
            acquired_buttons: Vec::new(),
            pending_wheel: None,
        };
        backend.enable_notifications()?;
        Ok(backend)
    }

    /// Translate one native event. Returns whether the backend consumed it.
    pub(crate) fn handle_event(
        &mut self,
        event: &Event,
        epoch: Option<u64>,
    ) -> Result<bool, BackendError> {
        match event {
            Event::KeyPress(event) => {
                let keyboard = self
                    .keys
                    .keyboard(event.detail, u16::from(event.state), epoch)?;
                self.emit(ShortcutEvent::Keyboard(keyboard));
                Ok(true)
            }
            Event::ButtonPress(event)
                if event.detail == BUTTON_WHEEL_UP || event.detail == BUTTON_WHEEL_DOWN =>
            {
                let token = u64::from(event.time);
                if epoch.is_none() {
                    self.pending_wheel = Some((token, event.time));
                }
                let wheel = WheelEvent {
                    epoch,
                    token,
                    modifiers: self.keys.modifiers(u16::from(event.state)),
                    delta: if event.detail == BUTTON_WHEEL_UP {
                        1.0
                    } else {
                        -1.0
                    },
                    root: (i32::from(event.root_x), i32::from(event.root_y)),
                };
                self.emit(ShortcutEvent::Wheel(wheel));
                Ok(true)
            }
            Event::XkbNewKeyboardNotify(_) | Event::XkbMapNotify(_) => {
                self.refresh()?;
                Ok(true)
            }
            Event::XkbStateNotify(event) if event.changed.contains(xkb::StatePart::GROUP_STATE) => {
                self.refresh()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Recompile every grab against the current keymap and active group.
    pub(crate) fn refresh(&mut self) -> Result<(), BackendError> {
        let global = self.global.clone();
        let wheel = self.wheel;
        let events = self.events.clone().ok_or_else(|| BackendError {
            message: "cannot refresh an unregistered backend".into(),
        })?;
        self.unregister()?;
        self.keys = NativeKeys::new().map_err(backend_error)?;
        self.register(&global, wheel, events)
    }

    fn enable_notifications(&self) -> Result<(), BackendError> {
        let reply = self
            .connection
            .xkb_use_extension(1, 0)
            .map_err(backend_error)?
            .reply()
            .map_err(backend_error)?;
        if !reply.supported {
            return Err(BackendError {
                message: "the X server does not support XKB".into(),
            });
        }
        self.connection
            .xkb_select_events(
                u16::from(xkb::ID::USE_CORE_KBD),
                xkb::EventType::default(),
                xkb::EventType::NEW_KEYBOARD_NOTIFY
                    | xkb::EventType::MAP_NOTIFY
                    | xkb::EventType::STATE_NOTIFY,
                xkb::MapPart::default(),
                xkb::MapPart::default(),
                &xkb::SelectEventsAux::new(),
            )
            .map_err(backend_error)?
            .check()
            .map_err(backend_error)?;
        self.connection.flush().map_err(backend_error)
    }

    fn emit(&self, event: ShortcutEvent) {
        if let Some(events) = &self.events {
            events(event);
        }
    }

    fn acquire(&mut self) -> Result<(), BackendError> {
        let ignored = self.keys.ignored_masks()?;
        let mut keys = HashSet::new();
        for binding in self.global.clone() {
            let action = binding.action.name();
            let expression = binding.expression;
            for candidate in self.keys.grab_candidates(binding.input)? {
                for lock in &ignored {
                    let grab = NativeGrab {
                        keycode: candidate.keycode,
                        modifiers: candidate.modifiers | *lock,
                    };
                    if !keys.insert(grab) {
                        continue;
                    }
                    self.connection
                        .grab_key(
                            false,
                            self.root,
                            grab.modifiers,
                            grab.keycode,
                            GrabMode::ASYNC,
                            GrabMode::ASYNC,
                        )
                        .map_err(backend_error)?
                        .check()
                        .map_err(|error| BackendError {
                            message: format!("{action} = {expression:?}: {error}"),
                        })?;
                    self.acquired_keys.push(grab);
                }
            }
        }

        let wheel = self.keys.wheel_mask(self.wheel)?;
        for button in [ButtonIndex::M4, ButtonIndex::M5] {
            for lock in &ignored {
                let modifiers = wheel | *lock;
                if self.acquired_buttons.contains(&(button, modifiers)) {
                    continue;
                }
                self.connection
                    .grab_button(
                        false,
                        self.root,
                        EventMask::BUTTON_PRESS,
                        GrabMode::SYNC,
                        GrabMode::ASYNC,
                        0_u32,
                        0_u32,
                        button,
                        modifiers,
                    )
                    .map_err(backend_error)?
                    .check()
                    .map_err(|error| BackendError {
                        message: format!("wheel button {button:?}: {error}"),
                    })?;
                self.acquired_buttons.push((button, modifiers));
            }
        }
        self.connection.flush().map_err(backend_error)
    }

    fn release_grabs(&mut self) -> Result<(), BackendError> {
        let mut failure = None;
        for grab in std::mem::take(&mut self.acquired_keys) {
            let result = self
                .connection
                .ungrab_key(grab.keycode, self.root, grab.modifiers)
                .map_err(backend_error)
                .and_then(|cookie| cookie.check().map_err(backend_error));
            if let Err(error) = result {
                failure.get_or_insert_with(|| backend_error(error));
            }
        }
        for (button, modifiers) in std::mem::take(&mut self.acquired_buttons) {
            let result = self
                .connection
                .ungrab_button(button, self.root, modifiers)
                .map_err(backend_error)
                .and_then(|cookie| cookie.check().map_err(backend_error));
            if let Err(error) = result {
                failure.get_or_insert_with(|| backend_error(error));
            }
        }
        let flushed = self.connection.flush().map_err(backend_error).err();
        match (failure, flushed) {
            (Some(error), _) => Err(error),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(()),
        }
    }

    fn release_pending(&mut self) -> Result<(), BackendError> {
        let Some((_, time)) = self.pending_wheel.take() else {
            return Ok(());
        };
        self.connection
            .allow_events(Allow::ASYNC_POINTER, time)
            .map_err(backend_error)?
            .check()
            .map_err(backend_error)?;
        self.connection.flush().map_err(backend_error)
    }
}

impl ShortcutBackend for X11ShortcutBackend {
    fn register(
        &mut self,
        global: &[Binding],
        wheel: Mods,
        events: ShortcutEventSink,
    ) -> Result<(), BackendError> {
        if self.events.is_some() {
            return Err(BackendError {
                message: "shortcut backend is already registered".into(),
            });
        }
        self.global.clear();
        self.global.extend_from_slice(global);
        self.wheel = wheel;
        self.events = Some(events);
        if let Err(error) = self.acquire() {
            let _ = self.release_grabs();
            self.events = None;
            return Err(error);
        }
        Ok(())
    }

    fn resolve_wheel(&mut self, token: u64, replay: bool) -> Result<(), BackendError> {
        let Some((pending, _)) = self.pending_wheel else {
            return Ok(());
        };
        if pending != token {
            return Ok(());
        }
        let Some((_, time)) = self.pending_wheel.take() else {
            return Ok(());
        };
        self.connection
            .allow_events(
                if replay {
                    Allow::REPLAY_POINTER
                } else {
                    Allow::ASYNC_POINTER
                },
                time,
            )
            .map_err(backend_error)?
            .check()
            .map_err(backend_error)?;
        self.connection.flush().map_err(backend_error)
    }

    fn unregister(&mut self) -> Result<(), BackendError> {
        self.release_pending()?;
        self.release_grabs()?;
        self.events = None;
        self.global.clear();
        self.wheel = Mods::NONE;
        Ok(())
    }
}

impl Drop for X11ShortcutBackend {
    fn drop(&mut self) {
        let _ = self.release_pending();
        let _ = self.release_grabs();
    }
}

fn backend_error(error: impl Display) -> BackendError {
    BackendError {
        message: error.to_string(),
    }
}

#[cfg(test)]
mod native_tests;
