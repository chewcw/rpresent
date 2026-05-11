use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("failed to execute command");
    assert!(status.success(), "command failed: {cmd:?}");
}

fn emit_rerun_for_tree(root: &Path) {
    println!("cargo:rerun-if-changed={}", root.display());

    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                emit_rerun_for_tree(&path);
            } else {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir.parent().and_then(Path::parent).unwrap().to_path_buf();
    let tool_src = workspace_root.join("tools/rpresent-qml-host");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let build_dir = out_dir.join("qml-host-build");
    let native_out = out_dir.join("rpresent-qml-host-native");

    emit_rerun_for_tree(&tool_src);

    run(Command::new("cmake")
        .arg("-S")
        .arg(&tool_src)
        .arg("-B")
        .arg(&build_dir)
        .arg("-DCMAKE_BUILD_TYPE=Release"));

    run(Command::new("cmake").arg("--build").arg(&build_dir).arg("-j"));

    let built = build_dir.join("rpresent-qml-host");
    std::fs::copy(&built, &native_out).expect("copy native host to OUT_DIR");

    println!("cargo:rustc-env=RPRESENT_QML_HOST_NATIVE={}", native_out.display());
}
