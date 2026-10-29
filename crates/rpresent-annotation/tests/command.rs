use std::str::FromStr;

use rpresent_annotation::{AnnotationCommand, AnnotationTool, Preset, parse_rgb};

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
        AnnotationCommand::from_str("annotation=delete").unwrap(),
        AnnotationCommand::Delete
    );
    for (text, tool) in [
        ("annotation=tool=select", AnnotationTool::Select),
        ("annotation=tool=pen", AnnotationTool::Pen),
        ("annotation=tool=line", AnnotationTool::Line),
        ("annotation=tool=arrow", AnnotationTool::Arrow),
        ("annotation=tool=rect", AnnotationTool::Rect),
        ("annotation=tool=ellipse", AnnotationTool::Ellipse),
        ("annotation=tool=eraser", AnnotationTool::Eraser),
        ("annotation=tool=text", AnnotationTool::Text),
    ] {
        assert_eq!(
            AnnotationCommand::from_str(text).unwrap(),
            AnnotationCommand::Tool(tool)
        );
    }
}

#[test]
fn parses_preset_commands_and_cycles() {
    assert_eq!(
        AnnotationCommand::from_str("annotation=color=next").unwrap(),
        AnnotationCommand::Color(Preset::Next)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=color=prev").unwrap(),
        AnnotationCommand::Color(Preset::Prev)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=color=#00ff00").unwrap(),
        AnnotationCommand::Color(Preset::Value([0, 255, 0]))
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=width=next").unwrap(),
        AnnotationCommand::Width(Preset::Next)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=width=prev").unwrap(),
        AnnotationCommand::Width(Preset::Prev)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=width=10").unwrap(),
        AnnotationCommand::Width(Preset::Value(10))
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=text-size=next").unwrap(),
        AnnotationCommand::TextSize(Preset::Next)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=text-size=prev").unwrap(),
        AnnotationCommand::TextSize(Preset::Prev)
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=text-size=48").unwrap(),
        AnnotationCommand::TextSize(Preset::Value(48))
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
        (AnnotationCommand::Delete, "annotation=delete"),
        (
            AnnotationCommand::Tool(AnnotationTool::Select),
            "annotation=tool=select",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Pen),
            "annotation=tool=pen",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Line),
            "annotation=tool=line",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Arrow),
            "annotation=tool=arrow",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Rect),
            "annotation=tool=rect",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Ellipse),
            "annotation=tool=ellipse",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Eraser),
            "annotation=tool=eraser",
        ),
        (
            AnnotationCommand::Tool(AnnotationTool::Text),
            "annotation=tool=text",
        ),
        (
            AnnotationCommand::Color(Preset::Value([255, 0, 0])),
            "annotation=color=#FF0000",
        ),
        (
            AnnotationCommand::Color(Preset::Next),
            "annotation=color=next",
        ),
        (
            AnnotationCommand::Color(Preset::Prev),
            "annotation=color=prev",
        ),
        (
            AnnotationCommand::Width(Preset::Value(6)),
            "annotation=width=6",
        ),
        (
            AnnotationCommand::Width(Preset::Next),
            "annotation=width=next",
        ),
        (
            AnnotationCommand::Width(Preset::Prev),
            "annotation=width=prev",
        ),
        (
            AnnotationCommand::TextSize(Preset::Value(24)),
            "annotation=text-size=24",
        ),
        (
            AnnotationCommand::TextSize(Preset::Next),
            "annotation=text-size=next",
        ),
        (
            AnnotationCommand::TextSize(Preset::Prev),
            "annotation=text-size=prev",
        ),
    ];

    for (command, expected) in cases {
        assert_eq!(command.to_string(), expected);
    }
}

#[test]
fn every_command_round_trips_through_its_display_form() {
    let commands = [
        AnnotationCommand::Delete,
        AnnotationCommand::Tool(AnnotationTool::Text),
        AnnotationCommand::Color(Preset::Value([1, 2, 3])),
        AnnotationCommand::Color(Preset::Prev),
        AnnotationCommand::Width(Preset::Value(2)),
        AnnotationCommand::Width(Preset::Next),
        AnnotationCommand::TextSize(Preset::Value(16)),
        AnnotationCommand::TextSize(Preset::Prev),
    ];
    for command in commands {
        assert_eq!(
            AnnotationCommand::from_str(&command.to_string()).unwrap(),
            command
        );
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

#[test]
fn rejects_malformed_values_and_numeric_overflow() {
    for text in [
        "annotation=color=#12345",
        "annotation=color=ff0000",
        "annotation=color=#GGGGGG",
        "annotation=width=lots",
        "annotation=width=4294967296",
        "annotation=text-size=-1",
        "annotation=text-size=99999999999999999999",
    ] {
        assert!(
            AnnotationCommand::from_str(text).is_err(),
            "{text} should be rejected"
        );
    }
}

#[test]
fn zero_preset_values_parse_for_the_session_to_reject() {
    assert_eq!(
        AnnotationCommand::from_str("annotation=width=0").unwrap(),
        AnnotationCommand::Width(Preset::Value(0))
    );
    assert_eq!(
        AnnotationCommand::from_str("annotation=text-size=0").unwrap(),
        AnnotationCommand::TextSize(Preset::Value(0))
    );
}

#[test]
fn parse_rgb_requires_exactly_six_case_insensitive_digits() {
    assert_eq!(parse_rgb("#00fFa0").unwrap(), [0, 255, 160]);
    assert_eq!(parse_rgb("#FF0000").unwrap(), [255, 0, 0]);
    for text in ["#FFF", "#FF00000", "FF0000", "#FF00GG", ""] {
        assert!(parse_rgb(text).is_err(), "{text} should be rejected");
    }
}
