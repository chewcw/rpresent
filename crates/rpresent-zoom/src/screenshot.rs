#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureMode {
    Static,
    Live { hz: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenshotState {
    mode: CaptureMode,
    refresh_requested: bool,
    acc_dt: f32,
}

impl ScreenshotState {
    pub fn new(mode: CaptureMode) -> Self {
        Self {
            mode,
            refresh_requested: false,
            acc_dt: 0.0,
        }
    }

    pub fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    pub fn should_refresh(&mut self, dt: f32) -> bool {
        match self.mode {
            CaptureMode::Static => {
                if self.refresh_requested {
                    self.refresh_requested = false;
                    true
                } else {
                    false
                }
            }
            CaptureMode::Live { hz } => {
                if hz == 0 {
                    return false;
                }

                self.acc_dt += dt;
                let interval = 1.0 / hz as f32;
                if self.acc_dt >= interval {
                    self.acc_dt -= interval;
                    true
                } else {
                    false
                }
            }
        }
    }
}
