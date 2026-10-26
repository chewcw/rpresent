use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

pub use keybinds::{Key, KeyInput, Mods};

mod backend;
pub use backend::{
    BackendError, EditorInput, KeyboardEvent, ShortcutBackend, ShortcutEvent, ShortcutEventSink,
    WheelEvent,
};

/// Raw shortcut tables as they appear in the configuration file.
///
/// An absent table keeps the scope defaults. An explicit table replaces the
/// whole scope, so an empty table disables that scope's bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShortcutConfig {
    #[serde(default = "default_global")]
    pub global: BTreeMap<String, String>,
    #[serde(default = "default_annotation")]
    pub annotation: BTreeMap<String, String>,
    #[serde(default)]
    pub wheel: WheelConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WheelConfig {
    pub modifiers: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    Annotation,
}

impl Scope {
    fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Annotation => "annotation",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Global => Self::Annotation,
            Self::Annotation => Self::Global,
        }
    }
}

/// A validated binding: the action, the expression the user wrote, and the
/// parsed package input used for matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub action: ShortcutAction,
    pub expression: String,
    pub input: KeyInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedShortcuts {
    pub global: Vec<Binding>,
    pub annotation: Vec<Binding>,
    pub wheel: Mods,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid shortcut {field} = {value:?}: {reason}")]
pub struct ShortcutError {
    pub field: String,
    pub value: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    SpotlightToggle,
    AnnotationToggle,
    ZoomToggle,
    AnnotationUndo,
    AnnotationRedo,
    AnnotationDelete,
    AnnotationClear,
    ToolSelect,
    ToolPen,
    ToolLine,
    ToolArrow,
    ToolRect,
    ToolEraser,
    ToolText,
    ColorNext,
    ColorPrev,
    WidthPrev,
    WidthNext,
    TextSizePrev,
    TextSizeNext,
}

impl ShortcutAction {
    /// Parse an action name scoped to one table. This parses application
    /// action names, not keyboard syntax.
    pub fn parse(name: &str, scope: Scope) -> Option<Self> {
        let action = match name {
            "spotlight.toggle" => Self::SpotlightToggle,
            "annotation.toggle" => Self::AnnotationToggle,
            "zoom.toggle" => Self::ZoomToggle,
            "annotation.undo" => Self::AnnotationUndo,
            "annotation.redo" => Self::AnnotationRedo,
            "annotation.delete" => Self::AnnotationDelete,
            "annotation.clear" => Self::AnnotationClear,
            "annotation.tool.select" => Self::ToolSelect,
            "annotation.tool.pen" => Self::ToolPen,
            "annotation.tool.line" => Self::ToolLine,
            "annotation.tool.arrow" => Self::ToolArrow,
            "annotation.tool.rect" => Self::ToolRect,
            "annotation.tool.eraser" => Self::ToolEraser,
            "annotation.tool.text" => Self::ToolText,
            "annotation.color.next" => Self::ColorNext,
            "annotation.color.prev" => Self::ColorPrev,
            "annotation.width.prev" => Self::WidthPrev,
            "annotation.width.next" => Self::WidthNext,
            "annotation.text-size.prev" => Self::TextSizePrev,
            "annotation.text-size.next" => Self::TextSizeNext,
            _ => return None,
        };
        (action.scope() == scope).then_some(action)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::SpotlightToggle => "spotlight.toggle",
            Self::AnnotationToggle => "annotation.toggle",
            Self::ZoomToggle => "zoom.toggle",
            Self::AnnotationUndo => "annotation.undo",
            Self::AnnotationRedo => "annotation.redo",
            Self::AnnotationDelete => "annotation.delete",
            Self::AnnotationClear => "annotation.clear",
            Self::ToolSelect => "annotation.tool.select",
            Self::ToolPen => "annotation.tool.pen",
            Self::ToolLine => "annotation.tool.line",
            Self::ToolArrow => "annotation.tool.arrow",
            Self::ToolRect => "annotation.tool.rect",
            Self::ToolEraser => "annotation.tool.eraser",
            Self::ToolText => "annotation.tool.text",
            Self::ColorNext => "annotation.color.next",
            Self::ColorPrev => "annotation.color.prev",
            Self::WidthPrev => "annotation.width.prev",
            Self::WidthNext => "annotation.width.next",
            Self::TextSizePrev => "annotation.text-size.prev",
            Self::TextSizeNext => "annotation.text-size.next",
        }
    }

    fn scope(self) -> Scope {
        match self {
            Self::SpotlightToggle | Self::AnnotationToggle | Self::ZoomToggle => Scope::Global,
            _ => Scope::Annotation,
        }
    }
}

