use rpresent_config::SpotlightConfig;
use serde::Serialize;

/// Messages sent from Rust to the `rpresent-qml-host` subprocess over its stdin.
/// Each message is serialized as a single JSON line followed by `\n`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMessage<'a> {
    Show,
    Hide,
    Quit,
    Redraw {
        cursor_x: i16,
        cursor_y: i16,
        surface_w: u16,
        surface_h: u16,
        config: &'a SpotlightConfig,
    },
}
