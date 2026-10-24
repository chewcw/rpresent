use std::ffi::c_char;
use std::os::fd::RawFd;

use x11_dl::xlib::{Display, XEvent, Xlib};
use xkeysym::Keysym;

use rpresent_annotation::{KeyInput, Modifiers};

/// Thread-confined Xlib display used only for keyboard translation. Xlib key
/// lookup follows the server's current layout and modifier mapping without an
/// input method, so typed characters stay layout-aware.
pub(crate) struct NativeKeys {
    xlib: Xlib,
    display: *mut Display,
}

impl NativeKeys {
    pub(crate) fn new() -> Result<Self, String> {
        let xlib = Xlib::open().map_err(|error| error.to_string())?;
        // SAFETY: a null display name asks Xlib to use $DISPLAY.
        let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null::<c_char>()) };
        if display.is_null() {
            return Err("native key translation requires an X11 display".to_owned());
        }
        Ok(Self { xlib, display })
    }

    pub(crate) fn fd(&self) -> RawFd {
        // SAFETY: the display stays open for the lifetime of `self`.
        unsafe { (self.xlib.XConnectionNumber)(self.display) }
    }

    /// `state` is the modifier mask from the original key event. XKB resolves
    /// Shift, Lock, and the selected group from it.
    pub(crate) fn translate(&mut self, keycode: u8, state: u16) -> Result<KeyInput, String> {
        let mut modifiers = 0u32;
        let mut keysym = 0u64;
        // SAFETY: the display is open and the out-pointers are valid.
        let found = unsafe {
            (self.xlib.XkbLookupKeySym)(
                self.display,
                keycode,
                u32::from(state),
                &mut modifiers,
                &mut keysym,
            )
        };
        let keysym = u32::try_from(keysym).map_err(|_| "keysym out of range".to_owned())?;
        if found == 0 || keysym == 0 {
            return Err(format!("keycode {keycode} has no keysym"));
        }
        let keysym = Keysym::from(keysym);
        let text = keysym.key_char().filter(|character| !character.is_control());
        Ok(KeyInput {
            keysym: keysym.raw(),
            text,
            modifiers: Modifiers {
                shift: state & x11_dl::xlib::ShiftMask as u16 != 0,
                ctrl: state & x11_dl::xlib::ControlMask as u16 != 0,
                alt: state & x11_dl::xlib::Mod1Mask as u16 != 0,
                super_key: state & x11_dl::xlib::Mod4Mask as u16 != 0,
            },
        })
    }

    /// Drain pending keyboard-map notifications so Xlib refreshes its cached
    /// mapping before the next translation.
    pub(crate) fn drain_map_notifications(&mut self) {
        // SAFETY: the display is open; events are read only while pending.
        unsafe {
            while (self.xlib.XPending)(self.display) > 0 {
                let mut event: XEvent = std::mem::zeroed();
                (self.xlib.XNextEvent)(self.display, &mut event);
            }
        }
    }
}

impl Drop for NativeKeys {
    fn drop(&mut self) {
        // SAFETY: the display was opened by `new` and is closed exactly once.
        unsafe {
            (self.xlib.XCloseDisplay)(self.display);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires an X11 session"]
    fn shifted_letters_follow_the_current_layout() {
        let mut keys = NativeKeys::new().expect("native keys require an X11 session");
        let xlib = Xlib::open().unwrap();
        // SAFETY: `keys` owns an open display for this lookup.
        let keycode = unsafe { (xlib.XKeysymToKeycode)(keys.display, u64::from(b'a' as u8)) };
        assert_ne!(keycode, 0);
        let input = keys
            .translate(keycode as u8, x11_dl::xlib::ShiftMask as u16)
            .expect("shifted letter");
        assert_eq!(input.text, Some('A'));
        assert!(input.modifiers.shift);
    }
}