#[derive(Debug)]
pub struct ShortcutDispatcher(keybinds::Keybinds<ShortcutAction>);

impl ShortcutDispatcher {
    pub fn new(bindings: &[Binding]) -> Self {
        Self(keybinds::Keybinds::new(
            bindings
                .iter()
                .map(|binding| keybinds::Keybind::new(binding.input, binding.action))
                .collect(),
        ))
    }

    /// Dispatch a normalized input. The package owns matching semantics, so
    /// uppercase and lowercase character bindings stay distinct.
    pub fn dispatch(&mut self, input: KeyInput) -> Option<ShortcutAction> {
        self.0.dispatch(input).copied()
    }
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            global: default_global(),
            annotation: default_annotation(),
            wheel: WheelConfig::default(),
        }
    }
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            modifiers: "Ctrl".into(),
        }
    }
}

impl ShortcutConfig {
    pub fn validate(&self) -> Result<ValidatedShortcuts, ShortcutError> {
        Ok(ValidatedShortcuts {
            global: validate_scope(Scope::Global, &self.global)?,
            annotation: validate_scope(Scope::Annotation, &self.annotation)?,
            wheel: parse_wheel_modifiers(&self.wheel.modifiers)?,
        })
    }
}

fn validate_scope(
    scope: Scope,
    map: &BTreeMap<String, String>,
) -> Result<Vec<Binding>, ShortcutError> {
    let mut seen: HashMap<KeyInput, String> = HashMap::new();
    let mut bindings = Vec::with_capacity(map.len());
    for (name, expression) in map {
        let field = format!("shortcuts.{}.{name}", scope.as_str());
        let Some(action) = ShortcutAction::parse(name, scope) else {
            let reason = if ShortcutAction::parse(name, scope.other()).is_some() {
                "action is not valid in this scope"
            } else {
                "unknown action"
            };
            return Err(ShortcutError {
                field,
                value: expression.clone(),
                reason: reason.into(),
            });
        };
        let input = expression
            .parse::<KeyInput>()
            .map_err(|error| ShortcutError {
                field: field.clone(),
                value: expression.clone(),
                reason: error.to_string(),
            })?;
        if let Some(first) = seen.insert(input, name.clone()) {
            return Err(ShortcutError {
                field,
                value: expression.clone(),
                reason: format!("duplicate binding of {first}"),
            });
        }
        bindings.push(Binding {
            action,
            expression: expression.clone(),
            input,
        });
    }
    Ok(bindings)
}

/// Parse a wheel modifier expression such as `Super+Shift`.
///
/// Tokens are parsed by the package so accepted casing and aliases match
/// keyboard expressions. Repeated modifiers are idempotent.
pub fn parse_wheel_modifiers(value: &str) -> Result<Mods, ShortcutError> {
    let failure = |reason: String| ShortcutError {
        field: "shortcuts.wheel.modifiers".into(),
        value: value.into(),
        reason,
    };
    if value.trim().is_empty() {
        return Err(failure("empty modifier expression".into()));
    }
    let mut modifiers = Mods::NONE;
    for token in value.split('+') {
        if token.trim().is_empty() {
            return Err(failure("empty modifier token".into()));
        }
        modifiers |= token
            .parse::<Mods>()
            .map_err(|error| failure(error.to_string()))?;
    }
    Ok(modifiers)
}

fn default_global() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("spotlight.toggle".into(), "Ctrl+Alt+1".into()),
        ("annotation.toggle".into(), "Ctrl+Alt+2".into()),
        ("zoom.toggle".into(), "Ctrl+Alt+3".into()),
    ])
}

fn default_annotation() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("annotation.undo".into(), "Ctrl+z".into()),
        ("annotation.redo".into(), "Ctrl+Z".into()),
        ("annotation.delete".into(), "Delete".into()),
        ("annotation.clear".into(), "x".into()),
        ("annotation.tool.select".into(), "v".into()),
        ("annotation.tool.pen".into(), "p".into()),
        ("annotation.tool.line".into(), "l".into()),
        ("annotation.tool.arrow".into(), "a".into()),
        ("annotation.tool.rect".into(), "r".into()),
        ("annotation.tool.eraser".into(), "e".into()),
        ("annotation.tool.text".into(), "t".into()),
        ("annotation.color.next".into(), "c".into()),
        ("annotation.color.prev".into(), "C".into()),
        ("annotation.width.prev".into(), "[".into()),
        ("annotation.width.next".into(), "]".into()),
        ("annotation.text-size.prev".into(), "-".into()),
        ("annotation.text-size.next".into(), "=".into()),
    ])
}
