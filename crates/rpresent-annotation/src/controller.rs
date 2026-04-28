use crate::command::AnnotationCommand;
use crate::history::{Stroke, StrokeHistory};
use rpresent_config::AnnotationTool;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationState {
    On,
    Off,
}

pub struct AnnotationController {
    state: AnnotationState,
    active_tool: AnnotationTool,
    history: StrokeHistory,
}

impl AnnotationController {
    pub fn new(initial_enabled: bool, default_tool: AnnotationTool, history_depth: usize) -> Self {
        Self {
            state: if initial_enabled {
                AnnotationState::On
            } else {
                AnnotationState::Off
            },
            active_tool: default_tool,
            history: StrokeHistory::new(history_depth),
        }
    }

    pub fn state(&self) -> AnnotationState {
        self.state
    }

    pub fn active_tool(&self) -> AnnotationTool {
        self.active_tool
    }

    pub fn strokes(&self) -> &[Stroke] {
        self.history.strokes()
    }

    pub fn record_stroke(&mut self, points: Vec<(i32, i32)>) {
        self.history.push(Stroke::new(self.active_tool, points));
    }

    pub fn apply(&mut self, command: AnnotationCommand) -> bool {
        match command {
            AnnotationCommand::On => self.set_state(AnnotationState::On),
            AnnotationCommand::Off => self.set_state(AnnotationState::Off),
            AnnotationCommand::Toggle => self.toggle_state(),
            AnnotationCommand::Clear => {
                self.history.clear();
                true
            }
            AnnotationCommand::Undo => self.history.undo().is_some(),
            AnnotationCommand::Redo => self.history.redo().is_some(),
            AnnotationCommand::Tool(tool) => {
                let changed = tool != self.active_tool;
                self.active_tool = tool;
                changed
            }
        }
    }

    fn set_state(&mut self, next: AnnotationState) -> bool {
        let changed = self.state != next;
        self.state = next;
        changed
    }

    fn toggle_state(&mut self) -> bool {
        let next = match self.state {
            AnnotationState::On => AnnotationState::Off,
            AnnotationState::Off => AnnotationState::On,
        };
        self.set_state(next)
    }
}
