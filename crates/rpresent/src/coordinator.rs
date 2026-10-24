use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use rpresent_annotation::{AnnotationController, AnnotationState};
use rpresent_config::ConfigService;
use rpresent_spotlight::{
    SpotlightController, SpotlightEvent, SpotlightRuntimeMessage, SpotlightState,
};
use rpresent_zoom::{PointerInput, ZoomEvent, ZoomHandle, ZoomRuntimeRequest};

use crate::command::AppCommand;
use crate::input::{
    AppEvent, InputAction, InputBindings, InputCoordinator, InputStatus, PointerOwner, RawPointer,
};
use crate::native_input::{NativeEvent, NativeHandle, NativeRequest};
use crate::overlay_lifecycle::{
    Completion, Effect, Layer, LayerSnapshot, LayerState, WindowLifecycle,
};

pub struct CoordinatorOutputs {
    pub spotlight: std::sync::mpsc::Sender<SpotlightRuntimeMessage>,
    pub zoom: ZoomHandle,
    pub native: NativeHandle,
    pub annotation: std::sync::mpsc::Sender<AnnotationState>,
}

pub struct Coordinator {
    config: Arc<ConfigService>,
    outputs: CoordinatorOutputs,
    spotlight: SpotlightController,
    annotation: AnnotationController,
    input: InputCoordinator,
    lifecycle: WindowLifecycle,
    layers: LayerSnapshot,
    zoom_monitor: Option<rpresent_zoom::MonitorRect>,
    pending: HashMap<u64, Effect>,
    pending_wheels: VecDeque<(String, f32, (i16, i16))>,
    zoom_ready: bool,
    spotlight_ready: bool,
    native_ready: bool,
    startup_applied: bool,
    zoom_worker_failed: bool,
}

impl Coordinator {
    pub fn run(
        config: Arc<ConfigService>,
        events: Receiver<AppEvent>,
        outputs: CoordinatorOutputs,
    ) -> Result<(), String> {
        let mut coordinator = Self::new(config, outputs);
        coordinator.run_loop(events)
    }

    fn new(config: Arc<ConfigService>, outputs: CoordinatorOutputs) -> Self {
        let annotation = AnnotationController::new(
            false,
            config.annotation().default_tool,
            config.annotation().history_depth,
        );
        let mut coordinator = Self {
            input: InputCoordinator::new(InputBindings::from(config.shortcuts())),
            spotlight: SpotlightController::new(false),
            annotation,
            config,
            outputs,
            lifecycle: WindowLifecycle::new(),
            layers: LayerSnapshot::default(),
            zoom_monitor: None,
            pending: HashMap::new(),
            pending_wheels: VecDeque::new(),
            zoom_ready: false,
            spotlight_ready: false,
            native_ready: false,
            startup_applied: false,
            zoom_worker_failed: false,
        };
        coordinator.update_input_status();
        let _ = coordinator.outputs.annotation.send(AnnotationState::Off);
        coordinator
    }

