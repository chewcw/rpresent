use std::path::PathBuf;

#[test]
fn candidates_are_deterministic_and_ordered() {
    let wrapper = PathBuf::from("/tmp/bin/rpresent-qml-host");
    let got = rpresent_qml_host::paths::native_binary_candidates(&wrapper);

    assert_eq!(
        got,
        vec![
            PathBuf::from("/tmp/bin/rpresent-qml-host-native"),
            PathBuf::from("/tmp/bin/.rpresent/rpresent-qml-host-native"),
        ]
    );
}

#[test]
fn candidates_depend_on_wrapper_directory() {
    let wrapper = PathBuf::from("/opt/custom/rpresent-qml-host");
    let got = rpresent_qml_host::paths::native_binary_candidates(&wrapper);

    assert_eq!(
        got,
        vec![
            PathBuf::from("/opt/custom/rpresent-qml-host-native"),
            PathBuf::from("/opt/custom/.rpresent/rpresent-qml-host-native"),
        ]
    );
}
