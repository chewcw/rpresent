use std::str::FromStr;

use rpresent_spotlight::{CommandParseError, SpotlightCommand};

#[test]
fn parses_exact_spot_commands() {
    assert_eq!(
        SpotlightCommand::from_str("spot=on").unwrap(),
        SpotlightCommand::On
    );
    assert_eq!(
        SpotlightCommand::from_str("spot=off").unwrap(),
        SpotlightCommand::Off
    );
    assert_eq!(
        SpotlightCommand::from_str("spot=toggle").unwrap(),
        SpotlightCommand::Toggle
    );
}

#[test]
fn display_uses_the_same_exact_strings() {
    assert_eq!(SpotlightCommand::On.to_string(), "spot=on");
    assert_eq!(SpotlightCommand::Off.to_string(), "spot=off");
    assert_eq!(SpotlightCommand::Toggle.to_string(), "spot=toggle");
}

#[test]
fn rejects_unknown_commands_and_preserves_the_input() {
    let error = SpotlightCommand::from_str("spot=blink").unwrap_err();

    assert_eq!(
        error,
        CommandParseError::UnknownCommand("spot=blink".to_owned())
    );
    assert_eq!(error.to_string(), "unknown command: spot=blink");
}