    fn run_loop(&mut self, events: Receiver<AppEvent>) -> Result<(), String> {
        loop {
            let event = match self.lifecycle.next_deadline() {
                Some(deadline) => {
                    let timeout = deadline.saturating_duration_since(Instant::now());
                    match events.recv_timeout(timeout) {
                        Ok(event) => event,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            self.expire()?;
                            continue;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                }
                None => match events.recv() {
                    Ok(event) => event,
                    Err(_) => return Ok(()),
                },
            };
            self.handle_event(event)?;
        }
    }

    fn handle_event(&mut self, event: AppEvent) -> Result<(), String> {
        match event {
            AppEvent::Command(command) => self.handle_command(command),
            AppEvent::Input(action) => self.dispatch_input(action),
            AppEvent::Native(event) => self.handle_native(event),
            AppEvent::SpotlightRuntime(event) => self.handle_spotlight(event),
            AppEvent::ZoomRuntime(event) => self.handle_zoom(event),
            AppEvent::IpcFailed(error) => {
                self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off))?;
                Err(error)
            }
        }
    }

    fn handle_command(&mut self, command: AppCommand) -> Result<(), String> {
        match command {
            AppCommand::Spotlight(command) => {
                if self.spotlight.apply(command) {
                    self.layers.spotlight.requested =
                        matches!(self.spotlight.state(), SpotlightState::On);
                    self.reconcile(Layer::Spotlight)?;
                }
            }
            AppCommand::Annotation(command) => {
                if self.annotation.apply(command) {
                    self.layers.annotation.requested =
                        matches!(self.annotation.state(), AnnotationState::On);
                    self.outputs
                        .annotation
                        .send(self.annotation.state())
                        .map_err(|error| error.to_string())?;
                    self.update_input_status();
                    self.reconcile(Layer::Annotation)?;
                }
            }
            AppCommand::Zoom(command) => {
                match command {
                    rpresent_zoom::ZoomCommand::On => self.layers.zoom.requested = true,
                    rpresent_zoom::ZoomCommand::Off => self.layers.zoom.requested = false,
                    rpresent_zoom::ZoomCommand::Toggle => {
                        self.layers.zoom.requested = !self.layers.zoom.requested
                    }
                    rpresent_zoom::ZoomCommand::Reset => {}
                }
                let effects = self
                    .lifecycle
                    .request_zoom(command, self.layers, Instant::now());
                self.dispatch_effects(effects)?;
                self.update_input_status();
            }
        }
        Ok(())
    }

    fn reconcile(&mut self, layer: Layer) -> Result<(), String> {
        let effects = self
            .lifecycle
            .visibility_changed(layer, self.layers, Instant::now());
        self.dispatch_effects(effects)
    }

    fn expire(&mut self) -> Result<(), String> {
        let effects = self.lifecycle.expire(self.layers, Instant::now());
        self.dispatch_effects(effects)
    }

    fn dispatch_effects(&mut self, effects: Vec<Effect>) -> Result<(), String> {
        for effect in effects {
            self.dispatch_effect(effect)?;
        }
        Ok(())
    }

    fn dispatch_effect(&mut self, effect: Effect) -> Result<(), String> {
        let operation = effect.operation();
        self.pending.insert(operation, effect.clone());
        let send_result: Result<(), String> = match effect {
            Effect::SelectZoom { .. } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::SelectMonitor { operation })
                .map_err(|error| error.to_string()),
            Effect::Present {
                layer: Layer::Spotlight,
                visible,
                ..
            } => self
                .outputs
                .spotlight
                .send(SpotlightRuntimeMessage::Present { operation, visible })
                .map_err(|error| error.to_string()),
            Effect::Present {
                layer: Layer::Annotation,
                ..
            } => Err("annotation presentation is not available".to_owned()),
            Effect::Present {
                layer: Layer::Zoom, ..
            } => Err("zoom presentation is controlled by preparation and close".to_owned()),
            Effect::PrepareZoom { monitor, .. } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::Prepare { operation, monitor })
                .map_err(|error| error.to_string()),
            Effect::Restack { bottom_to_top, .. } => self
                .outputs
                .native
                .send(NativeRequest::Restack {
                    operation,
                    bottom_to_top,
                })
                .map_err(|error| error.to_string()),
            Effect::SetOwner {
                window,
                restore_focus,
                ..
            } => self
                .outputs
                .native
                .send(NativeRequest::SetOwner {
                    operation,
                    window,
                    restore_focus,
                })
                .map_err(|error| error.to_string()),
            Effect::CommitZoom { .. } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::Commit { operation })
                .map_err(|error| error.to_string()),
            Effect::CloseZoom { .. } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::Close { operation })
                .map_err(|error| error.to_string()),
            Effect::ResetZoom { .. } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::Reset { operation })
                .map_err(|error| error.to_string()),
            Effect::CancelZoomMotion { .. } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::CancelMotion { operation })
                .map_err(|error| error.to_string()),
        };
        if let Err(error) = send_result {
            self.pending.remove(&operation);
            let effects = self.lifecycle.complete(
                operation,
                Err(error.to_string()),
                self.layers,
                Instant::now(),
            );
            self.dispatch_effects(effects)?;
        }
        Ok(())
    }

    fn complete(
        &mut self,
        operation: u64,
        result: Result<Completion, String>,
    ) -> Result<(), String> {
        if self.pending.remove(&operation).is_none() {
            return Ok(());
        }
        let effects = self
            .lifecycle
            .complete(operation, result, self.layers, Instant::now());
        self.dispatch_effects(effects)
    }

    fn handle_spotlight(&mut self, event: SpotlightEvent) -> Result<(), String> {
        match event {
            SpotlightEvent::Ready { window } => {
                self.spotlight_ready = true;
                self.layers.spotlight.window = Some(window);
                self.maybe_startup()
            }
            SpotlightEvent::Presented {
                operation,
                visible,
                result,
            } => {
                if result.is_ok() {
                    self.layers.spotlight.mapped = visible;
                }
                self.complete(
                    operation,
                    result.map(|()| Completion::Presented {
                        layer: Layer::Spotlight,
                        visible,
                    }),
                )
            }
            SpotlightEvent::Exited(result) => {
                self.spotlight_ready = false;
                result.map_err(|error| format!("spotlight runtime exited: {error}"))
            }
        }
    }

    fn handle_zoom(&mut self, event: ZoomEvent) -> Result<(), String> {
        match event {
            ZoomEvent::Ready => {
                self.zoom_ready = true;
                self.maybe_startup()
            }
            ZoomEvent::Selected { operation, monitor } => {
                self.zoom_monitor = Some(monitor);
                self.complete(operation, Ok(Completion::MonitorSelected(monitor)))
            }
            ZoomEvent::Prepared { operation, window } => {
                if !matches!(
                    self.pending.get(&operation),
                    Some(Effect::PrepareZoom { .. })
                ) {
                    self.outputs
                        .zoom
                        .send(ZoomRuntimeRequest::Close { operation })
                        .map_err(|error| error.to_string())?;
                    return Ok(());
                }
                self.layers.zoom.window = Some(window);
                self.layers.zoom.mapped = true;
                self.complete(operation, Ok(Completion::ZoomPrepared { window }))
            }
            ZoomEvent::Committed { operation } => {
                self.complete(operation, Ok(Completion::ZoomCommitted))?;
                self.update_input_status();
                self.flush_wheels()
            }
            ZoomEvent::Closed { operation } => {
                self.layers.zoom = LayerState::default();
                self.complete(operation, Ok(Completion::ZoomClosed))?;
                self.update_input_status();
                Ok(())
            }
            ZoomEvent::Reset { operation } => self.complete(operation, Ok(Completion::ZoomReset)),
            ZoomEvent::MotionCancelled { operation } => {
                self.complete(operation, Ok(Completion::MotionCancelled))
            }
            ZoomEvent::Failed { operation, error } => self.complete(operation, Err(error)),
            ZoomEvent::CloseRequested => {
                self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off))
            }
            ZoomEvent::Exited(result) => {
                self.zoom_ready = false;
                self.zoom_worker_failed = result.is_err();
                self.update_input_status();
                if result.is_err() && self.layers.zoom.requested {
                    self.layers.zoom.requested = false;
                    let effects = self.lifecycle.request_zoom(
                        rpresent_zoom::ZoomCommand::Off,
                        self.layers,
                        Instant::now(),
                    );
                    self.dispatch_effects(effects)?;
                }
                result.map_err(|error| format!("zoom runtime exited: {error}"))
            }
        }
    }

    fn handle_native(&mut self, event: NativeEvent) -> Result<(), String> {
        match event {
            NativeEvent::Ready => {
                self.native_ready = true;
                self.maybe_startup()
            }
            NativeEvent::Applied { operation, result } => {
                let owner_window = match self.pending.get(&operation) {
                    Some(Effect::SetOwner { window, .. }) => Some(*window),
                    _ => None,
                };
                if result.is_ok() {
                    if let Some(Some(window)) = owner_window {
                        self.input.confirm_owner(
                            if window == self.layers.annotation.window.unwrap_or(0) {
                                PointerOwner::Annotation
                            } else if window == self.layers.zoom.window.unwrap_or(0) {
                                PointerOwner::Zoom
                            } else {
                                PointerOwner::Desktop
                            },
                            operation,
                        );
                    } else if owner_window == Some(None) {
                        self.input.confirm_owner(PointerOwner::Desktop, operation);
                    }
                }
                self.complete(operation, result.map(|()| Completion::NativeApplied))
            }
            NativeEvent::Pointer { epoch, input } => {
                if let Some((owner, input)) = self.input.handle_pointer(epoch, input) {
                    match owner {
                        PointerOwner::Zoom => {
                            let Some(monitor) = self.zoom_monitor else {
                                return Ok(());
                            };
                            self.outputs
                                .zoom
                                .send(ZoomRuntimeRequest::Pointer(zoom_pointer(input, monitor)))
                                .map_err(|error| error.to_string())?;
                        }
                        PointerOwner::Annotation | PointerOwner::Desktop => {}
                    }
                }
                Ok(())
            }
            NativeEvent::Wheel {
                modifier,
                delta,
                root,
            } => self.handle_wheel(&modifier, delta, root),
            NativeEvent::Key(shortcut) => {
                if let Some(command) = self.input.handle_key(&shortcut) {
                    self.handle_command(command)?;
                }
                Ok(())
            }
            NativeEvent::Escape => {
                self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off))
            }
            NativeEvent::Lost { .. } => {
                self.input.cancel_gesture();
                self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off))
            }
            NativeEvent::Exited(result) => {
                self.native_ready = false;
                result.map_err(|error| format!("native input exited: {error}"))
            }
        }
    }

    fn handle_wheel(&mut self, modifier: &str, delta: f32, root: (i16, i16)) -> Result<(), String> {
        if self.layers.zoom.requested && !self.lifecycle.is_zoom_active() {
            if self
                .input
                .handle_wheel(modifier, delta, root.0, root.1)
                .is_some()
            {
                self.pending_wheels
                    .push_back((modifier.to_owned(), delta, root));
            }
            return Ok(());
        }
        if let Some(action) = self.input.handle_wheel(modifier, delta, root.0, root.1) {
            self.dispatch_input(action)?;
        }
        Ok(())
    }

    fn dispatch_input(&mut self, action: InputAction) -> Result<(), String> {
        match action {
            InputAction::SpotlightWheel { delta } => self
                .outputs
                .spotlight
                .send(SpotlightRuntimeMessage::ResizeBy(
                    delta * self.config.spotlight().wheel_speed,
                ))
                .map_err(|error| error.to_string()),
            InputAction::ZoomWheel {
                delta,
                cursor_x,
                cursor_y,
            } => self
                .outputs
                .zoom
                .send(ZoomRuntimeRequest::Scroll {
                    delta: f64::from(delta),
                    root: (i32::from(cursor_x), i32::from(cursor_y)),
                })
                .map_err(|error| error.to_string()),
        }
    }

    fn flush_wheels(&mut self) -> Result<(), String> {
        while let Some((modifier, delta, root)) = self.pending_wheels.pop_front() {
            if let Some(action) = self.input.handle_wheel(&modifier, delta, root.0, root.1) {
                self.dispatch_input(action)?;
            }
        }
        Ok(())
    }

    fn maybe_startup(&mut self) -> Result<(), String> {
        if self.startup_applied || !self.spotlight_ready || !self.zoom_ready || !self.native_ready {
            return Ok(());
        }
        self.startup_applied = true;
        if self.config.spotlight().enabled_on_start {
            self.handle_command(AppCommand::Spotlight(
                rpresent_spotlight::SpotlightCommand::On,
            ))?;
        }
        if self.config.annotation().enabled_on_start {
            self.handle_command(AppCommand::Annotation(
                rpresent_annotation::AnnotationCommand::On,
            ))?;
        }
        if self.config.zoom().enabled_on_start {
            self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::On))?;
        }
        Ok(())
    }

    fn update_input_status(&mut self) {
        self.input.set_status(InputStatus {
            zoom_ready: self.zoom_ready && !self.zoom_worker_failed,
            zoom_active: self.lifecycle.is_zoom_active(),
            annotation_enabled: self.layers.annotation.requested,
            annotation_ready: self.layers.annotation.input_ready,
        });
    }
}

