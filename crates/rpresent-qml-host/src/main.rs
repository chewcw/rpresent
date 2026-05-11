mod paths;

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    let me = std::env::current_exe().expect("resolve current executable path");

    let mut candidates = Vec::new();
    if let Some(path) = option_env!("RPRESENT_QML_HOST_NATIVE") {
        candidates.push(PathBuf::from(path));
    }
    candidates.extend(paths::native_binary_candidates(&me));

    let candidate = candidates
        .into_iter()
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("rpresent-qml-host-native"));

    let status = Command::new(candidate)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .expect("spawn native qml host");

    std::process::exit(status.code().unwrap_or(1));
}
