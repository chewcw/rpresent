use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use directories::BaseDirs;
use thiserror::Error;

use crate::command::{AppCommand, AppCommandParseError};

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("ipc io error: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Command(#[from] AppCommandParseError),
    #[error("could not resolve a user config directory")]
    NoConfigDirectory,
    #[error("no daemon listening at IPC socket {socket_path}; start `rpresent` first")]
    DaemonNotRunning { socket_path: PathBuf },
}

pub fn encode_command(command: AppCommand) -> String {
    format!("{command}\n")
}

pub fn read_command(stream: &mut impl Read) -> Result<AppCommand, IpcError> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).parse()?)
}

pub fn send_command(socket_path: &Path, command: AppCommand) -> Result<(), IpcError> {
    tracing::debug!(socket_path = %socket_path.display(), command = %command, "sending IPC command");

    let mut stream = match UnixStream::connect(socket_path) {
        Ok(stream) => stream,
        Err(error) => {
            // A file exists at the path but nothing accepted the connection:
            // either a stale socket left by a crashed daemon or a non-socket file.
            // Surface a clear, actionable error instead of a raw IO error.
            if socket_path.exists() {
                return Err(IpcError::DaemonNotRunning {
                    socket_path: socket_path.to_path_buf(),
                });
            }
            return Err(error.into());
        }
    };

    stream.write_all(encode_command(command).as_bytes())?;
    stream.flush()?;
    Ok(())
}

pub fn socket_path_for(socket_name: &str) -> Result<PathBuf, IpcError> {
    let base_dirs = BaseDirs::new().ok_or(IpcError::NoConfigDirectory)?;
    Ok(base_dirs.config_dir().join("rpresent").join(socket_name))
}

pub fn serve(socket_path: &Path, tx: Sender<AppCommand>) -> Result<(), IpcError> {
    tracing::debug!(socket_path = %socket_path.display(), "starting IPC server");

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove a stale IPC socket left by a previous daemon instance. Only remove
    // regular files or sockets; a directory or other special file at the path is
    // a configuration problem, not something to silently delete.
    if socket_path.exists() {
        match socket_path.metadata() {
            Ok(metadata) => {
                let file_type = metadata.file_type();
                if file_type.is_socket() || file_type.is_file() {
                    tracing::debug!(
                        socket_path = %socket_path.display(),
                        "removing stale IPC socket"
                    );
                    std::fs::remove_file(socket_path)?;
                } else {
                    return Err(std::io::Error::other(format!(
                        "IPC socket path exists and is not a socket or regular file: {}",
                        socket_path.display()
                    ))
                    .into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // File vanished between exists() and metadata(); proceed to bind.
            }
            Err(error) => return Err(error.into()),
        }
    }

    tracing::debug!(socket_path = %socket_path.display(), "binding IPC listener");
    // Bind once; if another instance created the socket in the meantime, remove it
    // and retry once so a transient race does not fail startup.
    let listener = match UnixListener::bind(socket_path) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if socket_path.exists() {
                let _ = std::fs::remove_file(socket_path);
            }
            UnixListener::bind(socket_path)?
        }
        Err(error) => return Err(error.into()),
    };
    tracing::debug!(socket_path = %socket_path.display(), "IPC server started");

    for connection in listener.incoming() {
        let mut stream = connection?;
        let command = match read_command(&mut stream) {
            Ok(command) => command,
            Err(error @ IpcError::Command(_)) => {
                tracing::error!(socket_path = %socket_path.display(), error = %error, "failed to parse IPC command");
                return Err(error);
            }
            Err(error) => return Err(error),
        };

        tracing::debug!(socket_path = %socket_path.display(), command = %command, "received IPC command");
        if let Err(source) = tx.send(command.clone()) {
            tracing::error!(socket_path = %socket_path.display(), command = %command, error = %source, "failed to forward IPC command");
            return Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, source).into());
        }
    }

    Ok(())
}
