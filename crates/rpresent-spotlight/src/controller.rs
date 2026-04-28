use crate::command::SpotlightCommand;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotlightState {
    On,
    Off,
}

pub struct SpotlightController {
    state: SpotlightState,
}

impl SpotlightController {
    pub fn new(initial_enabled: bool) -> Self {
        Self {
            state: if initial_enabled {
                SpotlightState::On
            } else {
                SpotlightState::Off
            },
        }
    }

    pub fn state(&self) -> SpotlightState {
        self.state
    }

    pub fn apply(&mut self, command: SpotlightCommand) -> bool {
        let next_state = match command {
            SpotlightCommand::On => SpotlightState::On,
            SpotlightCommand::Off => SpotlightState::Off,
            SpotlightCommand::Toggle => match self.state {
                SpotlightState::On => SpotlightState::Off,
                SpotlightState::Off => SpotlightState::On,
            },
        };

        let changed = next_state != self.state;
        self.state = next_state;
        changed
    }
}
