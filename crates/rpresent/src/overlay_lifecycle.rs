//! Acknowledged window lifecycle for spotlight, zoom, and a future annotation
//! layer.
//!
//! The reducer is pure: it never performs native I/O. Callers translate each
//! [`Effect`] into a worker request and feed the result back through
//! [`WindowLifecycle::complete`]. An effect always identifies a single native
//! operation, so acknowledgments match one initiating transition.
//!
//! Callers must apply acknowledged native state to their [`LayerSnapshot`]
//! before calling [`WindowLifecycle::complete`]: the snapshot is the source of
//! truth for which upper windows are mapped, and the lifecycle uses it to avoid
//! repeating native work that already happened.

use std::cell::Cell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rpresent_zoom::{MonitorRect, ZoomCommand};

/// How long one native acknowledgment may take before the operation is
/// abandoned and recovery starts. This is an implementation bound, not a
/// configuration field.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(2);

/// Visual layers, from lowest to highest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Zoom,
    Spotlight,
    Annotation,
}

impl Layer {
    fn index(self) -> usize {
        match self {
            Self::Zoom => 0,
            Self::Spotlight => 1,
            Self::Annotation => 2,
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Self::Zoom => "zoom",
            Self::Spotlight => "spotlight",
            Self::Annotation => "annotation",
        }
    }
}

const LAYERS: [Layer; 3] = [Layer::Zoom, Layer::Spotlight, Layer::Annotation];

/// Native facts about one layer. `requested` is controller intent; `mapped` is
/// the last acknowledged native visibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerState {
    pub requested: bool,
    pub window: Option<u32>,
    pub mapped: bool,
    pub input_ready: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LayerSnapshot {
    pub zoom: LayerState,
    pub spotlight: LayerState,
    pub annotation: LayerState,
}

impl LayerSnapshot {
    pub fn state(&self, layer: Layer) -> LayerState {
        match layer {
            Layer::Zoom => self.zoom,
            Layer::Spotlight => self.spotlight,
            Layer::Annotation => self.annotation,
        }
    }

    /// Mapped windows in fixed stacking positions, `[zoom, spotlight, annotation]`.
    pub fn bottom_to_top(&self) -> [Option<u32>; 3] {
        [
            mapped_window(self.zoom),
            mapped_window(self.spotlight),
            mapped_window(self.annotation),
        ]
    }
}

fn mapped_window(state: LayerState) -> Option<u32> {
    if state.mapped { state.window } else { None }
}

/// One native operation. New identifiers are allocated per effect, not per
/// activation, so a stale acknowledgment cannot advance a later transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    SelectZoom {
        operation: u64,
    },
    Present {
        operation: u64,
        layer: Layer,
        visible: bool,
    },
    PrepareZoom {
        operation: u64,
        monitor: MonitorRect,
    },
    Restack {
        operation: u64,
        bottom_to_top: [Option<u32>; 3],
    },
    SetOwner {
        operation: u64,
        window: Option<u32>,
        restore_focus: bool,
    },
    CommitZoom {
        operation: u64,
    },
    CloseZoom {
        operation: u64,
    },
    ResetZoom {
        operation: u64,
    },
    CancelZoomMotion {
        operation: u64,
    },
}

