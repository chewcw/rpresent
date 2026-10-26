use std::collections::HashSet;
use std::ffi::c_char;

use rpresent_config::shortcuts::{BackendError, EditorInput, Key, KeyInput, KeyboardEvent, Mods};
use x11_dl::xlib::{Display, Xlib};
use x11rb::protocol::xproto::ModMask;
use xkeysym::{Keysym, key};

/// `XkbUseCoreKbd` from `XKBlib.h`. `x11-dl` does not expose the constant.
const XKB_USE_CORE_KBD: u32 = 0x0100;

/// Native keycode and modifier-mask pair that owns a grabbed binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NativeGrab {
    pub(crate) keycode: u8,
    pub(crate) modifiers: ModMask,
}

/// Thread-confined Xlib display used only for keyboard translation. Xlib key
/// lookup follows the server's current layout and modifier mapping without an
/// input method, so typed characters stay layout-aware.
pub(crate) struct NativeKeys {
    xlib: Xlib,
    display: *mut Display,
}

/// Native modifier masks for the current keymap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModifierMasks {
    ctrl: u16,
    alt: u16,
    super_key: u16,
    shift: u16,
    caps: u16,
    num: u16,
}

impl ModifierMasks {
    fn logical(&self, state: u16) -> Mods {
        let mut modifiers = Mods::NONE;
        for (mask, flag) in [
            (self.ctrl, Mods::CTRL),
            (self.alt, Mods::ALT),
            (self.super_key, Mods::SUPER),
            (self.shift, Mods::SHIFT),
        ] {
            if mask != 0 && state & mask == mask {
                modifiers |= flag;
            }
        }
        modifiers
    }

    fn locks(&self) -> u16 {
        self.caps | self.num
    }
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

    /// Translate one native key press into the portable shortcut event.
    ///
    /// The editor input keeps the original Shift and lock effects. The
    /// shortcut input drops Caps Lock and Num Lock so lock toggles cannot
    /// change shortcut matching.
    pub(crate) fn keyboard(
        &mut self,
        keycode: u8,
        state: u16,
        epoch: Option<u64>,
    ) -> Result<KeyboardEvent, BackendError> {
        let masks = self.modifier_masks();
        let editor_keysym = self.lookup(keycode, state)?;
        let editor = EditorInput {
            key: keysym_to_key(editor_keysym).unwrap_or(Key::Unidentified),
            text: editor_text(editor_keysym),
            modifiers: masks.logical(state),
        };

        let shortcut_state = state & !masks.locks();
        let shortcut_keysym = self.lookup(keycode, shortcut_state)?;
        let shortcut = (shortcut_keysym != 0)
            .then(|| keysym_to_key(shortcut_keysym))
            .flatten()
            .map(|key| KeyInput::new(key, masks.logical(shortcut_state)));

        Ok(KeyboardEvent {
            epoch,
            shortcut,
            editor,
        })
    }

    /// Native Ctrl/Alt/Super/Shift flags held in `state`.
    pub(crate) fn modifiers(&mut self, state: u16) -> Mods {
        self.modifier_masks().logical(state)
    }

    /// Every native keycode and modifier mask that currently produces
    /// `input`, derived from XKB for the active group.
    pub(crate) fn grab_candidates(
        &mut self,
        input: KeyInput,
    ) -> Result<Vec<NativeGrab>, BackendError> {
        let active_group = self.active_group()?;
        let (min_keycode, max_keycode) = self.keycode_range()?;
        let flags = [Mods::CTRL, Mods::ALT, Mods::SUPER, Mods::SHIFT];
        let mut unique = HashSet::new();
        for keycode in min_keycode..=max_keycode {
            for combination in 0_u8..16 {
                let logical =
                    flags
                        .iter()
                        .enumerate()
                        .fold(Mods::NONE, |modifiers, (index, flag)| {
                            if combination & (1 << index) != 0 {
                                modifiers | *flag
                            } else {
                                modifiers
                            }
                        });
                let native = self.wheel_mask(logical)?;
                let state = u16::from(native) | (u16::from(active_group) << 13);
                if self.keyboard(keycode, state, None)?.shortcut == Some(input) {
                    unique.insert(NativeGrab {
                        keycode,
                        modifiers: native,
                    });
                }
            }
        }
        let mut candidates: Vec<NativeGrab> = unique.into_iter().collect();
        candidates.sort_by_key(|grab| (grab.keycode, u16::from(grab.modifiers)));
        if candidates.is_empty() {
            return Err(BackendError {
                message: format!("the native keymap cannot produce {input}"),
            });
        }
        Ok(candidates)
    }

