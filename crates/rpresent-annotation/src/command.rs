use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use rpresent_config::AnnotationTool;
use thiserror::Error;

use crate::catalog::{Preset, Rgb, parse_rgb};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationCommand {
    On,
    Off,
    Toggle,
    Clear,
    Undo,
    Redo,
    Delete,
    Tool(AnnotationTool),
    Color(Preset<Rgb>),
    Width(Preset<u32>),
    TextSize(Preset<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnnotationCommandParseError {
    #[error("unknown annotation command: {0}")]
    UnknownCommand(String),
    #[error("invalid annotation value {value}: expected {expected}")]
    InvalidValue {
        value: String,
        expected: &'static str,
    },
}

impl FromStr for AnnotationCommand {
    type Err = AnnotationCommandParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let command = match text {
            "annotation=on" => Self::On,
            "annotation=off" => Self::Off,
            "annotation=toggle" => Self::Toggle,
            "annotation=clear" => Self::Clear,
            "annotation=undo" => Self::Undo,
            "annotation=redo" => Self::Redo,
            "annotation=delete" => Self::Delete,
            other => {
                if let Some(name) = other.strip_prefix("annotation=tool=") {
                    Self::Tool(parse_tool(name, other)?)
                } else if let Some(value) = other.strip_prefix("annotation=color=") {
                    Self::Color(parse_color(value, other)?)
                } else if let Some(value) = other.strip_prefix("annotation=width=") {
                    Self::Width(parse_size(value, other)?)
                } else if let Some(value) = other.strip_prefix("annotation=text-size=") {
                    Self::TextSize(parse_size(value, other)?)
                } else {
                    return Err(AnnotationCommandParseError::UnknownCommand(
                        other.to_owned(),
                    ));
                }
            }
        };
        Ok(command)
    }
}

fn parse_tool(name: &str, command: &str) -> Result<AnnotationTool, AnnotationCommandParseError> {
    match name {
        "select" => Ok(AnnotationTool::Select),
        "pen" => Ok(AnnotationTool::Pen),
        "line" => Ok(AnnotationTool::Line),
        "eraser" => Ok(AnnotationTool::Eraser),
        "text" => Ok(AnnotationTool::Text),
        _ => Err(AnnotationCommandParseError::UnknownCommand(
            command.to_owned(),
        )),
    }
}

fn parse_color(value: &str, _command: &str) -> Result<Preset<Rgb>, AnnotationCommandParseError> {
    match value {
        "next" => Ok(Preset::Next),
        "prev" => Ok(Preset::Prev),
        other => parse_rgb(other).map(Preset::Value),
    }
}

fn parse_size(value: &str, _command: &str) -> Result<Preset<u32>, AnnotationCommandParseError> {
    match value {
        "next" => Ok(Preset::Next),
        "prev" => Ok(Preset::Prev),
        other => other.parse::<u32>().map(Preset::Value).map_err(|_| {
            AnnotationCommandParseError::InvalidValue {
                value: other.to_owned(),
                expected: "a positive integer, next, or prev",
            }
        }),
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
            AnnotationCommand::Delete => f.write_str("annotation=delete"),
            AnnotationCommand::Tool(tool) => match tool {
                AnnotationTool::Select => f.write_str("annotation=tool=select"),
                AnnotationTool::Pen => f.write_str("annotation=tool=pen"),
                AnnotationTool::Line => f.write_str("annotation=tool=line"),
                AnnotationTool::Eraser => f.write_str("annotation=tool=eraser"),
                AnnotationTool::Text => f.write_str("annotation=tool=text"),
            },
            AnnotationCommand::Color(preset) => match preset {
                Preset::Value(rgb) => write!(
                    f,
                    "annotation=color=#{:02X}{:02X}{:02X}",
                    rgb[0], rgb[1], rgb[2]
                ),
                Preset::Next => f.write_str("annotation=color=next"),
                Preset::Prev => f.write_str("annotation=color=prev"),
            },
            AnnotationCommand::Width(preset) => match preset {
                Preset::Value(width) => write!(f, "annotation=width={width}"),
                Preset::Next => f.write_str("annotation=width=next"),
                Preset::Prev => f.write_str("annotation=width=prev"),
            },
            AnnotationCommand::TextSize(preset) => match preset {
                Preset::Value(size) => write!(f, "annotation=text-size={size}"),
                Preset::Next => f.write_str("annotation=text-size=next"),
                Preset::Prev => f.write_str("annotation=text-size=prev"),
            },
        }
    }
}
