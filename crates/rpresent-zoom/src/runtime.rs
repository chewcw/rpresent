use std::sync::Arc;

use crate::{MonitorRect, PointerInput};

/// Runtime control for the native zoom worker. Preparation is separate from
/// activation: `Prepare` captures and maps, `Commit` publishes active state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ZoomRuntimeRequest {
    SelectMonitor {
        operation: u64,
    },
    Prepare {
        operation: u64,
        monitor: MonitorRect,
    },
    Commit {
        operation: u64,
    },
    Close {
        operation: u64,
    },
    Reset {
        operation: u64,
    },
    Scroll {
        delta: f64,
        root: (i32, i32),
    },
    Pointer(PointerInput),
    CancelMotion {
        operation: u64,
    },
}

#[derive(Debug)]
pub enum ZoomEvent {
    Ready,
    Selected {
        operation: u64,
        monitor: MonitorRect,
    },
    Prepared {
        operation: u64,
        window: u32,
    },
    Committed {
        operation: u64,
    },
    Closed {
        operation: u64,
    },
    Reset {
        operation: u64,
    },
    MotionCancelled {
        operation: u64,
    },
    Failed {
        operation: u64,
        error: String,
    },
    CloseRequested,
    Exited(Result<(), String>),
}

pub type ZoomEventSink = Arc<dyn Fn(ZoomEvent) + Send + Sync>;