    /// Distinct Caps Lock and Num Lock combinations to ignore when grabbing.
    pub(crate) fn ignored_masks(&mut self) -> Result<Vec<ModMask>, BackendError> {
        let masks = self.modifier_masks();
        let mut ignored = Vec::with_capacity(4);
        for combination in [0, masks.caps, masks.num, masks.caps | masks.num] {
            let mask = ModMask::from(combination);
            if !ignored.contains(&mask) {
                ignored.push(mask);
            }
        }
        Ok(ignored)
    }

    /// Convert a logical modifier set to native bits for the current keymap.
    pub(crate) fn wheel_mask(&mut self, modifiers: Mods) -> Result<ModMask, BackendError> {
        let masks = self.modifier_masks();
        let supported = Mods::CTRL | Mods::ALT | Mods::SUPER | Mods::SHIFT;
        let unsupported = modifiers - supported;
        if !unsupported.is_empty() {
            return Err(BackendError {
                message: format!("unsupported wheel modifiers: {unsupported:?}"),
            });
        }
        let mut native = 0u16;
        for (flag, mask, name) in [
            (Mods::CTRL, masks.ctrl, "Ctrl"),
            (Mods::ALT, masks.alt, "Alt"),
            (Mods::SUPER, masks.super_key, "Super"),
            (Mods::SHIFT, masks.shift, "Shift"),
        ] {
            if modifiers.contains(flag) {
                if mask == 0 {
                    return Err(BackendError {
                        message: format!("the native keymap cannot represent {name}"),
                    });
                }
                native |= mask;
            }
        }
        Ok(ModMask::from(native))
    }

    /// XKB lookup for one keycode and modifier state. A symbol of zero means
    /// the keycode has no character representation for that state.
    fn lookup(&mut self, keycode: u8, state: u16) -> Result<u32, BackendError> {
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
        if found == 0 {
            return Ok(0);
        }
        u32::try_from(keysym).map_err(|_| BackendError {
            message: format!("keycode {keycode} produced an out-of-range keysym"),
        })
    }

    /// XKB reports `Status`: zero means success.
    fn active_group(&mut self) -> Result<u8, BackendError> {
        let mut state: x11_dl::xlib::XkbStateRec = unsafe { std::mem::zeroed() };
        // SAFETY: the display is open and the out-pointer is valid.
        let status = unsafe { (self.xlib.XkbGetState)(self.display, XKB_USE_CORE_KBD, &mut state) };
        if status != 0 {
            return Err(BackendError {
                message: "cannot read the active keyboard group".into(),
            });
        }
        Ok(state.group)
    }

    fn keycode_range(&mut self) -> Result<(u8, u8), BackendError> {
        let mut min: std::ffi::c_int = 0;
        let mut max: std::ffi::c_int = 0;
        // SAFETY: the display is open and the out-pointers are valid.
        unsafe { (self.xlib.XDisplayKeycodes)(self.display, &mut min, &mut max) };
        if !(0..=255).contains(&min) || !(0..=255).contains(&max) || max < min {
            return Err(BackendError {
                message: format!("unexpected server keycode range {min}..={max}"),
            });
        }
        Ok((min as u8, max as u8))
    }

    fn modifier_masks(&mut self) -> ModifierMasks {
        ModifierMasks {
            ctrl: self.modifier_bits(key::Control_L),
            alt: self.modifier_bits(key::Alt_L),
            super_key: self.modifier_bits(key::Super_L),
            shift: self.modifier_bits(key::Shift_L),
            caps: self.modifier_bits(key::Caps_Lock),
            num: self.modifier_bits(key::Num_Lock),
        }
    }

    fn modifier_bits(&mut self, keysym: u32) -> u16 {
        // SAFETY: the display is open; this only reads the cached modifier map.
        let mask = unsafe { (self.xlib.XkbKeysymToModifiers)(self.display, u64::from(keysym)) };
        u16::try_from(mask & 0xffff).unwrap_or(0)
    }
}

fn editor_text(keysym: u32) -> Option<char> {
    Keysym::from(keysym)
        .key_char()
        .filter(|character| !character.is_control())
}

