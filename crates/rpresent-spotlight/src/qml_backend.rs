use rpresent_config::SpotlightConfig;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt;
use x11rb::rust_connection::RustConnection;

use crate::overlay::{CursorPosition, OverlayError, SpotlightOverlayBackend};

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

fn runtime_error(e: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(e.to_string())
}

/// Generate candidates for `rpresent-qml-host` executable discovery.
/// Returns colocated paths first (in the same directory as main_exe),
/// followed by a PATH fallback.
pub fn qml_host_candidates(main_exe: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    // Check colocated path first
    if let Some(dir) = main_exe.parent() {
        candidates.push(dir.join("rpresent-qml-host"));
    }

    // Fall back to searching in PATH
    candidates.push(PathBuf::from("rpresent-qml-host"));

    candidates
}

pub struct QmlSpotlightOverlay {
    connection: RustConnection,
    screen_num: usize,
    surface_size: (u16, u16),
    child: Child,
    stdin: ChildStdin,
}

impl QmlSpotlightOverlay {
    pub fn new() -> Result<Self, OverlayError> {
        let (connection, screen_num) = RustConnection::connect(None).map_err(runtime_error)?;
        let screen = &connection.setup().roots[screen_num];
        let surface_size = (screen.width_in_pixels, screen.height_in_pixels);

        let main_exe = std::env::current_exe().map_err(|e| {
            OverlayError::Runtime(format!("failed to determine current executable path: {e}"))
        })?;
        let candidates = qml_host_candidates(&main_exe);

        let mut last_error = String::new();
        let mut child = None;

        for candidate in candidates {
            match Command::new(&candidate)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
            {
                Ok(c) => {
                    child = Some(c);
                    break;
                }
                Err(e) => {
                    last_error = format!("tried {}: {}", candidate.display(), e);
                }
            }
        }

        let mut child = child.ok_or_else(|| {
            OverlayError::Runtime(format!(
                "failed to spawn rpresent-qml-host; {last_error}"
            ))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| OverlayError::Runtime("child stdin unavailable".into()))?;

        Ok(Self {
            connection,
            screen_num,
            surface_size,
            child,
            stdin,
        })
    }

    fn send_message(&mut self, msg: &IpcMessage<'_>) -> Result<(), OverlayError> {
        let mut line = serde_json::to_string(msg).map_err(runtime_error)?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .map_err(runtime_error)?;
        self.stdin.flush().map_err(runtime_error)
    }
}

impl Drop for QmlSpotlightOverlay {
    fn drop(&mut self) {
        // Best-effort graceful quit. Ignore errors if the child is already dead.
        let _ = self.send_message(&IpcMessage::Quit);
        // Best-effort flush so the host can observe any pending input before forced shutdown.
        let _ = self.stdin.flush();
        // Ensure wait() cannot block indefinitely on a wedged child.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl SpotlightOverlayBackend for QmlSpotlightOverlay {
    fn show(&mut self) -> Result<(), OverlayError> {
        self.send_message(&IpcMessage::Show)
    }

    fn hide(&mut self) -> Result<(), OverlayError> {
        self.send_message(&IpcMessage::Hide)
    }

    fn surface_size(&self) -> Result<(u16, u16), OverlayError> {
        Ok(self.surface_size)
    }

    fn cursor_position(&self) -> Result<CursorPosition, OverlayError> {
        let screen = &self.connection.setup().roots[self.screen_num];
        let reply = self
            .connection
            .query_pointer(screen.root)
            .map_err(runtime_error)?
            .reply()
            .map_err(runtime_error)?;
        Ok(CursorPosition {
            x: reply.root_x,
            y: reply.root_y,
        })
    }

    fn redraw(
        &mut self,
        cursor: CursorPosition,
        config: &SpotlightConfig,
    ) -> Result<(), OverlayError> {
        let (surface_w, surface_h) = self.surface_size;
        self.send_message(&IpcMessage::Redraw {
            cursor_x: cursor.x,
            cursor_y: cursor.y,
            surface_w,
            surface_h,
            config,
        })
    }
}
