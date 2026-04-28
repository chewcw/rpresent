use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use rpresent_config::AnnotationTool;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationCommand {
    On,
    Off,
    Toggle,
    Clear,
    Undo,
    Redo,
    Tool(AnnotationTool),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnnotationToolCommandParseError {
    #[error("unknown annotation command: {0}")]
    UnknownCommand(String),
}

impl FromStr for AnnotationCommand {
    type Err = AnnotationToolCommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "annotation=on" => Ok(Self::On),
            "annotation=off" => Ok(Self::Off),
            "annotation=toggle" => Ok(Self::Toggle),
            "annotation=clear" => Ok(Self::Clear),
            "annotation=undo" => Ok(Self::Undo),
            "annotation=redo" => Ok(Self::Redo),
            "annotation=tool=pen" => Ok(Self::Tool(AnnotationTool::Pen)),
            "annotation=tool=line" => Ok(Self::Tool(AnnotationTool::Line)),
            "annotation=tool=eraser" => Ok(Self::Tool(AnnotationTool::Eraser)),
            other => Err(AnnotationToolCommandParseError::UnknownCommand(
                other.to_owned(),
            )),
        }
    }
}

impl Display for AnnotationCommand {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AnnotationCommand::On => f.write_str("annotation=on"),
            AnnotationCommand::Off => f.write_str("annotation=off"),
            AnnotationCommand::Toggle => f.write_str("annotation=toggle"),
            AnnotationCommand::Clear => f.write_str("annotation=clear"),
            AnnotationCommand::Undo => f.write_str("annotation=undo"),
            AnnotationCommand::Redo => f.write_str("annotation=redo"),
            AnnotationCommand::Tool(tool) => match tool {
                AnnotationTool::Pen => f.write_str("annotation=tool=pen"),
                AnnotationTool::Line => f.write_str("annotation=tool=line"),
                AnnotationTool::Eraser => f.write_str("annotation=tool=eraser"),
            },
        }
    }
}
