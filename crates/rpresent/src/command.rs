use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

pub use rpresent_annotation::{AnnotationCommand, AnnotationTool};
pub use rpresent_spotlight::SpotlightCommand;
pub use rpresent_zoom::ZoomCommand;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppCommand {
    Spotlight(SpotlightCommand),
    Annotation(AnnotationCommand),
    Zoom(ZoomCommand),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AppCommandParseError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error(transparent)]
    Spotlight(#[from] rpresent_spotlight::CommandParseError),
    #[error(transparent)]
    Annotation(#[from] rpresent_annotation::AnnotationCommandParseError),
    #[error(transparent)]
    Zoom(#[from] rpresent_zoom::CommandParseError),
}

impl FromStr for AppCommand {
    type Err = AppCommandParseError;

    #[tracing::instrument(level = "debug", skip(text), fields(command = %text), err)]
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.starts_with("spot=") {
            tracing::trace!(command = %text, "matched spotlight command route");
            return Ok(Self::Spotlight(text.parse()?));
        }

        if text.starts_with("annotation=") {
            tracing::trace!(command = %text, "matched annotation command route");
            return Ok(Self::Annotation(text.parse()?));
        }

        if text.starts_with("zoom=") {
            tracing::trace!(command = %text, "matched zoom command route");
            return Ok(Self::Zoom(text.parse()?));
        }

        Err(AppCommandParseError::UnknownCommand(text.to_owned()))
    }
}

impl Display for AppCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spotlight(command) => write!(f, "{command}"),
            Self::Annotation(command) => write!(f, "{command}"),
            Self::Zoom(command) => write!(f, "{command}"),
        }
    }
}
