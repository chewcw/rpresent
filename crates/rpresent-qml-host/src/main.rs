mod paths;

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn main() {
    let me = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("rpresent-qml-host: failed to resolve current executable path: {error}");
            std::process::exit(1);
        }
    };

    let mut candidates = Vec::new();
    candidates.extend(paths::native_binary_candidates(&me));
    candidates.push(PathBuf::from("rpresent-qml-host-native"));
    if let Some(path) = option_env!("RPRESENT_QML_HOST_NATIVE") {
        candidates.push(PathBuf::from(path));
    }

    let mut errors = Vec::new();
    for candidate in candidates {
        match Command::new(&candidate)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
        {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => {
                errors.push(format!("tried {}: {error}", candidate.display()));
            }
        }
    }

    eprintln!(
        "rpresent-qml-host: failed to launch native host; {}",
        errors.join("; ")
    );
    std::process::exit(1);
}
