use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomCommand {
    On,
    Off,
    Toggle,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CommandParseError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
}

impl FromStr for ZoomCommand {
    type Err = CommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "zoom=on" => Ok(Self::On),
            "zoom=off" => Ok(Self::Off),
            "zoom=toggle" => Ok(Self::Toggle),
            "zoom=reset" => Ok(Self::Reset),
            other => Err(CommandParseError::UnknownCommand(other.to_owned())),
        }
    }
}

impl Display for ZoomCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ZoomCommand::On => f.write_str("zoom=on"),
            ZoomCommand::Off => f.write_str("zoom=off"),
            ZoomCommand::Toggle => f.write_str("zoom=toggle"),
            ZoomCommand::Reset => f.write_str("zoom=reset"),
        }
    }
}
