use std::path::PathBuf;

#[test]
fn prefers_native_binary_next_to_wrapper() {
    let wrapper = PathBuf::from("/tmp/bin/rpresent-qml-host");
    let got = rpresent_qml_host::paths::native_binary_candidates(&wrapper);
    assert!(
        got.iter().any(|p| p.ends_with("rpresent-qml-host-native")),
        "expected candidates to include native host binary name"
    );
}