impl Effect {
    pub fn operation(&self) -> u64 {
        match self {
            Self::SelectZoom { operation }
            | Self::Present { operation, .. }
            | Self::PrepareZoom { operation, .. }
            | Self::Restack { operation, .. }
            | Self::SetOwner { operation, .. }
            | Self::CommitZoom { operation }
            | Self::CloseZoom { operation }
            | Self::ResetZoom { operation }
            | Self::CancelZoomMotion { operation } => *operation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    MonitorSelected(MonitorRect),
    Presented { layer: Layer, visible: bool },
    ZoomPrepared { window: u32 },
    NativeApplied,
    ZoomCommitted,
    ZoomClosed,
    ZoomReset,
    MotionCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Monitor,
    Present { layer: Layer, visible: bool },
    Prepared,
    Restack,
    Owner,
    Commit,
    Close,
    Reset,
    CancelMotion,
}

impl Expect {
    fn describe(self) -> &'static str {
        match self {
            Self::Monitor => "monitor selection",
            Self::Present { layer, .. } => layer.describe(),
            Self::Prepared => "zoom preparation",
            Self::Restack => "window restacking",
            Self::Owner => "pointer ownership",
            Self::Commit => "zoom commit",
            Self::Close => "zoom close",
            Self::Reset => "zoom reset",
            Self::CancelMotion => "zoom motion cancellation",
        }
    }

    fn matches(self, completion: Completion) -> bool {
        match (self, completion) {
            (Self::Monitor, Completion::MonitorSelected(_))
            | (Self::Prepared, Completion::ZoomPrepared { .. })
            | (Self::Restack | Self::Owner, Completion::NativeApplied)
            | (Self::Commit, Completion::ZoomCommitted)
            | (Self::Close, Completion::ZoomClosed)
            | (Self::Reset, Completion::ZoomReset)
            | (Self::CancelMotion, Completion::MotionCancelled) => true,
            (
                Self::Present { layer, visible },
                Completion::Presented {
                    layer: done_layer,
                    visible: done_visible,
                },
            ) => layer == done_layer && visible == done_visible,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Outstanding {
    operation: u64,
    expect: Expect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// No zoom window and nothing suppressed.
    Idle,
    /// Hiding participating overlays before capture.
    Suppressing,
    /// Waiting for the prepared zoom window.
    Preparing,
    /// Restoring overlay visibility and order after capture.
    Restoring,
    /// Waiting for pointer ownership confirmation before commit.
    AcquiringOwner,
    /// Waiting for the commit acknowledgment.
    Committing,
    /// Zoom is published as active.
    Active,
    /// Waiting for a reset acknowledgment.
    Resetting,
    /// Cancelling gestures, releasing ownership, closing zoom, restoring.
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopStep {
    CancelMotion,
    Release,
    Close,
    Restore,
    Done,
}

pub struct WindowLifecycle {
    next_operation: Cell<u64>,
    stage: Stage,
    stop_step: StopStep,
    suppressed: bool,
    committed: bool,
    zoom_window: Option<u32>,
    zoom_mapped: bool,
    mapped: [bool; 3],
    selected_monitor: Option<MonitorRect>,
    applied_order: [Option<u32>; 3],
    pending_order: Option<[Option<u32>; 3]>,
    owner_window: Option<u32>,
    pending_owner: Option<Option<u32>>,
    outstanding: Option<Outstanding>,
    deadline: Option<Instant>,
    present_blocked: [bool; 3],
    stack_blocked: bool,
    queued: VecDeque<ZoomCommand>,
    errors: Vec<String>,
}

impl Default for WindowLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowLifecycle {
    pub fn new() -> Self {
        Self {
            next_operation: Cell::new(1),
            stage: Stage::Idle,
            stop_step: StopStep::Done,
            suppressed: false,
            committed: false,
            zoom_window: None,
            zoom_mapped: false,
            mapped: [false; 3],
            selected_monitor: None,
            applied_order: [None; 3],
            pending_order: None,
            owner_window: None,
            pending_owner: None,
            outstanding: None,
            deadline: None,
            present_blocked: [false; 3],
            stack_blocked: false,
            queued: VecDeque::new(),
            errors: Vec::new(),
        }
    }

    /// Whether zoom finished committing and remains published. Closing starts
    /// as soon as an Off request arrives, so wheel routing stops then too.
    pub fn is_zoom_active(&self) -> bool {
        self.committed
    }

    /// Whether a capture window is currently hiding participating overlays.
    pub fn is_suppressed(&self) -> bool {
        self.suppressed
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Drain diagnostics collected from failed and abandoned operations.
    pub fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }

    /// Accept a zoom command. Commands that cannot start yet are queued so the
    /// order the user issued them in survives the current transition.
    pub fn request_zoom(
        &mut self,
        command: ZoomCommand,
        layers: LayerSnapshot,
        now: Instant,
    ) -> Vec<Effect> {
        let changed = match command {
            ZoomCommand::On => match self.stage {
                Stage::Idle => {
                    self.begin_activation();
                    true
                }
                Stage::Active => false,
                Stage::Stopping | Stage::Resetting => {
                    self.queued.push_back(ZoomCommand::On);
                    false
                }
                _ => false,
            },
            ZoomCommand::Off => match self.stage {
                Stage::Idle | Stage::Stopping => false,
                _ => {
                    self.begin_stopping();
                    true
                }
            },
            ZoomCommand::Toggle => {
                if self.committed {
                    self.begin_stopping();
                    true
                } else {
                    match self.stage {
                        Stage::Idle => {
                            self.begin_activation();
                            true
                        }
                        Stage::Stopping | Stage::Resetting => {
                            self.queued.push_back(ZoomCommand::Toggle);
                            false
                        }
                        _ => false,
                    }
                }
            }
            ZoomCommand::Reset => match self.stage {
                Stage::Idle => false,
                Stage::Active => {
                    self.begin_reset();
                    true
                }
                _ => {
                    self.queued.push_back(ZoomCommand::Reset);
                    false
                }
            },
        };

        if changed {
            self.pump(&layers, now)
        } else {
            Vec::new()
        }
    }

    /// Accept one native acknowledgment. `result` mirrors the worker outcome for
    /// the operation; a failure starts recovery instead of advancing.
    pub fn complete(
        &mut self,
        operation: u64,
        result: Result<Completion, String>,
        layers: LayerSnapshot,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(outstanding) = self.outstanding else {
            return Vec::new();
        };
        if outstanding.operation != operation {
            return Vec::new();
        }

        match result {
            Ok(completion) => {
                if !outstanding.expect.matches(completion) {
                    // Wrong completion kind: keep waiting on the same operation.
                    return Vec::new();
                }
                self.outstanding = None;
                self.deadline = None;
                self.apply_completion(outstanding.expect, completion);
            }
            Err(error) => {
                self.outstanding = None;
                self.deadline = None;
                self.errors
                    .push(format!("{} failed: {error}", outstanding.expect.describe()));
                if self.stage == Stage::Stopping {
                    if self.stop_step == StopStep::Restore {
                        // Keep restoring the items that still can succeed.
                        self.block_failed(outstanding.expect);
                    } else {
                        self.advance_stop_step();
                    }
                } else {
                    self.begin_stopping();
                }
            }
        }

        self.pump(&layers, now)
    }

    /// The controller or runtime reported a visibility change. Reconcile the
    /// effective presentation and order, skipping work that already agrees.
    pub fn visibility_changed(
        &mut self,
        _layer: Layer,
        layers: LayerSnapshot,
        now: Instant,
    ) -> Vec<Effect> {
        self.pump(&layers, now)
    }

    /// Abandon an overdue operation. Recovery continues from whichever step is
    /// still applicable, so overlays are never left suppressed.
    pub fn expire(&mut self, layers: LayerSnapshot, now: Instant) -> Vec<Effect> {
        let Some(deadline) = self.deadline else {
            return Vec::new();
        };
        if now < deadline {
            return Vec::new();
        }

        let description = self
            .outstanding
            .map_or("native acknowledgment", |outstanding| {
                outstanding.expect.describe()
            });
        self.errors
            .push(format!("timed out waiting for {description}"));
        self.outstanding = None;
        self.deadline = None;

        match self.stage {
            Stage::Idle | Stage::Active => {}
            _ => self.begin_stopping(),
        }

        self.pump(&layers, now)
    }

    fn begin_activation(&mut self) {
        self.suppressed = true;
        self.committed = false;
        self.selected_monitor = None;
        self.zoom_window = None;
        self.zoom_mapped = false;
        self.mapped[Layer::Zoom.index()] = false;
        self.present_blocked = [false; 3];
        self.stack_blocked = false;
        self.stage = Stage::Suppressing;
    }

    fn begin_reset(&mut self) {
        self.stage = Stage::Resetting;
    }

    fn begin_stopping(&mut self) {
        self.suppressed = false;
        self.committed = false;
        self.stage = Stage::Stopping;
        self.stop_step = StopStep::CancelMotion;
        self.outstanding = None;
        self.deadline = None;
    }

    /// Keep recovery moving when one restore step cannot complete.
    fn block_failed(&mut self, expect: Expect) {
        match expect {
            Expect::Present { layer, .. } => self.present_blocked[layer.index()] = true,
            Expect::Restack => self.stack_blocked = true,
            _ => {}
        }
    }

    fn advance_stop_step(&mut self) {
        self.stop_step = match self.stop_step {
            StopStep::CancelMotion => StopStep::Release,
            StopStep::Release => StopStep::Close,
            StopStep::Close => StopStep::Restore,
            StopStep::Restore => StopStep::Done,
            StopStep::Done => StopStep::Done,
        };
    }

    fn apply_completion(&mut self, expect: Expect, completion: Completion) {
        match (expect, completion) {
            (Expect::Monitor, Completion::MonitorSelected(monitor)) => {
                self.selected_monitor = Some(monitor);
            }
            (Expect::Present { .. }, Completion::Presented { layer, visible }) => {
                self.mapped[layer.index()] = visible;
            }
            (Expect::Prepared, Completion::ZoomPrepared { window }) => {
                self.zoom_window = Some(window);
                self.zoom_mapped = true;
                self.mapped[Layer::Zoom.index()] = true;
                // Capture is complete: eligible overlays may return.
                self.suppressed = false;
                self.stage = Stage::Restoring;
            }
            (Expect::Restack, Completion::NativeApplied) => {
                if let Some(order) = self.pending_order.take() {
                    self.applied_order = order;
                }
            }
            (Expect::Owner, Completion::NativeApplied) => {
                self.owner_window = self.pending_owner.take().flatten();
                if self.stage == Stage::Stopping {
                    self.advance_stop_step();
                } else {
                    self.stage = Stage::Committing;
                }
            }
            (Expect::Commit, Completion::ZoomCommitted) => {
                self.committed = true;
                self.stage = Stage::Active;
            }
            (Expect::Close, Completion::ZoomClosed) => {
                if self.owner_window == self.zoom_window {
                    self.owner_window = None;
                }
                self.zoom_window = None;
                self.zoom_mapped = false;
                self.mapped[Layer::Zoom.index()] = false;
                if self.stage == Stage::Stopping {
                    self.advance_stop_step();
                }
            }
            (Expect::Reset, Completion::ZoomReset) => {
                if self.stage == Stage::Resetting {
                    self.stage = Stage::Active;
                }
            }
            (Expect::CancelMotion, Completion::MotionCancelled) => {
                if self.stage == Stage::Stopping {
                    self.advance_stop_step();
                }
            }
            _ => {}
        }
    }

    /// Return the next effect to emit, or nothing when the current stage is
    /// either waiting for an acknowledgment or settled.
    fn pump(&mut self, layers: &LayerSnapshot, now: Instant) -> Vec<Effect> {
        loop {
            if self.outstanding.is_some() {
                return Vec::new();
            }

            match self.stage {
                Stage::Idle | Stage::Active => {
                    if self.drain_next_command() {
                        continue;
                    }
                    return self.reconcile(layers, now, true);
                }
                Stage::Suppressing => {
                    if self.selected_monitor.is_none() {
                        return self.emit(
                            Effect::SelectZoom {
                                operation: self.next_operation(),
                            },
                            Expect::Monitor,
                            now,
                        );
                    }
                    let effects = self.reconcile(layers, now, false);
                    if !effects.is_empty() {
                        return effects;
                    }
                    let Some(monitor) = self.selected_monitor else {
                        self.errors
                            .push("monitor selection was not recorded".to_owned());
                        self.begin_stopping();
                        continue;
                    };
                    self.stage = Stage::Preparing;
                    return self.emit(
                        Effect::PrepareZoom {
                            operation: self.next_operation(),
                            monitor,
                        },
                        Expect::Prepared,
                        now,
                    );
                }
                Stage::Preparing => return Vec::new(),
                Stage::Restoring => {
                    let effects = self.reconcile(layers, now, true);
                    if !effects.is_empty() {
                        return effects;
                    }
                    match self.preferred_owner_window(layers) {
                        Some(window) => {
                            self.stage = Stage::AcquiringOwner;
                            self.pending_owner = Some(Some(window));
                            return self.emit(
                                Effect::SetOwner {
                                    operation: self.next_operation(),
                                    window: Some(window),
                                    restore_focus: false,
                                },
                                Expect::Owner,
                                now,
                            );
                        }
                        None => {
                            self.stage = Stage::Committing;
                            return self.emit(
                                Effect::CommitZoom {
                                    operation: self.next_operation(),
                                },
                                Expect::Commit,
                                now,
                            );
                        }
                    }
                }
                Stage::AcquiringOwner => {
                    // Ownership was not confirmed; never commit on that basis.
                    return Vec::new();
                }
                Stage::Committing => {
                    return self.emit(
                        Effect::CommitZoom {
                            operation: self.next_operation(),
                        },
                        Expect::Commit,
                        now,
                    );
                }
                Stage::Resetting => {
                    return self.emit(
                        Effect::ResetZoom {
                            operation: self.next_operation(),
                        },
                        Expect::Reset,
                        now,
                    );
                }
                Stage::Stopping => {
                    let effects = self.run_stop_step(layers, now);
                    if effects.is_empty() {
                        continue;
                    }
                    return effects;
                }
            }
        }
    }

    fn drain_next_command(&mut self) -> bool {
        let Some(command) = self.queued.pop_front() else {
            return false;
        };

        match command {
            ZoomCommand::On => {
                if self.stage == Stage::Idle {
                    self.begin_activation();
                }
            }
            ZoomCommand::Off => {
                if self.stage == Stage::Active {
                    self.begin_stopping();
                }
            }
            ZoomCommand::Toggle => {
                if self.committed {
                    self.begin_stopping();
                } else if self.stage == Stage::Idle {
                    self.begin_activation();
                }
            }
            ZoomCommand::Reset => {
                // Reset while inactive stays inactive.
                if self.stage == Stage::Active {
                    self.begin_reset();
                }
            }
        }

        true
    }

    fn run_stop_step(&mut self, layers: &LayerSnapshot, now: Instant) -> Vec<Effect> {
        match self.stop_step {
            StopStep::CancelMotion => self.emit(
                Effect::CancelZoomMotion {
                    operation: self.next_operation(),
                },
                Expect::CancelMotion,
                now,
            ),
            StopStep::Release => {
                if self.owner_window.is_none() {
                    self.stop_step = StopStep::Close;
                    return Vec::new();
                }
                // Transfer to a ready annotation window, or give focus back to
                // the desktop window that held it before zoom.
                let transfer = self.eligible_annotation_window(layers);
                self.pending_owner = Some(transfer);
                self.emit(
                    Effect::SetOwner {
                        operation: self.next_operation(),
                        window: transfer,
                        restore_focus: transfer.is_none(),
                    },
                    Expect::Owner,
                    now,
                )
            }
            StopStep::Close => self.emit(
                Effect::CloseZoom {
                    operation: self.next_operation(),
                },
                Expect::Close,
                now,
            ),
            StopStep::Restore => {
                let effects = self.reconcile(layers, now, true);
                if !effects.is_empty() {
                    return effects;
                }
                self.stop_step = StopStep::Done;
                Vec::new()
            }
            StopStep::Done => {
                self.stage = Stage::Idle;
                self.applied_order = self.effective_order(layers);
                Vec::new()
            }
        }
    }

    fn reconcile(&mut self, layers: &LayerSnapshot, now: Instant, restack: bool) -> Vec<Effect> {
        self.sync(layers);

        for layer in [Layer::Spotlight, Layer::Annotation] {
            if self.present_blocked[layer.index()] {
                continue;
            }
            let desired = self.desired_visible(layer, layers);
            if self.mapped[layer.index()] != desired {
                return self.emit(
                    Effect::Present {
                        operation: self.next_operation(),
                        layer,
                        visible: desired,
                    },
                    Expect::Present {
                        layer,
                        visible: desired,
                    },
                    now,
                );
            }
        }

        if restack && !self.stack_blocked {
            let desired = self.effective_order(layers);
            let any_window = desired.iter().any(Option::is_some);
            if any_window && desired != self.applied_order {
                self.pending_order = Some(desired);
                return self.emit(
                    Effect::Restack {
                        operation: self.next_operation(),
                        bottom_to_top: desired,
                    },
                    Expect::Restack,
                    now,
                );
            }
        }

        Vec::new()
    }

    fn sync(&mut self, layers: &LayerSnapshot) {
        self.mapped[Layer::Spotlight.index()] = layers.spotlight.mapped;
        self.mapped[Layer::Annotation.index()] = layers.annotation.mapped;
        if self.zoom_window.is_none() {
            self.zoom_window = layers.zoom.window;
        }
        self.mapped[Layer::Zoom.index()] = self.zoom_mapped;
    }

    fn desired_visible(&self, layer: Layer, layers: &LayerSnapshot) -> bool {
        !self.suppressed && layers.state(layer).window.is_some() && layers.state(layer).requested
    }

    fn effective_order(&self, layers: &LayerSnapshot) -> [Option<u32>; 3] {
        let mut order = [None; 3];
        for layer in LAYERS {
            if !self.mapped[layer.index()] {
                continue;
            }
            order[layer.index()] = match layer {
                Layer::Zoom => self.zoom_window.or(layers.zoom.window),
                Layer::Spotlight => layers.spotlight.window,
                Layer::Annotation => layers.annotation.window,
            };
        }
        order
    }

    /// Annotation takes drawing priority once it is ready; otherwise zoom owns
    /// input for the activation.
    fn preferred_owner_window(&self, layers: &LayerSnapshot) -> Option<u32> {
        self.eligible_annotation_window(layers).or(self.zoom_window)
    }

    fn eligible_annotation_window(&self, layers: &LayerSnapshot) -> Option<u32> {
        if layers.annotation.requested && layers.annotation.input_ready {
            layers.annotation.window
        } else {
            None
        }
    }

    fn emit(&mut self, effect: Effect, expect: Expect, now: Instant) -> Vec<Effect> {
        self.outstanding = Some(Outstanding {
            operation: effect.operation(),
            expect,
        });
        self.deadline = Some(now + ACK_TIMEOUT);
        vec![effect]
    }

    fn next_operation(&self) -> u64 {
        let operation = self.next_operation.get();
        self.next_operation.set(operation + 1);
        operation
    }
}

impl std::fmt::Debug for WindowLifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowLifecycle")
            .field("stage", &self.stage)
            .field("suppressed", &self.suppressed)
            .field("committed", &self.committed)
            .field("zoom_window", &self.zoom_window)
            .field("queued", &self.queued.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPOTLIGHT: u32 = 0x20_0002;
    const ANNOTATION: u32 = 0x30_0004;
    const ZOOM: u32 = 0x40_0003;

    fn monitor() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    fn layers(spotlight: (bool, bool), annotation: (bool, bool, u32)) -> LayerSnapshot {
        let (spot_requested, spot_mapped) = spotlight;
        let (annotation_requested, annotation_ready, annotation_window) = annotation;
        LayerSnapshot {
            zoom: LayerState::default(),
            spotlight: LayerState {
                requested: spot_requested,
                window: Some(SPOTLIGHT),
                mapped: spot_mapped,
                input_ready: false,
            },
            annotation: LayerState {
                requested: annotation_requested,
                window: Some(annotation_window),
                mapped: annotation_requested,
                input_ready: annotation_ready,
            },
        }
    }

    fn describe(effect: &Effect) -> String {
        match effect {
            Effect::SelectZoom { .. } => "select".to_owned(),
            Effect::Present { layer, visible, .. } => format!(
                "present {} {}",
                layer.describe(),
                if *visible { "on" } else { "off" }
            ),
            Effect::PrepareZoom { .. } => "prepare".to_owned(),
            Effect::Restack { bottom_to_top, .. } => format!("restack {bottom_to_top:?}"),
            Effect::SetOwner {
                window,
                restore_focus,
                ..
            } => format!("owner {window:?} restore_focus={restore_focus}"),
            Effect::CommitZoom { .. } => "commit".to_owned(),
            Effect::CloseZoom { .. } => "close".to_owned(),
            Effect::ResetZoom { .. } => "reset".to_owned(),
            Effect::CancelZoomMotion { .. } => "cancel-motion".to_owned(),
        }
    }

    struct Driver {
        lifecycle: WindowLifecycle,
        layers: LayerSnapshot,
        now: Instant,
        seen: Vec<String>,
    }

    impl Driver {
        fn new(layers: LayerSnapshot) -> Self {
            Self {
                lifecycle: WindowLifecycle::new(),
                layers,
                now: Instant::now(),
                seen: Vec::new(),
            }
        }

        fn request(&mut self, command: ZoomCommand) -> Vec<Effect> {
            let effects = self.lifecycle.request_zoom(command, self.layers, self.now);
            self.record(&effects);
            effects
        }

        fn record(&mut self, effects: &[Effect]) {
            for effect in effects {
                self.seen.push(describe(effect));
            }
        }

        fn ack(&mut self, effect: &Effect) -> Vec<Effect> {
            let completion = match effect.clone() {
                Effect::SelectZoom { .. } => Completion::MonitorSelected(monitor()),
                Effect::Present { layer, visible, .. } => {
                    match layer {
                        Layer::Spotlight => self.layers.spotlight.mapped = visible,
                        Layer::Annotation => self.layers.annotation.mapped = visible,
                        Layer::Zoom => {}
                    }
                    Completion::Presented { layer, visible }
                }
                Effect::PrepareZoom { .. } => {
                    self.layers.zoom = LayerState {
                        requested: true,
                        window: Some(ZOOM),
                        mapped: true,
                        input_ready: false,
                    };
                    Completion::ZoomPrepared { window: ZOOM }
                }
                Effect::Restack { .. } | Effect::SetOwner { .. } => Completion::NativeApplied,
                Effect::CommitZoom { .. } => Completion::ZoomCommitted,
                Effect::CloseZoom { .. } => {
                    self.layers.zoom = LayerState::default();
                    Completion::ZoomClosed
                }
                Effect::ResetZoom { .. } => Completion::ZoomReset,
                Effect::CancelZoomMotion { .. } => Completion::MotionCancelled,
            };
            let effects =
                self.lifecycle
                    .complete(effect.operation(), Ok(completion), self.layers, self.now);
            self.record(&effects);
            effects
        }

        fn fail(&mut self, effect: &Effect, error: &str) -> Vec<Effect> {
            let effects = self.lifecycle.complete(
                effect.operation(),
                Err(error.to_owned()),
                self.layers,
                self.now,
            );
            self.record(&effects);
            effects
        }

        fn run(&mut self, mut effects: Vec<Effect>) {
            let mut guard = 0;
            while let Some(effect) = effects.first().cloned() {
                guard += 1;
                assert!(guard < 64, "did not settle: {:?}", self.seen);
                effects = self.ack(&effect);
            }
        }

        /// Walk the activation up to and including the effect `target`.
        fn run_until(&mut self, mut effects: Vec<Effect>, target: &str) -> Vec<Effect> {
            let mut guard = 0;
            loop {
                guard += 1;
                assert!(guard < 64, "never reached {target}: {:?}", self.seen);
                let effect = effects.first().cloned().unwrap_or_else(|| {
                    panic!("no effect while looking for {target}: {:?}", self.seen)
                });
                if describe(&effect).starts_with(target) {
                    return vec![effect];
                }
                effects = self.ack(&effect);
            }
        }
    }

    #[test]
    fn activation_starts_with_monitor_selection() {
        let mut lifecycle = WindowLifecycle::new();
        let effects =
            lifecycle.request_zoom(ZoomCommand::On, LayerSnapshot::default(), Instant::now());
        assert!(matches!(effects.as_slice(), [Effect::SelectZoom { .. }]));
        assert!(!lifecycle.is_zoom_active());
    }

    #[test]
    fn an_unrelated_acknowledgment_cannot_advance_activation() {
        let mut lifecycle = WindowLifecycle::new();
        let now = Instant::now();
        let layers = LayerSnapshot::default();
        let effects = lifecycle.request_zoom(ZoomCommand::On, layers, now);
        let operation = effects[0].operation();
        assert!(
            lifecycle
                .complete(operation + 1, Ok(Completion::NativeApplied), layers, now)
                .is_empty()
        );
        assert!(!lifecycle.is_zoom_active());
    }

    #[test]
    fn capture_waits_for_the_overlay_hide_acknowledgment() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        let effects = driver.ack(&effects[0]);
        assert_eq!(describe(&effects[0]), "present spotlight off");

        // Nothing prepares zoom until the hide is acknowledged.
        assert_eq!(driver.seen, ["select", "present spotlight off"]);

        let effects = driver.ack(&effects[0]);
        assert!(matches!(effects.as_slice(), [Effect::PrepareZoom { .. }]));
    }

    #[test]
    fn capture_restores_the_latest_requested_visibility() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        let hide = driver.ack(&effects[0]);

        // Spotlight Off arrives while the hide is still in flight.
        driver.layers.spotlight.requested = false;
        assert!(
            driver
                .lifecycle
                .visibility_changed(Layer::Spotlight, driver.layers, driver.now)
                .is_empty()
        );

        let prepare = driver.ack(&hide[0]);
        assert!(matches!(prepare.as_slice(), [Effect::PrepareZoom { .. }]));
        driver.run(prepare);

        assert!(
            !driver
                .seen
                .iter()
                .any(|seen| seen == "present spotlight on")
        );
        assert!(driver.lifecycle.is_zoom_active());
    }

    #[test]
    fn activation_restacks_in_fixed_order_and_annotation_owns_input() {
        let mut driver = Driver::new(layers((true, true), (true, true, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        driver.run(effects);

        let order = format!("restack [Some({ZOOM}), Some({SPOTLIGHT}), Some({ANNOTATION})]");
        let owner = format!("owner Some({ANNOTATION}) restore_focus=false");
        assert!(driver.seen.contains(&order), "{:?}", driver.seen);
        assert!(driver.seen.contains(&owner), "{:?}", driver.seen);
        assert!(
            driver.seen.iter().filter(|seen| *seen == "prepare").count() == 1,
            "{:?}",
            driver.seen
        );
        assert!(driver.lifecycle.is_zoom_active());
    }

    #[test]
    fn off_during_suppression_closes_and_restores_requested_visibility() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let select = driver.request(ZoomCommand::On);
        let stop = driver.request(ZoomCommand::Off);
        assert_eq!(describe(&stop[0]), "cancel-motion");
        assert!(matches!(select.as_slice(), [Effect::SelectZoom { .. }]));

        driver.run(stop);

        assert!(!driver.lifecycle.is_zoom_active());
        assert!(!driver.lifecycle.is_suppressed());
        // Spotlight was never hidden, so no restore presentation is needed.
        assert!(driver.layers.spotlight.mapped);
        assert!(!driver.seen.contains(&"present spotlight on".to_owned()));
        assert!(driver.seen.contains(&"close".to_owned()));
        assert!(driver.lifecycle.take_errors().is_empty());
    }

    #[test]
    fn queued_commands_replay_after_the_intervening_close() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let select = driver.request(ZoomCommand::On);
        let stop = driver.request(ZoomCommand::Off);
        assert_eq!(describe(&stop[0]), "cancel-motion");
        assert_eq!(describe(&select[0]), "select");

        // On arrives while the close is still in progress.
        let queued = driver.request(ZoomCommand::On);
        assert!(queued.is_empty(), "{queued:?}");
        driver.run(stop);

        let selects = driver.seen.iter().filter(|seen| *seen == "select").count();
        let closes = driver.seen.iter().filter(|seen| *seen == "close").count();
        assert_eq!(selects, 2, "{:?}", driver.seen);
        assert_eq!(closes, 1, "{:?}", driver.seen);
        assert!(driver.lifecycle.is_zoom_active());
    }

    #[test]
    fn reset_is_ignored_while_inactive_and_waits_for_the_commit() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        assert!(driver.request(ZoomCommand::Reset).is_empty());
        assert!(!driver.lifecycle.is_zoom_active());

        let effects = driver.request(ZoomCommand::On);
        let prepare = driver.run_until(effects, "prepare");
        // Reset arrives mid-activation and waits for the commit to land.
        assert!(driver.request(ZoomCommand::Reset).is_empty());
        driver.run(prepare);

        assert!(driver.lifecycle.is_zoom_active());
        assert_eq!(driver.seen.last().map(String::as_str), Some("reset"));
    }

    #[test]
    fn repeated_on_is_idempotent() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        assert_eq!(describe(&effects[0]), "select");
        assert!(driver.request(ZoomCommand::On).is_empty());
        driver.run(effects);
        assert!(driver.request(ZoomCommand::On).is_empty());
        assert_eq!(
            driver.seen.iter().filter(|seen| *seen == "select").count(),
            1
        );
    }

    #[test]
    fn expiration_abandons_the_operation_and_recovers() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let select = driver.request(ZoomCommand::On);
        let abandoned = driver
            .ack(&select[0])
            .into_iter()
            .next()
            .expect("hide effect");

        driver.now += ACK_TIMEOUT + Duration::from_millis(1);
        let recovery = driver.lifecycle.expire(driver.layers, driver.now);
        assert_eq!(describe(&recovery[0]), "cancel-motion");
        assert!(
            driver
                .lifecycle
                .take_errors()
                .iter()
                .any(|error| error.contains("timed out"))
        );

        // A late acknowledgment of the abandoned operation changes nothing.
        assert!(
            driver
                .lifecycle
                .complete(
                    abandoned.operation(),
                    Ok(Completion::Presented {
                        layer: Layer::Spotlight,
                        visible: false,
                    }),
                    driver.layers,
                    driver.now,
                )
                .is_empty()
        );
        assert!(!driver.lifecycle.is_zoom_active());
        driver.run(recovery);
        assert!(driver.layers.spotlight.mapped);
    }

    #[test]
    fn recovery_continues_after_a_failed_cancellation() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        let prepare = driver.run_until(effects, "prepare");
        let cancel = driver.fail(&prepare[0], "capture failed");
        assert_eq!(describe(&cancel[0]), "cancel-motion");

        let close = driver.fail(&cancel[0], "runtime gone");
        assert_eq!(describe(&close[0]), "close");

        driver.run(close);
        assert!(!driver.lifecycle.is_zoom_active());
        assert!(driver.layers.spotlight.mapped);
        let errors = driver.lifecycle.take_errors();
        assert!(errors.iter().any(|error| error.contains("capture failed")));
        assert!(errors.iter().any(|error| error.contains("runtime gone")));
    }

