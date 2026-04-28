use std::str::FromStr;

use rpresent_zoom::{CommandParseError, ZoomCommand};

#[test]
fn parses_zoom_commands() {
    assert_eq!(ZoomCommand::from_str("zoom=on").unwrap(), ZoomCommand::On);
    assert_eq!(ZoomCommand::from_str("zoom=off").unwrap(), ZoomCommand::Off);
    assert_eq!(
        ZoomCommand::from_str("zoom=toggle").unwrap(),
        ZoomCommand::Toggle
    );
    assert_eq!(
        ZoomCommand::from_str("zoom=reset").unwrap(),
        ZoomCommand::Reset
    );
}

#[test]
fn displays_zoom_commands_as_their_source_strings() {
    assert_eq!(ZoomCommand::On.to_string(), "zoom=on");
    assert_eq!(ZoomCommand::Off.to_string(), "zoom=off");
    assert_eq!(ZoomCommand::Toggle.to_string(), "zoom=toggle");
    assert_eq!(ZoomCommand::Reset.to_string(), "zoom=reset");
}

#[test]
fn rejects_unknown_commands_and_preserves_the_input_text() {
    let error = ZoomCommand::from_str("zoom=blink").unwrap_err();
    match &error {
        CommandParseError::UnknownCommand(text) => {
            assert_eq!(text, "zoom=blink");
        }
    }
    assert_eq!(error.to_string(), "unknown command: zoom=blink");
}
