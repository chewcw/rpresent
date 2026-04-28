use std::os::unix::net::UnixListener;

use tempfile::tempdir;

use rpresent::command::{AppCommand, ZoomCommand};
use rpresent::ipc::{encode_command, read_command, send_command};

#[test]
fn round_trips_one_command_over_unix_socket() {
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
    assert_eq!(
        encode_command(AppCommand::Zoom(ZoomCommand::On)),
        "zoom=on\n"
    );
}
