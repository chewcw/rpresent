use std::io::{BufRead, BufReader, Read, Write};
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
    let mut stream = UnixStream::connect(socket_path)?;
    stream.write_all(encode_command(command).as_bytes())?;
    stream.flush()?;
    Ok(())
}

pub fn socket_path_for(socket_name: &str) -> Result<PathBuf, IpcError> {
    let base_dirs = BaseDirs::new().ok_or(IpcError::NoConfigDirectory)?;
    Ok(base_dirs.config_dir().join("rpresent").join(socket_name))
}

pub fn serve(socket_path: &Path, tx: Sender<AppCommand>) -> Result<(), IpcError> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    let listener = UnixListener::bind(socket_path)?;
    for connection in listener.incoming() {
        let mut stream = connection?;
        let command = read_command(&mut stream)?;
        tx.send(command)
            .map_err(|source| std::io::Error::new(std::io::ErrorKind::BrokenPipe, source))?;
    }

    Ok(())
}
