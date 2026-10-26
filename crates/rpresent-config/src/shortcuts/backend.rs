use std::sync::Arc;

use super::{Binding, Key, KeyInput, Mods};

/// The original editor input for a key press, with native lock and Shift
/// effects preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorInput {
    pub key: Key,
    pub text: Option<char>,
    pub modifiers: Mods,
}

/// A normalized key press. `shortcut` is `None` when the native symbol cannot
/// form a configurable binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardEvent {
    pub epoch: Option<u64>,
    pub shortcut: Option<KeyInput>,
    pub editor: EditorInput,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WheelEvent {
    pub epoch: Option<u64>,
    pub token: u64,
    pub modifiers: Mods,
    pub delta: f32,
    pub root: (i32, i32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShortcutEvent {
    Keyboard(KeyboardEvent),
    Wheel(WheelEvent),
}

pub type ShortcutEventSink = Arc<dyn Fn(ShortcutEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("shortcut backend failed: {message}")]
pub struct BackendError {
    pub message: String,
}

/// Platform boundary for native shortcut input.
///
/// The trait is not `Send`: an implementation may stay confined to its native
/// thread. The event sink is thread-safe.
pub trait ShortcutBackend {
    fn register(
        &mut self,
        global: &[Binding],
        wheel: Mods,
        events: ShortcutEventSink,
    ) -> Result<(), BackendError>;

    /// Release or replay a pending wheel gesture. An opaque token that no
    /// longer matches the pending gesture does nothing.
    fn resolve_wheel(&mut self, token: u64, replay: bool) -> Result<(), BackendError>;

    fn unregister(&mut self) -> Result<(), BackendError>;
}
