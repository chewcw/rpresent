use rpresent_config::AnnotationTool;

use crate::command::AnnotationCommandParseError;

pub const TOOLS: [AnnotationTool; 7] = [
    AnnotationTool::Select,
    AnnotationTool::Pen,
    AnnotationTool::Line,
    AnnotationTool::Arrow,
    AnnotationTool::Rect,
    AnnotationTool::Eraser,
    AnnotationTool::Text,
];

/// An explicit catalog value or a request to cycle through the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset<T> {
    Value(T),
    Next,
    Prev,
}

pub fn cycle_index(index: usize, count: usize, forward: bool) -> usize {
    if forward {
        (index + 1) % count
    } else {
        (index + count - 1) % count
    }
}

/// An opaque RGB color value.
pub type Rgb = [u8; 3];

/// Parse an opaque `#RRGGBB` color. Hex digits are case insensitive.
pub fn parse_rgb(text: &str) -> Result<Rgb, AnnotationCommandParseError> {
    let invalid = || AnnotationCommandParseError::InvalidValue {
        value: text.to_owned(),
        expected: "#RRGGBB",
    };
    let digits = text.strip_prefix('#').ok_or_else(invalid)?;
    if digits.len() != 6 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let channel =
        |start: usize| u8::from_str_radix(&digits[start..start + 2], 16).map_err(|_| invalid());
    Ok([channel(0)?, channel(2)?, channel(4)?])
}
