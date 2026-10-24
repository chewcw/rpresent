use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::time::Duration;

use tempfile::tempdir;

use rpresent::command::{AppCommand, ZoomCommand};
use rpresent::ipc::{encode_command, read_command, send_command, IpcError};

#[test]
fn round_trips_one_command_over_unix_socket() {
    rpresent::logging::init();

    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("rpresent.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();

    let writer = std::thread::spawn({
        let socket_path = socket_path.clone();
        move || send_command(&socket_path, AppCommand::Zoom(ZoomCommand::Toggle)).unwrap()
    });

    let (mut stream, _) = listener.accept().unwrap();
    let command = read_command(&mut stream).unwrap();
    assert_eq!(command, AppCommand::Zoom(ZoomCommand::Toggle));

    writer.join().unwrap();
}

#[test]
fn encodes_commands_as_single_lines() {
    rpresent::logging::init();

    assert_eq!(
        encode_command(AppCommand::Zoom(ZoomCommand::On)),
        "zoom=on\n"
    );
}

#[test]
fn send_command_to_stale_socket_returns_daemon_not_running() {
    rpresent::logging::init();

    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("rpresent.sock");
    // A stale empty file where a socket would be (e.g. left by a crashed daemon).
    std::fs::File::create(&socket_path).unwrap();
    assert!(socket_path.exists());

    let error = send_command(&socket_path, AppCommand::Zoom(ZoomCommand::Toggle))
        .unwrap_err();
    assert!(
        matches!(error, IpcError::DaemonNotRunning { .. }),
        "expected DaemonNotRunning, got {error:?}"
    );
}

#[test]
fn send_command_to_missing_socket_surfaces_io_error() {
    rpresent::logging::init();

    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("does_not_exist.sock");
    assert!(!socket_path.exists());

    let error = send_command(&socket_path, AppCommand::Zoom(ZoomCommand::Toggle))
        .unwrap_err();
    assert!(
        !matches!(error, IpcError::DaemonNotRunning { .. }),
        "expected underlying IO error for a missing socket, got {error:?}"
    );
}

#[test]
fn serve_binds_over_stale_socket_file() {
    rpresent::logging::init();

    let dir = tempdir().unwrap();
    let socket_path = dir.path().join("rpresent.sock");
    // Stale empty file from a previous daemon instance.
    std::fs::File::create(&socket_path).unwrap();
    assert!(socket_path.exists());

    let (tx, rx) = mpsc::channel::<AppCommand>();
    let serve_path = socket_path.clone();
    std::thread::spawn(move || {
        let _ = rpresent::ipc::serve(&serve_path, tx);
    });

    // Wait for the server to bind over the stale file and become connectable.
    let mut connected = None;
    for _ in 0..100 {
        if let Ok(stream) = UnixStream::connect(&socket_path) {
            connected = Some(stream);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut stream = connected.expect("serve should bind over the stale socket file");

    stream
        .write_all(encode_command(AppCommand::Zoom(ZoomCommand::Toggle)).as_bytes())
        .unwrap();
    stream.flush().unwrap();

    let received = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("serve should forward the received command");
    assert_eq!(received, AppCommand::Zoom(ZoomCommand::Toggle));
    // The serve thread keeps running (listener loop); intentionally not joined so
    // the test can return while the daemon thread is cleaned up on process exit.
}