    #[test]
    fn a_failed_restore_does_not_block_the_remaining_recovery_steps() {
        let submit = |driver: &mut Driver| -> Vec<Effect> {
            let effects = driver.request(ZoomCommand::On);
            driver.run(effects);
            driver.request(ZoomCommand::Off)
        };
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let stop = submit(&mut driver);
        let acked = driver.ack(&stop[0]);
        let restore = driver.run_until(acked, "restack [None");
        let after = driver.fail(&restore[0], "stack failed");
        driver.run(after);

        assert!(!driver.lifecycle.is_zoom_active());
        assert!(
            driver
                .lifecycle
                .take_errors()
                .iter()
                .any(|error| error.contains("stack failed"))
        );
    }

    #[test]
    fn a_failed_stack_still_closes_zoom() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        let restack = driver.run_until(effects, "restack");
        let cancel = driver.fail(&restack[0], "stack failed");
        assert_eq!(describe(&cancel[0]), "cancel-motion");
        driver.run(cancel);
        assert!(!driver.lifecycle.is_zoom_active());
    }

    #[test]
    fn a_visibility_change_restacks_without_redundant_work() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects =
            driver
                .lifecycle
                .visibility_changed(Layer::Spotlight, driver.layers, driver.now);
        assert_eq!(
            describe(&effects[0]),
            format!("restack [None, Some({SPOTLIGHT}), None]")
        );

        driver.run(effects);
        assert!(
            driver
                .lifecycle
                .visibility_changed(Layer::Spotlight, driver.layers, driver.now)
                .is_empty()
        );
    }

    #[test]
    fn a_late_prepared_window_is_never_committed() {
        let mut driver = Driver::new(layers((true, true), (false, false, ANNOTATION)));
        let effects = driver.request(ZoomCommand::On);
        let prepare = driver.run_until(effects, "prepare");

        driver.now += ACK_TIMEOUT + Duration::from_millis(1);
        let recovery = driver.lifecycle.expire(driver.layers, driver.now);
        driver.run(recovery);

        assert!(
            driver
                .lifecycle
                .complete(
                    prepare[0].operation(),
                    Ok(Completion::ZoomPrepared { window: ZOOM }),
                    driver.layers,
                    driver.now,
                )
                .is_empty()
        );
        assert!(!driver.lifecycle.is_zoom_active());
    }
}
