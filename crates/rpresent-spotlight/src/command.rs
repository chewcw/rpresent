use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightCommand {
    On,
    Off,
    Toggle,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandParseError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
}

impl FromStr for SpotlightCommand {
    type Err = CommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "spot=on" => Ok(Self::On),
            "spot=off" => Ok(Self::Off),
            "spot=toggle" => Ok(Self::Toggle),
            other => Err(CommandParseError::UnknownCommand(other.to_owned())),
        }
    }
}

impl Display for SpotlightCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SpotlightCommand::On => write!(f, "spot=on"),
            SpotlightCommand::Off => write!(f, "spot=off"),
            SpotlightCommand::Toggle => write!(f, "spot=toggle"),
        }
    }
}
