use std::str::FromStr;

use rpresent_annotation::{AnnotationCommand, AnnotationTool};

#[test]
fn parses_all_supported_commands() {
    assert_eq!(
        AnnotationCommand::from_str("annotation=on").unwrap(),
        AnnotationCommand::On
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=off").unwrap(),
        AnnotationCommand::Off
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=toggle").unwrap(),
        AnnotationCommand::Toggle
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=clear").unwrap(),
        AnnotationCommand::Clear
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=undo").unwrap(),
        AnnotationCommand::Undo
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=redo").unwrap(),
        AnnotationCommand::Redo
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=tool=pen").unwrap(),
        AnnotationCommand::Tool(AnnotationTool::Pen)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=tool=line").unwrap(),
        AnnotationCommand::Tool(AnnotationTool::Line)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=tool=eraser").unwrap(),
        AnnotationCommand::Tool(AnnotationTool::Eraser)
    );
}

#[test]
fn displays_the_original_command_strings() {
    let cases = [
        (AnnotationCommand::On, "annotation=on"),
        (AnnotationCommand::Off, "annotation=off"),
        (AnnotationCommand::Toggle, "annotation=toggle"),
        (AnnotationCommand::Clear, "annotation=clear"),
        (AnnotationCommand::Undo, "annotation=undo"),
        (AnnotationCommand::Redo, "annotation=redo"),
        (
            AnnotationCommand::Tool(AnnotationTool::Pen),
            "annotation=tool=pen",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Line),
            "annotation=tool=line",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Eraser),
            "annotation=tool=eraser",
        ),
    ];

    for (command, expected) in cases {
        assert_eq!(command.to_string(), expected);
    }
}

#[test]
fn preserves_unknown_command_text_in_the_error() {
    let error = AnnotationCommand::from_str("annotation=tool=brush").unwrap_err();
    assert_eq!(
        error.to_string(),
        "unknown annotation command: annotation=tool=brush"
    );
}
