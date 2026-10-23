use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use rpresent_annotation::{AnnotationState, AnnotationTool, run_overlay};
use rpresent_config::ConfigService;

#[test]
fn default_tool_is_pen() {
    assert_eq!(AnnotationTool::default(), AnnotationTool::Pen);
}

#[test]
fn linux_overlay_loop_accepts_state_changes_and_exits_when_channel_closes() {
    let (tx, rx) = mpsc::channel();
    tx.send(AnnotationState::Off).unwrap();
    tx.send(AnnotationState::On).unwrap();
    drop(tx);

    let config_service = Arc::new(ConfigService::new(
        PathBuf::from("test.toml"),
        rpresent_config::Config::default(),
    ));

    run_overlay(config_service, rx).unwrap();
}
