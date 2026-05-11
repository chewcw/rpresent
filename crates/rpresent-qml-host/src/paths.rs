use std::path::{Path, PathBuf};

pub fn native_binary_candidates(wrapper_exe: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = wrapper_exe.parent() {
        out.push(dir.join("rpresent-qml-host-native"));
        out.push(dir.join(".rpresent").join("rpresent-qml-host-native"));
    }
    out
}
