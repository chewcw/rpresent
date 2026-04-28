use rpresent_annotation::{
    AnnotationCommand, AnnotationController, AnnotationState, AnnotationTool,
};

#[test]
fn visibility_and_tool_changes_are_applied() {
    let mut controller = AnnotationController::new(false, AnnotationTool::Pen, 64);

    assert_eq!(controller.state(), AnnotationState::Off);
    assert!(controller.apply(AnnotationCommand::On));
    assert_eq!(controller.state(), AnnotationState::On);
    assert!(controller.apply(AnnotationCommand::Toggle));
    assert_eq!(controller.state(), AnnotationState::Off);
    assert!(controller.apply(AnnotationCommand::Tool(AnnotationTool::Line)));
    assert_eq!(controller.active_tool(), AnnotationTool::Line);
}

#[test]
fn recorded_strokes_use_the_active_tool_and_history_commands_forward() {
    let mut controller = AnnotationController::new(true, AnnotationTool::Pen, 64);

    controller.record_stroke(vec![(0, 0), (4, 4)]);
    assert_eq!(controller.strokes().len(), 1);
    assert_eq!(controller.strokes()[0].tool, AnnotationTool::Pen);

    assert!(controller.apply(AnnotationCommand::Tool(AnnotationTool::Eraser)));
    controller.record_stroke(vec![(4, 4), (8, 8)]);
    assert_eq!(controller.strokes().len(), 2);
    assert_eq!(controller.strokes()[1].tool, AnnotationTool::Eraser);

    assert!(controller.apply(AnnotationCommand::Undo));
    assert_eq!(controller.strokes().len(), 1);
    assert!(controller.apply(AnnotationCommand::Redo));
    assert_eq!(controller.strokes().len(), 2);
    assert!(controller.apply(AnnotationCommand::Clear));
    assert!(controller.strokes().is_empty());
}
