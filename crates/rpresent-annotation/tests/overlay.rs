use std::sync::mpsc;

use rpresent_annotation::{AnnotationAppearance, AnnotationState, AnnotationTool, run_overlay};
use rpresent_config::AnnotationConfig;

#[test]
fn overlay_defaults_match_the_mvp() {
    let appearance = AnnotationAppearance::default();
    assert_eq!(appearance.stroke_width_px, 6);
    assert_eq!(appearance.history_depth, 64);
    assert!((appearance.canvas_alpha - 0.2).abs() < f32::EPSILON);
}

#[test]
fn appearance_can_be_built_from_the_shared_annotation_config() {
    let config = AnnotationConfig {
        enabled_on_start: true,
        default_tool: AnnotationTool::Eraser,
        stroke_width_px: 12,
        history_depth: 128,
    };

    let appearance = AnnotationAppearance::from(config);
    assert_eq!(appearance.stroke_width_px, 12);
    assert_eq!(appearance.history_depth, 128);
    assert!((appearance.canvas_alpha - 0.2).abs() < f32::EPSILON);
}

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

    run_overlay(AnnotationAppearance::default(), rx).unwrap();
}