/// Convert a native keysym to the package's logical key.
///
/// Only real X11 differences are translated here. Named keys that already
/// match the package's names fall through to the package parser.
pub(crate) fn keysym_to_key(raw: u32) -> Option<Key> {
    let named = match raw {
        key::Return | key::KP_Enter => Some(Key::Enter),
        key::Escape => Some(Key::Esc),
        key::Tab | key::ISO_Left_Tab | key::KP_Tab => Some(Key::Tab),
        key::BackSpace => Some(Key::Backspace),
        key::Delete | key::KP_Delete => Some(Key::Delete),
        key::Home | key::KP_Home => Some(Key::Home),
        key::End | key::KP_End => Some(Key::End),
        key::Prior | key::KP_Prior => Some(Key::PageUp),
        key::Next | key::KP_Next => Some(Key::PageDown),
        key::Left | key::KP_Left => Some(Key::Left),
        key::Right | key::KP_Right => Some(Key::Right),
        key::Up | key::KP_Up => Some(Key::Up),
        key::Down | key::KP_Down => Some(Key::Down),
        key::Insert | key::KP_Insert => Some(Key::Insert),
        key::Help => Some(Key::Help),
        key::Menu => Some(Key::Menu),
        key::Scroll_Lock => Some(Key::ScrollLock),
        key::Print | key::Sys_Req => Some(Key::PrintScreen),
        key::XF86_AudioPlay => Some(Key::Play),
        key::XF86_AudioPause => Some(Key::Pause),
        key::XF86_AudioMedia => Some(Key::PlayPause),
        key::XF86_AudioStop => Some(Key::Stop),
        key::XF86_AudioNext => Some(Key::NextTrack),
        key::XF86_AudioPrev => Some(Key::PrevTrack),
        key::XF86_AudioRaiseVolume => Some(Key::VolumeUp),
        key::XF86_AudioLowerVolume => Some(Key::VolumeDown),
        key::XF86_AudioMute => Some(Key::Mute),
        key::XF86_Copy => Some(Key::Copy),
        key::XF86_Cut => Some(Key::Cut),
        key::XF86_Paste => Some(Key::Paste),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let keysym = Keysym::from(raw);
    if let Some(character) = keysym
        .key_char()
        .filter(|character| !character.is_control())
    {
        return Some(Key::Char(character));
    }
    let name = keysym.name()?.strip_prefix("XK_")?;
    name.parse::<Key>().ok()
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
    fn native_symbols_convert_to_package_keys() {
        assert_eq!(keysym_to_key(xkeysym::key::Return), Some(Key::Enter));
        assert_eq!(keysym_to_key(xkeysym::key::Escape), Some(Key::Esc));
        assert_eq!(keysym_to_key(xkeysym::key::A), Some(Key::Char('A')));
        assert_eq!(keysym_to_key(xkeysym::key::F8), Some(Key::F8));
        assert_eq!(keysym_to_key(xkeysym::key::Shift_L), None);
    }

    #[test]
    fn named_and_keypad_symbols_use_package_keys() {
        assert_eq!(keysym_to_key(xkeysym::key::KP_Enter), Some(Key::Enter));
        assert_eq!(keysym_to_key(xkeysym::key::Prior), Some(Key::PageUp));
        assert_eq!(keysym_to_key(xkeysym::key::Next), Some(Key::PageDown));
        assert_eq!(keysym_to_key(xkeysym::key::KP_Left), Some(Key::Left));
        assert_eq!(keysym_to_key(xkeysym::key::ISO_Left_Tab), Some(Key::Tab));
        assert_eq!(
            keysym_to_key(xkeysym::key::XF86_AudioNext),
            Some(Key::NextTrack)
        );
        assert_eq!(keysym_to_key(xkeysym::key::Control_L), None);
    }

    #[test]
    #[ignore = "requires a private X11 display with US layout"]
    fn shortcut_case_ignores_caps_but_editor_text_keeps_it() {
        let mut keys = NativeKeys::new().unwrap();
        // SAFETY: The owned display is open and accessed only on this thread.
        let keycode =
            unsafe { (keys.xlib.XKeysymToKeycode)(keys.display, u64::from(xkeysym::key::a)) };
        assert_ne!(keycode, 0);
        let event = keys
            .keyboard(keycode, x11_dl::xlib::LockMask as u16, None)
            .unwrap();
        assert_eq!(event.shortcut, Some("a".parse().unwrap()));
        assert_eq!(event.editor.text, Some('A'));
    }

    #[test]
    #[ignore = "requires a private X11 display with US layout"]
    fn uppercase_binding_requires_shift_without_caps_effect() {
        let mut keys = NativeKeys::new().unwrap();
        // SAFETY: The owned display is open and accessed only on this thread.
        let keycode =
            unsafe { (keys.xlib.XKeysymToKeycode)(keys.display, u64::from(xkeysym::key::a)) };
        assert_ne!(keycode, 0);
        let state = (x11_dl::xlib::ShiftMask | x11_dl::xlib::LockMask) as u16;
        let event = keys.keyboard(keycode, state, None).unwrap();
        assert_eq!(event.shortcut, Some("A".parse().unwrap()));
        assert_eq!(event.editor.text, Some('a'));
    }
}