pub(crate) fn zoom_pointer(input: RawPointer, monitor: rpresent_zoom::MonitorRect) -> PointerInput {
    match input {
        RawPointer::Down { root, time } => PointerInput::Down {
            local: (
                f64::from(root.0) - f64::from(monitor.x),
                f64::from(root.1) - f64::from(monitor.y),
            ),
            time,
        },
        RawPointer::Move { root, time } => PointerInput::Move {
            local: (
                f64::from(root.0) - f64::from(monitor.x),
                f64::from(root.1) - f64::from(monitor.y),
            ),
            time,
        },
        RawPointer::Up { time } => PointerInput::Up { time },
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;

    use rpresent_config::{Config, ConfigService};
    use rpresent_spotlight::SpotlightCommand;
    use rpresent_zoom::{MonitorRect, ZoomCommand, ZoomRuntimeRequest, channel as zoom_channel};

    use super::*;
    use crate::native_input::{NativeEvent, NativeRequest, channel as native_channel};

    fn coordinator() -> (
        Coordinator,
        mpsc::Receiver<SpotlightRuntimeMessage>,
        rpresent_zoom::ZoomInbox,
        crate::native_input::NativeInbox,
    ) {
        let config = Arc::new(ConfigService::new(
            PathBuf::from("test.toml"),
            Config::default(),
        ));
        let (spotlight, spotlight_events) = mpsc::channel();
        let (zoom, zoom_events) = zoom_channel().unwrap();
        let (native, native_events) = native_channel().unwrap();
        let outputs = CoordinatorOutputs {
            spotlight,
            zoom,
            native,
            annotation: mpsc::channel().0,
        };
        (
            Coordinator::new(config, outputs),
            spotlight_events,
            zoom_events,
            native_events,
        )
    }

    fn monitor() -> MonitorRect {
        MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn activation_does_not_prepare_before_spotlight_hide_acknowledgment() {
        let (mut coordinator, spotlight_events, zoom_events, native_events) = coordinator();

        coordinator
            .handle_event(AppEvent::SpotlightRuntime(SpotlightEvent::Ready {
                window: 2,
            }))
            .unwrap();
        coordinator
            .handle_event(AppEvent::Command(AppCommand::Spotlight(
                SpotlightCommand::On,
            )))
            .unwrap();
        let SpotlightRuntimeMessage::Present {
            operation,
            visible: true,
        } = spotlight_events.recv().unwrap()
        else {
            panic!("spotlight should be presented");
        };
        coordinator
            .handle_event(AppEvent::SpotlightRuntime(SpotlightEvent::Presented {
                operation,
                visible: true,
                result: Ok(()),
            }))
            .unwrap();
        let NativeRequest::Restack { operation, .. } = native_events.try_recv().unwrap() else {
            panic!("spotlight presentation should be restacked");
        };
        coordinator
            .handle_event(AppEvent::Native(NativeEvent::Applied {
                operation,
                result: Ok(()),
            }))
            .unwrap();

        coordinator
            .handle_event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::On)))
            .unwrap();
        let ZoomRuntimeRequest::SelectMonitor { operation } = zoom_events.try_recv().unwrap()
        else {
            panic!("zoom should select a monitor first");
        };
        coordinator
            .handle_event(AppEvent::ZoomRuntime(ZoomEvent::Selected {
                operation,
                monitor: monitor(),
            }))
            .unwrap();

        let SpotlightRuntimeMessage::Present {
            operation: hide_operation,
            visible: false,
        } = spotlight_events.recv().unwrap()
        else {
            panic!("spotlight must be hidden before capture");
        };
        assert!(matches!(
            zoom_events.try_recv(),
            Err(crossbeam_channel::TryRecvError::Empty)
        ));

        coordinator
            .handle_event(AppEvent::SpotlightRuntime(SpotlightEvent::Presented {
                operation: hide_operation,
                visible: false,
                result: Ok(()),
            }))
            .unwrap();
        assert!(matches!(
            zoom_events.try_recv().unwrap(),
            ZoomRuntimeRequest::Prepare { .. }
        ));
    }
}
