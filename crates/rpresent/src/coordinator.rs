use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

use rpresent_annotation::{
    AnnotationEvent, AnnotationHandle, AnnotationRequest, AnnotationStatus,
    KeyInput as AnnotationKeyInput, Modifiers,
};
use rpresent_config::ConfigService;
use rpresent_config::shortcuts::{Key, Mods, ShortcutEvent, WheelEvent};
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
    pub annotation: AnnotationHandle,
}

pub struct Coordinator {
    config: Arc<ConfigService>,
    outputs: CoordinatorOutputs,
    spotlight: SpotlightController,
    input: InputCoordinator,
    lifecycle: WindowLifecycle,
    layers: LayerSnapshot,
    zoom_monitor: Option<rpresent_zoom::MonitorRect>,
    pending: HashMap<u64, Effect>,
    pending_wheels: VecDeque<(Mods, f32, (i32, i32))>,
    zoom_ready: bool,
    spotlight_ready: bool,
    spotlight_started: bool,
    native_ready: bool,
    startup_applied: bool,
    zoom_worker_failed: bool,
    annotation_ready: bool,
    annotation_editing: bool,
    annotation_generation: u64,
    annotation_started: bool,
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
        let mut coordinator = Self {
            input: InputCoordinator::new(InputBindings::from(config.bindings())),
            spotlight: SpotlightController::new(false),
            config,
            outputs,
            lifecycle: WindowLifecycle::new(),
            layers: LayerSnapshot::default(),
            zoom_monitor: None,
            pending: HashMap::new(),
            pending_wheels: VecDeque::new(),
            zoom_ready: false,
            spotlight_ready: false,
            spotlight_started: false,
            native_ready: false,
            startup_applied: false,
            zoom_worker_failed: false,
            annotation_ready: false,
            annotation_editing: false,
            annotation_generation: 0,
            annotation_started: false,
        };
        coordinator.update_input_status();
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
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            self.shutdown();
                            return Ok(());
                        }
                    }
                }
                None => match events.recv() {
                    Ok(event) => event,
                    Err(_) => {
                        self.shutdown();
                        return Ok(());
                    }
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
            AppEvent::AnnotationRuntime(event) => self.handle_annotation(event),
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
                let retry = matches!(command, rpresent_spotlight::SpotlightCommand::On)
                    && matches!(self.spotlight.state(), SpotlightState::On);
                if self.spotlight.apply(command) {
                    self.layers.spotlight.requested =
                        matches!(self.spotlight.state(), SpotlightState::On);
                    self.reconcile(Layer::Spotlight)?;
                } else if retry {
                    let effects = self.lifecycle.retry_owner(self.layers, Instant::now());
                    self.dispatch_effects(effects)?;
                }
            }
            AppCommand::Annotation(command) => {
                let retry = matches!(command, rpresent_annotation::AnnotationCommand::On);
                if matches!(command, rpresent_annotation::AnnotationCommand::On)
                    && self.annotation_ready
                {
                    // An explicit On retries an unavailable backend.
                    self.annotation_ready = self.layers.annotation.input_ready;
                }
                self.outputs
                    .annotation
                    .send(AnnotationRequest::Command(command))
                    .map_err(|error| error.to_string())?;
                if retry {
                    let effects = self.lifecycle.retry_owner(self.layers, Instant::now());
                    self.dispatch_effects(effects)?;
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
                if matches!(command, rpresent_zoom::ZoomCommand::Off)
                    || matches!(command, rpresent_zoom::ZoomCommand::Toggle)
                        && !self.layers.zoom.requested
                {
                    self.pending_wheels.clear();
                }
                let effects = self
                    .lifecycle
                    .request_zoom(command, self.layers, Instant::now());
                self.dispatch_effects(effects)?;
                self.refresh_input()?;
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
        self.prune_pending();
        self.refresh_input()?;
        self.drain_lifecycle_diagnostics();
        if self.lifecycle.native_cleanup_failed() {
            return Err("native input ownership cleanup failed".to_owned());
        }
        for effect in effects {
            self.dispatch_effect(effect)?;
        }
        Ok(())
    }

    fn accepts(&self, operation: u64, completion: Completion) -> bool {
        self.pending.contains_key(&operation)
            && self.lifecycle.accepts_completion(operation, completion)
    }

    fn prune_pending(&mut self) {
        self.pending
            .retain(|operation, _| self.lifecycle.is_pending_operation(*operation));
    }

    fn drain_lifecycle_diagnostics(&mut self) {
        for error in self.lifecycle.take_errors() {
            tracing::warn!(%error, "overlay coordination failed");
        }
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
                visible,
                ..
            } => self
                .outputs
                .annotation
                .send(AnnotationRequest::Present { operation, visible })
                .map_err(|error| error.to_string()),
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
            Effect::CancelAnnotation { .. } => self
                .outputs
                .annotation
                .send(AnnotationRequest::Cancel { operation })
                .map_err(|error| error.to_string()),
            Effect::SetOwner {
                window,
                restore_focus,
                ..
            } => {
                self.input.revoke_owner();
                let _ = self
                    .outputs
                    .annotation
                    .send(AnnotationRequest::Ownership { epoch: None });
                self.outputs
                    .native
                    .send(NativeRequest::SetOwner {
                        operation,
                        window,
                        restore_focus,
                    })
                    .map_err(|error| error.to_string())
            }
            Effect::SetSpotlightFollowing { following, .. } => {
                if !following && self.input.confirmed_target() != PointerOwner::Desktop {
                    self.lifecycle.invalidate_delivery();
                    self.input.revoke_owner();
                    let _ = self
                        .outputs
                        .annotation
                        .send(AnnotationRequest::Ownership { epoch: None });
                }
                self.outputs
                    .spotlight
                    .send(SpotlightRuntimeMessage::SetFollowing {
                        operation,
                        following,
                    })
                    .map_err(|error| error.to_string())
            }
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
            Effect::CancelZoomMotion { .. } => {
                if self.input.confirmed_target() == PointerOwner::Zoom {
                    self.input.revoke_owner();
                }
                self.outputs
                    .zoom
                    .send(ZoomRuntimeRequest::CancelMotion { operation })
                    .map_err(|error| error.to_string())
            }
        };
        if let Err(error) = send_result {
            return self.complete(operation, Err(error));
        }
        Ok(())
    }

    fn complete(
        &mut self,
        operation: u64,
        result: Result<Completion, String>,
    ) -> Result<(), String> {
        if !self.lifecycle.is_pending_operation(operation) || !self.pending.contains_key(&operation)
        {
            return Ok(());
        }
        if let Ok(completion) = &result
            && !self.lifecycle.accepts_completion(operation, *completion)
        {
            return Ok(());
        }
        self.pending.remove(&operation);
        let effects = self
            .lifecycle
            .complete(operation, result, self.layers, Instant::now());
        self.refresh_input()?;
        self.drain_lifecycle_diagnostics();
        if self.lifecycle.native_cleanup_failed() {
            return Err("native input ownership cleanup failed".to_owned());
        }
        self.dispatch_effects(effects)
    }

    fn handle_spotlight(&mut self, event: SpotlightEvent) -> Result<(), String> {
        match event {
            SpotlightEvent::Ready { window } => {
                self.lifecycle.spotlight_restarted();
                self.spotlight_started = true;
                self.spotlight_ready = true;
                self.layers.spotlight.mapped = false;
                self.layers.spotlight.window = Some(window);
                self.layers.spotlight.input_ready = true;
                self.refresh_input()?;
                self.reconcile(Layer::Spotlight)?;
                self.maybe_startup()
            }
            SpotlightEvent::Presented {
                operation,
                visible,
                result,
            } => {
                if !self.lifecycle.is_pending_operation(operation)
                    || !self.pending.contains_key(&operation)
                {
                    return Ok(());
                }
                if !matches!(
                    self.pending.get(&operation),
                    Some(Effect::Present {
                        layer: Layer::Spotlight,
                        visible: expected,
                        ..
                    }) if *expected == visible
                ) {
                    return Ok(());
                }
                if result.is_ok()
                    && !self.accepts(
                        operation,
                        Completion::Presented {
                            layer: Layer::Spotlight,
                            visible,
                        },
                    )
                {
                    return Ok(());
                }
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
                self.spotlight_started = true;
                self.spotlight_ready = false;
                self.layers.spotlight.mapped = false;
                self.layers.spotlight.window = None;
                self.layers.spotlight.input_ready = false;
                if let Err(error) = result {
                    tracing::warn!(%error, "spotlight runtime exited");
                }
                self.refresh_input()?;
                self.reconcile(Layer::Spotlight)?;
                self.maybe_startup()
            }
            SpotlightEvent::FollowingApplied {
                operation,
                following,
                result,
            } => {
                if !matches!(
                    self.pending.get(&operation),
                    Some(Effect::SetSpotlightFollowing {
                        following: expected,
                        ..
                    }) if *expected == following
                ) || !self.lifecycle.is_pending_operation(operation)
                {
                    return Ok(());
                }
                self.complete(
                    operation,
                    result.map(|()| Completion::SpotlightFollowing { following }),
                )
            }
        }
    }

    fn handle_zoom(&mut self, event: ZoomEvent) -> Result<(), String> {
        match event {
            ZoomEvent::Ready => {
                self.zoom_ready = true;
                self.refresh_input()?;
                self.maybe_startup()
            }
            ZoomEvent::Selected { operation, monitor } => {
                if !self.accepts(operation, Completion::MonitorSelected(monitor)) {
                    return Ok(());
                }
                self.zoom_monitor = Some(monitor);
                self.complete(operation, Ok(Completion::MonitorSelected(monitor)))
            }
            ZoomEvent::Prepared { operation, window } => {
                if !self.accepts(operation, Completion::ZoomPrepared { window }) {
                    return Ok(());
                }
                self.layers.zoom.window = Some(window);
                self.layers.zoom.mapped = true;
                self.complete(operation, Ok(Completion::ZoomPrepared { window }))
            }
            ZoomEvent::Committed { operation } => {
                if !self.accepts(operation, Completion::ZoomCommitted) {
                    return Ok(());
                }
                self.complete(operation, Ok(Completion::ZoomCommitted))?;
                self.flush_wheels()
            }
            ZoomEvent::Closed { operation } => {
                if !self.accepts(operation, Completion::ZoomClosed) {
                    return Ok(());
                }
                self.layers.zoom = LayerState::default();
                self.pending_wheels.clear();
                self.complete(operation, Ok(Completion::ZoomClosed))?;
                Ok(())
            }
            ZoomEvent::Reset { operation } => self.complete(operation, Ok(Completion::ZoomReset)),
            ZoomEvent::MotionCancelled { operation } => {
                self.complete(operation, Ok(Completion::MotionCancelled))
            }
            ZoomEvent::Failed { operation, error } => {
                self.pending_wheels.clear();
                self.complete(operation, Err(error))
            }
            ZoomEvent::CloseRequested => {
                self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off))
            }
            ZoomEvent::Exited(result) => {
                let requested = self.layers.zoom.requested;
                self.zoom_ready = false;
                self.zoom_worker_failed = result.is_err();
                self.layers.zoom.window = None;
                self.layers.zoom.mapped = false;
                self.refresh_input()?;
                self.pending_wheels.clear();
                if result.is_err() && requested {
                    self.layers.zoom.requested = false;
                    let effects = self.lifecycle.request_zoom(
                        rpresent_zoom::ZoomCommand::Off,
                        self.layers,
                        Instant::now(),
                    );
                    self.dispatch_effects(effects)?;
                }
                self.reconcile(Layer::Zoom)?;
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
                if result.is_ok() && !self.accepts(operation, Completion::NativeApplied) {
                    return Ok(());
                }
                let mut reconfirm = false;
                if result.is_ok() {
                    if let Some(Some(window)) = owner_window {
                        let owner = if window == self.layers.annotation.window.unwrap_or(0) {
                            PointerOwner::Annotation
                        } else if window == self.layers.spotlight.window.unwrap_or(0) {
                            PointerOwner::Spotlight
                        } else if window == self.layers.zoom.window.unwrap_or(0) {
                            PointerOwner::Zoom
                        } else {
                            PointerOwner::Desktop
                        };
                        let prepared_zoom = owner == PointerOwner::Zoom
                            && !self.lifecycle.is_zoom_active()
                            && self.layers.zoom.requested
                            && self.layers.zoom.mapped
                            && self.input.preferred_owner() == PointerOwner::Desktop;
                        let allowed = (owner == self.input.preferred_owner() || prepared_zoom)
                            && (owner != PointerOwner::Annotation
                                || self.lifecycle.annotation_input_allowed(&self.layers));
                        reconfirm = !allowed;
                        if allowed {
                            self.input.confirm_owner(owner, operation);
                            if owner == PointerOwner::Annotation {
                                self.outputs
                                    .annotation
                                    .send(AnnotationRequest::Ownership {
                                        epoch: Some(operation),
                                    })
                                    .map_err(|error| error.to_string())?;
                            }
                        }
                    } else if owner_window == Some(None) {
                        self.input.confirm_owner(PointerOwner::Desktop, operation);
                        let _ = self
                            .outputs
                            .annotation
                            .send(AnnotationRequest::Ownership { epoch: None });
                    }
                }
                if result.is_err() && owner_window.is_some() {
                    self.pending_wheels.clear();
                }
                let success = result.is_ok();
                self.complete(operation, result.map(|()| Completion::NativeApplied))?;
                if reconfirm {
                    self.lifecycle.invalidate_delivery();
                    self.reconcile(Layer::Spotlight)?;
                }
                if success && owner_window.is_some() {
                    self.flush_wheels()?;
                }
                Ok(())
            }
            NativeEvent::Pointer { epoch, input } => {
                if let Some((owner, input)) = self.input.handle_pointer(epoch, input) {
                    match owner {
                        PointerOwner::Annotation => {
                            self.outputs
                                .annotation
                                .send(AnnotationRequest::Pointer {
                                    epoch,
                                    input: annotation_pointer(input),
                                })
                                .map_err(|error| error.to_string())?;
                        }
                        PointerOwner::Spotlight => {}
                        PointerOwner::Zoom => {
                            let Some(monitor) = self.zoom_monitor else {
                                return Ok(());
                            };
                            self.outputs
                                .zoom
                                .send(ZoomRuntimeRequest::Pointer(zoom_pointer(input, monitor)))
                                .map_err(|error| error.to_string())?;
                        }
                        PointerOwner::Desktop => {}
                    }
                }
                Ok(())
            }
            NativeEvent::Shortcut(event) => match event {
                ShortcutEvent::Wheel(wheel) => self.handle_native_wheel(wheel),
                ShortcutEvent::Keyboard(keyboard) => {
                    if let Some(epoch) = keyboard.epoch
                        && self.input.confirmed_owner(epoch).is_none()
                    {
                        return Ok(());
                    }
                    if let Some(shortcut) = keyboard.shortcut
                        && let Some(command) = self.input.handle_key(shortcut)
                    {
                        self.handle_command(command)?;
                        return Ok(());
                    }
                    let epoch = keyboard.epoch.unwrap_or(0);
                    if keyboard.epoch.is_some()
                        && self.input.owns_epoch(PointerOwner::Annotation, epoch)
                    {
                        let annotation_input = AnnotationKeyInput {
                            key: keyboard.editor.key,
                            text: keyboard.editor.text,
                            modifiers: Modifiers {
                                shift: keyboard.editor.modifiers.contains(Mods::SHIFT),
                                ctrl: keyboard.editor.modifiers.contains(Mods::CTRL),
                                alt: keyboard.editor.modifiers.contains(Mods::ALT),
                                super_key: keyboard.editor.modifiers.contains(Mods::SUPER),
                            },
                            shortcut: keyboard.shortcut,
                        };
                        self.outputs
                            .annotation
                            .send(AnnotationRequest::Key {
                                epoch,
                                input: annotation_input,
                            })
                            .map_err(|error| error.to_string())?;
                    } else if keyboard.editor.key == Key::Esc
                        && (keyboard.epoch.is_none()
                            || self
                                .input
                                .confirmed_owner(epoch)
                                .is_some_and(|owner| self.input.owns_epoch(owner, epoch)))
                    {
                        self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off))?;
                    }
                    Ok(())
                }
            },
            NativeEvent::Lost {
                epoch,
                window,
                destroyed,
                reason,
            } => self.handle_owner_loss(epoch, window, destroyed, &reason),
            NativeEvent::Exited(result) => {
                self.native_ready = false;
                result.map_err(|error| format!("native input exited: {error}"))
            }
        }
    }

    fn handle_native_wheel(&mut self, wheel: WheelEvent) -> Result<(), String> {
        let target = self.input.preferred_owner();
        if let Some(epoch) = wheel.epoch {
            if !self.input.owns_epoch(target, epoch) {
                return Ok(());
            }
            return self.handle_wheel(wheel.modifiers, wheel.delta, wheel.root.0, wheel.root.1);
        }

        let replay = target == PointerOwner::Desktop
            && !self.layers.zoom.requested
            && !self.annotation_editing;
        self.outputs
            .native
            .send(NativeRequest::ResolveWheel {
                token: wheel.token,
                replay,
            })
            .map_err(|error| error.to_string())?;
        if replay || self.annotation_editing {
            return Ok(());
        }
        if self.lifecycle.is_suppressed() && self.layers.zoom.requested {
            return self.handle_wheel(wheel.modifiers, wheel.delta, wheel.root.0, wheel.root.1);
        }

        Ok(())
    }

    fn handle_owner_loss(
        &mut self,
        epoch: u64,
        window: u32,
        destroyed: bool,
        reason: &str,
    ) -> Result<(), String> {
        let Some(owner) = self.input.confirmed_owner(epoch) else {
            return Ok(());
        };
        let expected_window = match owner {
            PointerOwner::Annotation => self.layers.annotation.window,
            PointerOwner::Spotlight => self.layers.spotlight.window,
            PointerOwner::Zoom => self.layers.zoom.window,
            PointerOwner::Desktop => None,
        };
        if expected_window != Some(window) {
            return Ok(());
        }
        tracing::warn!(%reason, ?owner, "native input ownership was lost");
        self.input.revoke_owner();
        if owner == PointerOwner::Annotation {
            self.outputs
                .annotation
                .send(AnnotationRequest::Ownership { epoch: None })
                .map_err(|error| error.to_string())?;
        }
        let destroyed_layer = if destroyed {
            match owner {
                PointerOwner::Spotlight => {
                    self.layers.spotlight = LayerState::default();
                    Some(Layer::Spotlight)
                }
                PointerOwner::Annotation => {
                    self.layers.annotation = LayerState::default();
                    self.annotation_editing = false;
                    Some(Layer::Annotation)
                }
                PointerOwner::Zoom | PointerOwner::Desktop => None,
            }
        } else {
            None
        };
        self.refresh_input()?;
        let effects = self
            .lifecycle
            .owner_lost(window, self.layers, Instant::now());
        self.dispatch_effects(effects)?;
        if let Some(layer) = destroyed_layer {
            let effects = self
                .lifecycle
                .visibility_changed(layer, self.layers, Instant::now());
            self.dispatch_effects(effects)?;
        }
        if owner == PointerOwner::Zoom {
            return self.handle_command(AppCommand::Zoom(rpresent_zoom::ZoomCommand::Off));
        }
        Ok(())
    }

    fn handle_wheel(&mut self, modifiers: Mods, delta: f32, x: i32, y: i32) -> Result<(), String> {
        if !self.input.matches_wheel_modifier(modifiers) {
            return Ok(());
        }
        let target = self.input.preferred_owner();
        if target == PointerOwner::Annotation {
            return Ok(());
        }
        if self.lifecycle.is_suppressed() && self.layers.zoom.requested {
            self.pending_wheels.push_back((modifiers, delta, (x, y)));
            return Ok(());
        }
        if target == PointerOwner::Desktop || self.input.confirmed_target() != target {
            return Ok(());
        }
        if let Some(action) = self.input.handle_wheel(modifiers, delta, x, y) {
            self.dispatch_input(action)?;
        }
        Ok(())
    }

    fn dispatch_input(&mut self, action: InputAction) -> Result<(), String> {
        let target = match &action {
            InputAction::SpotlightWheel { .. } => PointerOwner::Spotlight,
            InputAction::ZoomWheel { .. } => PointerOwner::Zoom,
        };
        if self.input.preferred_owner() != target || self.input.confirmed_target() != target {
            return Ok(());
        }
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
                    root: (cursor_x, cursor_y),
                })
                .map_err(|error| error.to_string()),
        }
    }

    fn flush_wheels(&mut self) -> Result<(), String> {
        if self.lifecycle.is_suppressed() {
            return Ok(());
        }
        let target = self.input.preferred_owner();
        if target == PointerOwner::Annotation {
            self.pending_wheels.clear();
            return Ok(());
        }
        if target == PointerOwner::Desktop || self.input.confirmed_target() != target {
            return Ok(());
        }
        let queued = std::mem::take(&mut self.pending_wheels);
        for (modifiers, delta, (x, y)) in queued {
            self.handle_wheel(modifiers, delta, x, y)?;
        }
        Ok(())
    }

    fn maybe_startup(&mut self) -> Result<(), String> {
        if self.startup_applied
            || !self.spotlight_started
            || !self.zoom_ready
            || !self.native_ready
            || !self.annotation_started
        {
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

    fn handle_annotation(&mut self, event: AnnotationEvent) -> Result<(), String> {
        match event {
            AnnotationEvent::Status(status) => self.annotation_status(status),
            AnnotationEvent::CommandRejected { command, error } => {
                tracing::warn!(%command, %error, "annotation command rejected");
                Ok(())
            }
            AnnotationEvent::Presented {
                operation,
                generation,
                visible,
                result,
            } => {
                if generation != self.annotation_generation {
                    return Ok(());
                }
                if !self.lifecycle.is_pending_operation(operation)
                    || !self.pending.contains_key(&operation)
                {
                    return Ok(());
                }
                if !matches!(
                    self.pending.get(&operation),
                    Some(Effect::Present {
                        layer: Layer::Annotation,
                        visible: expected,
                        ..
                    }) if *expected == visible
                ) {
                    return Ok(());
                }
                if result.is_ok()
                    && !self.accepts(
                        operation,
                        Completion::Presented {
                            layer: Layer::Annotation,
                            visible,
                        },
                    )
                {
                    return Ok(());
                }
                let result = match result {
                    Ok(()) => {
                        self.layers.annotation.mapped = visible;
                        Ok(Completion::Presented {
                            layer: Layer::Annotation,
                            visible,
                        })
                    }
                    Err(error) => Err(error),
                };
                let effects =
                    self.lifecycle
                        .complete(operation, result, self.layers, Instant::now());
                self.dispatch_effects(effects)
            }
            AnnotationEvent::Cancelled { operation } => {
                if !self.accepts(operation, Completion::AnnotationCancelled) {
                    return Ok(());
                }
                let effects = self.lifecycle.complete(
                    operation,
                    Ok(Completion::AnnotationCancelled),
                    self.layers,
                    Instant::now(),
                );
                self.dispatch_effects(effects)
            }
            AnnotationEvent::Failed {
                generation,
                error,
                recoverable,
            } => {
                tracing::warn!(%error, recoverable, "annotation runtime failed");
                if generation == self.annotation_generation {
                    self.annotation_ready = false;
                    self.annotation_editing = false;
                    self.layers.annotation.input_ready = false;
                    self.layers.annotation.requested = false;
                    self.layers.annotation.window = None;
                    self.layers.annotation.mapped = false;
                    self.pending_wheels.clear();
                    self.refresh_input()?;
                    let effects = self.lifecycle.visibility_changed(
                        Layer::Annotation,
                        self.layers,
                        Instant::now(),
                    );
                    self.dispatch_effects(effects)?;
                }
                Ok(())
            }
            AnnotationEvent::Exited(result) => {
                self.annotation_ready = false;
                self.layers.annotation.input_ready = false;
                self.layers.annotation.window = None;
                self.layers.annotation.mapped = false;
                self.refresh_input()?;
                self.reconcile(Layer::Annotation)?;
                match result {
                    Ok(()) => tracing::debug!("annotation runtime exited"),
                    Err(error) => tracing::warn!(%error, "annotation runtime exited"),
                }
                Ok(())
            }
        }
    }

    fn annotation_status(&mut self, status: AnnotationStatus) -> Result<(), String> {
        if self.annotation_generation != status.generation {
            self.layers.annotation.mapped = false;
            self.layers.annotation.window = None;
        }
        self.annotation_generation = status.generation;
        self.annotation_ready = status.ready;
        self.annotation_editing = status.editing_requested;
        self.layers.annotation_editing = status.editing_requested;
        self.layers.annotation.window = status.window;
        self.layers.annotation.input_ready = status.ready;
        self.layers.annotation.requested = status.ready
            && crate::overlay_lifecycle::annotation_needs_presentation(
                status.editing_requested,
                status.has_content,
            );
        if !status.ready {
            self.layers.annotation.mapped = false;
            self.annotation_editing = false;
        }
        self.refresh_input()?;
        let first = !self.annotation_started;
        self.annotation_started = true;
        let effects =
            self.lifecycle
                .visibility_changed(Layer::Annotation, self.layers, Instant::now());
        self.dispatch_effects(effects)?;
        if first {
            self.maybe_startup()?;
        }
        Ok(())
    }

    fn shutdown(&mut self) {
        let _ = self.outputs.annotation.send(AnnotationRequest::Shutdown);
    }

    fn refresh_input(&mut self) -> Result<(), String> {
        let previous = self.input.preferred_owner();
        self.update_input_status();
        let preferred = self.input.preferred_owner();

        if self.lifecycle.is_zoom_active()
            && preferred != previous
            && matches!(
                preferred,
                PointerOwner::Spotlight | PointerOwner::Annotation
            )
        {
            self.outputs
                .zoom
                .send(ZoomRuntimeRequest::Pointer(PointerInput::Cancel))
                .map_err(|error| error.to_string())?;
        }
        let confirmed = self.input.confirmed_target();
        let prepared_zoom = confirmed == PointerOwner::Zoom
            && !self.lifecycle.is_zoom_active()
            && self.layers.zoom.requested
            && self.layers.zoom.mapped;
        if confirmed != PointerOwner::Desktop && confirmed != preferred && !prepared_zoom {
            self.lifecycle.invalidate_delivery();
            self.input.revoke_owner();
        }
        Ok(())
    }

    fn update_input_status(&mut self) {
        self.input.set_status(InputStatus {
            ..self.layers.input_status(
                self.zoom_ready && !self.zoom_worker_failed,
                self.lifecycle.is_zoom_active(),
            )
        });
    }
}

pub(crate) fn annotation_pointer(input: RawPointer) -> rpresent_annotation::PointerInput {
    let point = |root: (i32, i32)| rpresent_annotation::Point {
        x: root.0,
        y: root.1,
    };
    match input {
        RawPointer::Down {
            root,
            time,
            modifiers,
        } => rpresent_annotation::PointerInput::Down {
            root: point(root),
            time,
            modifiers: annotation_modifiers(modifiers),
        },
        RawPointer::Move {
            root,
            time,
            modifiers,
        } => rpresent_annotation::PointerInput::Move {
            root: point(root),
            time,
            modifiers: annotation_modifiers(modifiers),
        },
        RawPointer::Up {
            root,
            time,
            modifiers,
        } => rpresent_annotation::PointerInput::Up {
            root: point(root),
            time,
            modifiers: annotation_modifiers(modifiers),
        },
    }
}

fn annotation_modifiers(mods: Mods) -> Modifiers {
    Modifiers {
        shift: mods.contains(Mods::SHIFT),
        ctrl: mods.contains(Mods::CTRL),
        alt: mods.contains(Mods::ALT),
        super_key: mods.contains(Mods::SUPER),
    }
}

pub(crate) fn zoom_pointer(input: RawPointer, monitor: rpresent_zoom::MonitorRect) -> PointerInput {
    match input {
        RawPointer::Down { root, time, .. } => PointerInput::Down {
            local: (
                f64::from(root.0) - f64::from(monitor.x),
                f64::from(root.1) - f64::from(monitor.y),
            ),
            time,
        },
        RawPointer::Move { root, time, .. } => PointerInput::Move {
            local: (
                f64::from(root.0) - f64::from(monitor.x),
                f64::from(root.1) - f64::from(monitor.y),
            ),
            time,
        },
        RawPointer::Up { time, .. } => PointerInput::Up { time },
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

    #[test]
    fn annotation_pointer_maps_every_modifier_flag() {
        use std::time::Duration;

        let all = Mods::SHIFT | Mods::CTRL | Mods::ALT | Mods::SUPER;
        assert_eq!(
            annotation_pointer(RawPointer::Move {
                root: (10, 20),
                time: Duration::from_millis(5),
                modifiers: all,
            }),
            rpresent_annotation::PointerInput::Move {
                root: rpresent_annotation::Point { x: 10, y: 20 },
                time: Duration::from_millis(5),
                modifiers: Modifiers {
                    shift: true,
                    ctrl: true,
                    alt: true,
                    super_key: true,
                },
            }
        );
        assert_eq!(
            annotation_pointer(RawPointer::Down {
                root: (0, 0),
                time: Duration::ZERO,
                modifiers: Mods::NONE,
            }),
            rpresent_annotation::PointerInput::Down {
                root: rpresent_annotation::Point { x: 0, y: 0 },
                time: Duration::ZERO,
                modifiers: Modifiers::default(),
            }
        );
    }

    fn coordinator() -> (
        Coordinator,
        mpsc::Receiver<SpotlightRuntimeMessage>,
        rpresent_zoom::ZoomInbox,
        crate::native_input::NativeInbox,
        rpresent_annotation::AnnotationInbox,
    ) {
        let config =
            Arc::new(ConfigService::new(PathBuf::from("test.toml"), Config::default()).unwrap());
        let (spotlight, spotlight_events) = mpsc::channel();
        let (zoom, zoom_events) = zoom_channel().unwrap();
        let (native, native_events) = native_channel().unwrap();
        let (annotation, annotation_inbox) = rpresent_annotation::channel().unwrap();
        let outputs = CoordinatorOutputs {
            spotlight,
            zoom,
            native,
            annotation,
        };
        (
            Coordinator::new(config, outputs),
            spotlight_events,
            zoom_events,
            native_events,
            annotation_inbox,
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

    pub(super) struct Rig {
        pub(super) app: Coordinator,
        spot: mpsc::Receiver<SpotlightRuntimeMessage>,
        zoom: rpresent_zoom::ZoomInbox,
        native: crate::native_input::NativeInbox,
        annotation: rpresent_annotation::AnnotationInbox,
        pub(super) zoom_inputs: Vec<ZoomRuntimeRequest>,
        pub(super) annotation_inputs: Vec<AnnotationRequest>,
        resizes: Vec<f32>,
        resolutions: Vec<(u32, bool)>,
        owners: Vec<(u64, Option<u32>)>,
        pause_owner: bool,
        held_owner: Option<NativeRequest>,
        pause_following: bool,
        held_following: Option<SpotlightRuntimeMessage>,
        following_requests: Vec<(u64, bool)>,
    }

    impl Rig {
        pub(super) fn new() -> Self {
            let (mut app, spot, zoom, native, annotation) = coordinator();
            app.spotlight_ready = true;
            app.spotlight_started = true;
            app.zoom_ready = true;
            app.native_ready = true;
            app.annotation_started = true;
            app.startup_applied = true;
            app.annotation_generation = 1;
            app.layers.spotlight.window = Some(2);
            app.layers.spotlight.input_ready = true;
            app.layers.annotation.window = Some(3);
            app.layers.annotation.input_ready = true;
            app.update_input_status();
            Self {
                app,
                spot,
                zoom,
                native,
                annotation,
                zoom_inputs: Vec::new(),
                annotation_inputs: Vec::new(),
                resizes: Vec::new(),
                resolutions: Vec::new(),
                owners: Vec::new(),
                pause_owner: false,
                held_owner: None,
                pause_following: false,
                held_following: None,
                following_requests: Vec::new(),
            }
        }

        pub(super) fn event(&mut self, event: AppEvent) {
            self.app.handle_event(event).unwrap();
        }

        fn command(&mut self, command: AppCommand) {
            self.event(AppEvent::Command(command));
            self.settle();
        }

        fn settle(&mut self) {
            for _ in 0..128 {
                if !self.step() {
                    return;
                }
            }
            panic!("coordinator requests did not settle");
        }

        pub(super) fn step(&mut self) -> bool {
            if let Ok(request) = self.spot.try_recv() {
                match request {
                    SpotlightRuntimeMessage::Present { operation, visible } => {
                        self.event(AppEvent::SpotlightRuntime(SpotlightEvent::Presented {
                            operation,
                            visible,
                            result: Ok(()),
                        }));
                    }
                    SpotlightRuntimeMessage::SetFollowing {
                        operation,
                        following,
                    } => {
                        self.following_requests.push((operation, following));
                        if self.pause_following {
                            self.held_following = Some(request);
                            return false;
                        }
                        self.event(AppEvent::SpotlightRuntime(
                            SpotlightEvent::FollowingApplied {
                                operation,
                                following,
                                result: Ok(()),
                            },
                        ));
                    }
                    SpotlightRuntimeMessage::ResizeBy(delta) => self.resizes.push(delta),
                }
                return true;
            }
            if let Ok(request) = self.zoom.try_recv() {
                let event = match request {
                    ZoomRuntimeRequest::SelectMonitor { operation } => Some(ZoomEvent::Selected {
                        operation,
                        monitor: monitor(),
                    }),
                    ZoomRuntimeRequest::Prepare { operation, .. } => Some(ZoomEvent::Prepared {
                        operation,
                        window: 4,
                    }),
                    ZoomRuntimeRequest::Commit { operation } => {
                        Some(ZoomEvent::Committed { operation })
                    }
                    ZoomRuntimeRequest::Close { operation } => {
                        Some(ZoomEvent::Closed { operation })
                    }
                    ZoomRuntimeRequest::Reset { operation } => Some(ZoomEvent::Reset { operation }),
                    ZoomRuntimeRequest::CancelMotion { operation } => {
                        Some(ZoomEvent::MotionCancelled { operation })
                    }
                    ZoomRuntimeRequest::Pointer(_) | ZoomRuntimeRequest::Scroll { .. } => {
                        self.zoom_inputs.push(request);
                        None
                    }
                };
                if let Some(event) = event {
                    self.event(AppEvent::ZoomRuntime(event));
                }
                return true;
            }
            if let Ok(request) = self.annotation.try_recv() {
                match request {
                    AnnotationRequest::Present { operation, visible } => {
                        self.event(AppEvent::AnnotationRuntime(AnnotationEvent::Presented {
                            operation,
                            generation: 1,
                            visible,
                            result: Ok(()),
                        }));
                    }
                    AnnotationRequest::Cancel { operation } => {
                        self.event(AppEvent::AnnotationRuntime(AnnotationEvent::Cancelled {
                            operation,
                        }));
                    }
                    AnnotationRequest::Ownership { .. } => {}
                    other => self.annotation_inputs.push(other),
                }
                return true;
            }
            if let Ok(request) = self.native.try_recv() {
                let operation = match request {
                    NativeRequest::Restack { operation, .. } => operation,
                    NativeRequest::SetOwner {
                        operation, window, ..
                    } => {
                        if self.pause_owner {
                            self.held_owner = Some(request);
                            return false;
                        }
                        self.owners.push((operation, window));
                        operation
                    }
                    NativeRequest::ResolveWheel { token, replay } => {
                        self.resolutions.push((token as u32, replay));
                        return true;
                    }
                };
                self.event(AppEvent::Native(NativeEvent::Applied {
                    operation,
                    result: Ok(()),
                }));
                return true;
            }
            false
        }

        fn epoch(&self) -> u64 {
            self.owners.last().expect("a native owner was confirmed").0
        }
    }

    #[test]
    fn zoom_release_keeps_its_existing_semantics_and_ignores_root() {
        let released = zoom_pointer(
            RawPointer::Up {
                root: (900, 800),
                time: std::time::Duration::from_millis(5),
                modifiers: Mods::NONE,
            },
            monitor(),
        );
        assert_eq!(
            released,
            rpresent_zoom::PointerInput::Up {
                time: std::time::Duration::from_millis(5),
            }
        );
    }

    #[test]
    fn annotation_status_preserves_viewing_without_input() {
        let (mut coordinator, _, _, _, annotation) = coordinator();
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
                AnnotationStatus {
                    generation: 1,
                    window: Some(3),
                    ready: true,
                    editing_requested: false,
                    has_content: true,
                    text_editing: false,
                },
            )))
            .unwrap();
        assert!(coordinator.layers.annotation.requested);
        assert!(!coordinator.annotation_editing);
        assert!(!coordinator.input.owns_epoch(PointerOwner::Annotation, 1));
        assert!(matches!(
            annotation.try_recv(),
            Ok(AnnotationRequest::Present { visible: true, .. })
        ));
    }

    #[test]
    fn editing_annotation_acquires_native_owner_after_presentation() {
        let (mut coordinator, _, _, native, annotation) = coordinator();
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
                AnnotationStatus {
                    generation: 1,
                    window: Some(3),
                    ready: true,
                    editing_requested: true,
                    has_content: false,
                    text_editing: false,
                },
            )))
            .unwrap();

        let AnnotationRequest::Present { operation, .. } = annotation.try_recv().unwrap() else {
            panic!("editing annotation should be presented");
        };
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Presented {
                operation,
                generation: 1,
                visible: true,
                result: Ok(()),
            }))
            .unwrap();

        let NativeRequest::Restack { operation, .. } = native.try_recv().unwrap() else {
            panic!("annotation presentation should be restacked");
        };
        coordinator
            .handle_event(AppEvent::Native(NativeEvent::Applied {
                operation,
                result: Ok(()),
            }))
            .unwrap();

        assert!(matches!(
            native.try_recv(),
            Ok(NativeRequest::SetOwner {
                window: Some(3),
                restore_focus: false,
                ..
            })
        ));
    }

    #[test]
    fn a_stale_presentation_acknowledgment_is_ignored() {
        let (mut coordinator, _, _, _, annotation) = coordinator();
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
                AnnotationStatus {
                    generation: 1,
                    window: Some(3),
                    ready: true,
                    editing_requested: true,
                    has_content: false,
                    text_editing: false,
                },
            )))
            .unwrap();
        let mut operation = 0;
        while let Ok(request) = annotation.try_recv() {
            if let AnnotationRequest::Present { operation: id, .. } = request {
                operation = id;
            }
        }
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Presented {
                operation,
                generation: 2,
                visible: true,
                result: Ok(()),
            }))
            .unwrap();
        assert!(!coordinator.layers.annotation.mapped);
    }

    #[test]
    fn a_failed_backend_is_retried_by_an_explicit_on() {
        let (mut coordinator, _, _, _, annotation) = coordinator();
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
                AnnotationStatus {
                    generation: 1,
                    window: Some(3),
                    ready: true,
                    editing_requested: true,
                    has_content: false,
                    text_editing: false,
                },
            )))
            .unwrap();
        coordinator
            .handle_event(AppEvent::AnnotationRuntime(AnnotationEvent::Failed {
                generation: 1,
                error: "presentation failed".to_owned(),
                recoverable: true,
            }))
            .unwrap();
        assert!(!coordinator.layers.annotation.requested);
        while annotation.try_recv().is_ok() {}
        coordinator
            .handle_event(AppEvent::Command(AppCommand::Annotation(
                rpresent_annotation::AnnotationCommand::On,
            )))
            .unwrap();
        assert!(matches!(
            annotation.try_recv(),
            Ok(AnnotationRequest::Command(
                rpresent_annotation::AnnotationCommand::On
            ))
        ));
    }

    #[test]
    fn shutdown_is_forwarded_before_the_sender_is_released() {
        let (mut coordinator, _, _, _, annotation) = coordinator();
        coordinator.shutdown();
        assert_eq!(annotation.try_recv(), Ok(AnnotationRequest::Shutdown));
    }

    #[test]
    fn activation_does_not_prepare_before_spotlight_hide_acknowledgment() {
        let (mut coordinator, spotlight_events, zoom_events, native_events, _annotation) =
            coordinator();

        coordinator
            .handle_event(AppEvent::SpotlightRuntime(SpotlightEvent::Ready {
                window: 2,
            }))
            .unwrap();
        let SpotlightRuntimeMessage::SetFollowing {
            operation,
            following: false,
        } = spotlight_events.recv().unwrap()
        else {
            panic!("spotlight policy should be acknowledged before presentation");
        };
        coordinator
            .handle_event(AppEvent::SpotlightRuntime(
                SpotlightEvent::FollowingApplied {
                    operation,
                    following: false,
                    result: Ok(()),
                },
            ))
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

        let NativeRequest::SetOwner { operation, .. } = native_events.try_recv().unwrap() else {
            panic!("spotlight should acquire input after restacking");
        };
        coordinator
            .handle_event(AppEvent::Native(NativeEvent::Applied {
                operation,
                result: Ok(()),
            }))
            .unwrap();

        let SpotlightRuntimeMessage::SetFollowing {
            operation,
            following: true,
        } = spotlight_events.recv().unwrap()
        else {
            panic!("spotlight should resume following after native acquisition");
        };
        coordinator
            .handle_event(AppEvent::SpotlightRuntime(
                SpotlightEvent::FollowingApplied {
                    operation,
                    following: true,
                    result: Ok(()),
                },
            ))
            .unwrap();

        coordinator
            .handle_event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::On)))
            .unwrap();
        let SpotlightRuntimeMessage::SetFollowing {
            operation,
            following: false,
        } = spotlight_events.recv().unwrap()
        else {
            panic!("capture should suspend spotlight following before selection");
        };
        coordinator
            .handle_event(AppEvent::SpotlightRuntime(
                SpotlightEvent::FollowingApplied {
                    operation,
                    following: false,
                    result: Ok(()),
                },
            ))
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

        let NativeRequest::SetOwner {
            operation: release_operation,
            window: None,
            ..
        } = native_events.try_recv().unwrap()
        else {
            panic!("capture should release spotlight input before hiding it");
        };
        coordinator
            .handle_event(AppEvent::Native(NativeEvent::Applied {
                operation: release_operation,
                result: Ok(()),
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

    #[test]
    fn spotlight_drag_does_not_reach_lower_workers() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.zoom_inputs.clear();
        rig.annotation_inputs.clear();
        let epoch = rig.epoch();
        for input in [
            RawPointer::Down {
                root: (10, 20),
                time: std::time::Duration::ZERO,
                modifiers: Mods::NONE,
            },
            RawPointer::Move {
                root: (30, 40),
                time: std::time::Duration::from_millis(10),
                modifiers: Mods::NONE,
            },
            RawPointer::Up {
                root: (30, 40),
                time: std::time::Duration::from_millis(20),
                modifiers: Mods::NONE,
            },
        ] {
            rig.event(AppEvent::Native(NativeEvent::Pointer { epoch, input }));
        }
        rig.settle();
        assert!(rig.zoom_inputs.is_empty());
        assert!(rig.annotation_inputs.is_empty());
        assert!(rig.app.lifecycle.is_zoom_active());
    }

    #[test]
    fn spotlight_blocks_zoom_before_native_transfer_is_acknowledged() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        let old_epoch = rig.epoch();
        rig.event(AppEvent::Native(NativeEvent::Pointer {
            epoch: old_epoch,
            input: RawPointer::Down {
                root: (10, 20),
                time: std::time::Duration::ZERO,
                modifiers: Mods::NONE,
            },
        }));
        rig.settle();
        rig.zoom_inputs.clear();
        rig.pause_owner = true;
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        assert!(rig.held_owner.is_some());
        rig.event(AppEvent::Native(NativeEvent::Pointer {
            epoch: old_epoch,
            input: RawPointer::Move {
                root: (300, 400),
                time: std::time::Duration::from_millis(10),
                modifiers: Mods::NONE,
            },
        }));
        rig.settle();
        assert!(rig.zoom_inputs.iter().any(|request| {
            matches!(request, ZoomRuntimeRequest::Pointer(PointerInput::Cancel))
        }));
        assert!(!rig.zoom_inputs.iter().any(|request| {
            matches!(
                request,
                ZoomRuntimeRequest::Pointer(PointerInput::Move { .. })
            )
        }));
    }

    #[test]
    fn denied_spotlight_grab_keeps_zoom_active_and_waits_for_retry() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.pause_owner = true;
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        let NativeRequest::SetOwner { operation, .. } = rig.held_owner.take().unwrap() else {
            panic!("expected a paused owner acquisition");
        };
        rig.event(AppEvent::Native(NativeEvent::Applied {
            operation,
            result: Err("grab denied".to_owned()),
        }));
        rig.settle();
        assert!(rig.app.lifecycle.is_zoom_active());
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
        assert_eq!(rig.app.input.preferred_owner(), PointerOwner::Spotlight);
        assert!(rig.native.try_recv().is_err());
    }

    #[test]
    fn obsolete_owner_ack_cannot_restore_a_revoked_epoch() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.pause_owner = true;
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        let NativeRequest::SetOwner { operation, .. } = rig.held_owner.take().unwrap() else {
            panic!("expected owner acquisition");
        };
        rig.event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::Off)));
        rig.event(AppEvent::Native(NativeEvent::Applied {
            operation,
            result: Ok(()),
        }));
        assert_eq!(rig.app.input.confirmed_owner(operation), None);
    }

    #[test]
    fn idle_wheel_is_replayed_and_stale_overlay_wheel_is_ignored() {
        let mut rig = Rig::new();
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: None,
                token: 123,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (10, 20),
            }),
        )));
        rig.settle();
        assert_eq!(rig.resolutions, vec![(123, true)]);
        assert!(rig.resizes.is_empty());

        rig.command(AppCommand::Zoom(ZoomCommand::On));
        let stale = rig.epoch();
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.zoom_inputs.clear();
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: Some(stale),
                token: 124,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (10, 20),
            }),
        )));
        rig.settle();
        assert!(rig.resizes.is_empty());
        assert!(rig.zoom_inputs.is_empty());
    }

    #[test]
    fn confirmed_spotlight_wheel_changes_radius_only() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.zoom_inputs.clear();
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: Some(rig.epoch()),
                token: 125,
                modifiers: Mods::CTRL,
                delta: -1.0,
                root: (-100, 200),
            }),
        )));
        rig.settle();
        assert_eq!(rig.resizes, vec![-rig.app.config.spotlight().wheel_speed]);
        assert!(rig.zoom_inputs.is_empty());
    }

    #[test]
    fn stale_loss_and_escape_leave_the_current_spotlight_owner_unchanged() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        let stale = rig.epoch();
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        let current = rig.epoch();
        rig.event(AppEvent::Native(NativeEvent::Lost {
            epoch: stale,
            window: 4,
            destroyed: true,
            reason: "obsolete".to_owned(),
        }));
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Keyboard(rpresent_config::shortcuts::KeyboardEvent {
                epoch: Some(stale),
                shortcut: Some("Escape".parse().unwrap()),
                editor: rpresent_config::shortcuts::EditorInput {
                    key: Key::Esc,
                    text: None,
                    modifiers: Mods::NONE,
                },
            }),
        )));
        assert_eq!(
            rig.app.input.confirmed_owner(current),
            Some(PointerOwner::Spotlight)
        );
        assert!(rig.app.lifecycle.is_zoom_active());
    }

    #[test]
    fn configured_global_key_does_not_reach_annotation() {
        use rpresent_config::shortcuts::{
            EditorInput, Key, KeyboardEvent, ShortcutAction, ShortcutEvent,
        };
        let mut rig = Rig::new();
        rig.event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
            AnnotationStatus {
                generation: 1,
                window: Some(3),
                ready: true,
                editing_requested: true,
                has_content: true,
                text_editing: false,
            },
        )));
        rig.settle();
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Annotation);
        let epoch = rig.epoch();
        rig.annotation_inputs.clear();
        // Resolve the configured global zoom-toggle binding instead of hardcoding
        // a key expression, so the test follows the default whatever it is.
        let binding = rig
            .app
            .config
            .bindings()
            .global
            .iter()
            .find(|b| b.action == ShortcutAction::ZoomToggle)
            .expect("zoom.toggle global binding must be configured");
        let input = binding.input;
        let editor = EditorInput {
            key: input.key(),
            text: match input.key() {
                Key::Char(c) => Some(c),
                _ => None,
            },
            modifiers: input.mods(),
        };
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Keyboard(KeyboardEvent {
                epoch: Some(epoch),
                shortcut: Some(input),
                editor,
            }),
        )));
        rig.settle();
        assert!(rig.app.layers.zoom.requested);
        assert!(
            !rig.annotation_inputs
                .iter()
                .any(|request| { matches!(request, AnnotationRequest::Key { .. }) })
        );
    }

    #[test]
    fn spotlight_focus_loss_preserves_zoom_and_waits_for_retry() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.event(AppEvent::Native(NativeEvent::Lost {
            epoch: rig.epoch(),
            window: 2,
            destroyed: false,
            reason: "focus lost".to_owned(),
        }));
        rig.settle();
        assert!(rig.app.lifecycle.is_zoom_active());
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
        assert!(rig.native.try_recv().is_err());
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Spotlight);
    }

    #[test]
    fn annotation_editing_consumes_wheel_and_viewing_returns_to_spotlight() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
            AnnotationStatus {
                generation: 1,
                window: Some(3),
                ready: true,
                editing_requested: true,
                has_content: true,
                text_editing: false,
            },
        )));
        rig.settle();
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Annotation);
        rig.zoom_inputs.clear();
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: Some(rig.epoch()),
                token: 201,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (100, 200),
            }),
        )));
        rig.settle();
        assert!(rig.resizes.is_empty());
        assert!(rig.zoom_inputs.is_empty());
        rig.event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
            AnnotationStatus {
                generation: 1,
                window: Some(3),
                ready: true,
                editing_requested: false,
                has_content: true,
                text_editing: false,
            },
        )));
        rig.settle();
        assert!(rig.app.layers.annotation.mapped);
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Spotlight);
    }

    #[test]
    fn buffered_wheel_uses_spotlight_priority_after_capture_restoration() {
        let mut rig = Rig::new();
        rig.event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::On)));
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: None,
                token: 202,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (100, 200),
            }),
        )));
        assert_eq!(rig.app.pending_wheels.len(), 1);
        rig.event(AppEvent::Command(AppCommand::Spotlight(
            SpotlightCommand::On,
        )));
        rig.settle();
        assert!(rig.app.pending_wheels.is_empty());
        assert_eq!(rig.resizes, vec![rig.app.config.spotlight().wheel_speed]);
        assert!(
            !rig.zoom_inputs
                .iter()
                .any(|request| { matches!(request, ZoomRuntimeRequest::Scroll { .. }) })
        );
    }

    #[test]
    fn annotation_consumed_wheel_does_not_enter_capture_queue() {
        let mut rig = Rig::new();
        rig.event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::On)));
        rig.event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
            AnnotationStatus {
                generation: 1,
                window: Some(3),
                ready: true,
                editing_requested: true,
                has_content: false,
                text_editing: false,
            },
        )));
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: None,
                token: 203,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (100, 200),
            }),
        )));
        assert!(rig.app.pending_wheels.is_empty());
        rig.settle();
        assert!(rig.resizes.is_empty());
        assert!(
            !rig.zoom_inputs
                .iter()
                .any(|request| { matches!(request, ZoomRuntimeRequest::Scroll { .. }) })
        );
    }

    #[test]
    fn cancelling_capture_clears_buffered_wheel() {
        let mut rig = Rig::new();
        rig.event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::On)));
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: None,
                token: 204,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (100, 200),
            }),
        )));
        assert_eq!(rig.app.pending_wheels.len(), 1);
        rig.event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::Off)));
        assert!(rig.app.pending_wheels.is_empty());
        rig.settle();
        assert!(!rig.app.lifecycle.is_zoom_active());
        assert!(rig.resizes.is_empty());
    }

    #[test]
    fn failed_preparation_clears_buffered_wheel() {
        let mut rig = Rig::new();
        rig.event(AppEvent::Command(AppCommand::Zoom(ZoomCommand::On)));
        assert!(rig.step());
        let ZoomRuntimeRequest::SelectMonitor { operation } = rig.zoom.try_recv().unwrap() else {
            panic!("expected monitor selection");
        };
        rig.event(AppEvent::ZoomRuntime(ZoomEvent::Selected {
            operation,
            monitor: monitor(),
        }));
        let ZoomRuntimeRequest::Prepare { operation, .. } = rig.zoom.try_recv().unwrap() else {
            panic!("expected capture preparation");
        };
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: None,
                token: 205,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (100, 200),
            }),
        )));
        assert_eq!(rig.app.pending_wheels.len(), 1);
        rig.event(AppEvent::ZoomRuntime(ZoomEvent::Failed {
            operation,
            error: "capture failed".to_owned(),
        }));
        assert!(rig.app.pending_wheels.is_empty());
        rig.settle();
        assert!(!rig.app.lifecycle.is_zoom_active());
    }

    #[test]
    fn closing_zoom_keeps_spotlight_delivery_confirmed() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.command(AppCommand::Zoom(ZoomCommand::Off));
        let epoch = rig.epoch();
        assert!(rig.app.input.owns_epoch(PointerOwner::Spotlight, epoch));
        rig.event(AppEvent::Native(NativeEvent::Shortcut(
            ShortcutEvent::Wheel(WheelEvent {
                epoch: Some(epoch),
                token: 206,
                modifiers: Mods::CTRL,
                delta: 1.0,
                root: (100, 200),
            }),
        )));
        rig.settle();
        assert_eq!(rig.resizes.len(), 1);
    }

    #[test]
    fn prepared_zoom_owner_becomes_usable_after_commit() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        assert!(rig.app.lifecycle.is_zoom_active());
        assert!(rig.app.input.owns_epoch(PointerOwner::Zoom, rig.epoch()));
    }

    #[test]
    fn hide_keeps_spotlight_priority_until_its_acknowledgment() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.event(AppEvent::Command(AppCommand::Spotlight(
            SpotlightCommand::Off,
        )));
        assert!(rig.app.layers.spotlight.mapped);
        assert_eq!(rig.app.input.preferred_owner(), PointerOwner::Spotlight);
        rig.settle();
        assert!(!rig.app.layers.spotlight.mapped);
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Zoom);
    }

    #[test]
    fn reversed_takeover_reconfirms_zoom_in_a_new_epoch() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        let old = rig.epoch();
        rig.event(AppEvent::Command(AppCommand::Spotlight(
            SpotlightCommand::On,
        )));
        let SpotlightRuntimeMessage::Present { operation, visible } = rig.spot.try_recv().unwrap()
        else {
            panic!("expected spotlight presentation");
        };
        rig.event(AppEvent::SpotlightRuntime(SpotlightEvent::Presented {
            operation,
            visible,
            result: Ok(()),
        }));
        rig.event(AppEvent::Command(AppCommand::Spotlight(
            SpotlightCommand::Off,
        )));
        rig.settle();
        assert!(rig.app.input.owns_epoch(PointerOwner::Zoom, rig.epoch()));
        assert_ne!(rig.epoch(), old);
    }

    #[test]
    fn direct_input_actions_cannot_bypass_spotlight_priority() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.zoom_inputs.clear();
        rig.event(AppEvent::Input(InputAction::ZoomWheel {
            delta: 1.0,
            cursor_x: 100,
            cursor_y: 200,
        }));
        rig.settle();
        assert!(rig.zoom_inputs.is_empty());
    }

    fn report_editing(rig: &mut Rig, editing: bool) {
        rig.event(AppEvent::AnnotationRuntime(AnnotationEvent::Status(
            AnnotationStatus {
                generation: 1,
                window: Some(3),
                ready: true,
                editing_requested: editing,
                has_content: true,
                text_editing: false,
            },
        )));
        rig.settle();
    }

    #[test]
    fn annotation_grab_waits_for_suspension_and_native_confirmation() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.following_requests.clear();
        rig.pause_following = true;
        let prior_owners = rig.owners.len();
        report_editing(&mut rig, true);
        let SpotlightRuntimeMessage::SetFollowing {
            operation,
            following: false,
        } = rig.held_following.take().unwrap()
        else {
            panic!("expected suspension");
        };
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
        assert_eq!(rig.owners.len(), prior_owners);
        rig.pause_owner = true;
        rig.event(AppEvent::SpotlightRuntime(
            SpotlightEvent::FollowingApplied {
                operation,
                following: false,
                result: Ok(()),
            },
        ));
        rig.settle();
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
        let NativeRequest::SetOwner {
            operation,
            window: Some(3),
            ..
        } = rig.held_owner.take().unwrap()
        else {
            panic!("expected annotation acquisition");
        };
        rig.event(AppEvent::Native(NativeEvent::Applied {
            operation,
            result: Ok(()),
        }));
        rig.settle();
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Annotation);
    }

    #[test]
    fn viewing_artwork_resumes_only_after_spotlight_native_confirmation() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        report_editing(&mut rig, true);
        rig.following_requests.clear();
        rig.pause_owner = true;
        report_editing(&mut rig, false);
        assert!(rig.following_requests.is_empty());
        let NativeRequest::SetOwner {
            operation,
            window: Some(2),
            ..
        } = rig.held_owner.take().unwrap()
        else {
            panic!("expected spotlight acquisition");
        };
        rig.event(AppEvent::Native(NativeEvent::Applied {
            operation,
            result: Ok(()),
        }));
        rig.settle();
        assert_eq!(rig.following_requests.len(), 1);
        assert!(rig.following_requests[0].1);
        assert!(rig.app.layers.annotation.mapped);
    }

    #[test]
    fn rapid_editing_rejects_a_delayed_resume_acknowledgment() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        report_editing(&mut rig, true);
        rig.pause_following = true;
        report_editing(&mut rig, false);
        let SpotlightRuntimeMessage::SetFollowing {
            operation: resume,
            following: true,
        } = rig.held_following.take().unwrap()
        else {
            panic!("expected paused resume");
        };
        report_editing(&mut rig, true);
        let SpotlightRuntimeMessage::SetFollowing {
            operation: suspend,
            following: false,
        } = rig.held_following.take().unwrap()
        else {
            panic!("expected newer suspension");
        };
        rig.event(AppEvent::SpotlightRuntime(
            SpotlightEvent::FollowingApplied {
                operation: resume,
                following: true,
                result: Ok(()),
            },
        ));
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
        assert!(rig.app.lifecycle.is_pending_operation(suspend));
        rig.pause_following = false;
        rig.event(AppEvent::SpotlightRuntime(
            SpotlightEvent::FollowingApplied {
                operation: suspend,
                following: false,
                result: Ok(()),
            },
        ));
        rig.settle();
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Annotation);
    }

    #[test]
    fn spotlight_failure_does_not_stop_healthy_zoom_or_annotation_startup() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.event(AppEvent::SpotlightRuntime(SpotlightEvent::Exited(Err(
            "injected spotlight failure".into(),
        ))));
        report_editing(&mut rig, true);
        assert!(rig.app.lifecycle.is_zoom_active());
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Annotation);
        assert!(rig.app.spotlight_started);
        assert!(!rig.app.spotlight_ready);
    }

    #[test]
    fn denied_annotation_grab_keeps_following_suspended_and_zoom_healthy() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Zoom(ZoomCommand::On));
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.following_requests.clear();
        rig.pause_owner = true;
        report_editing(&mut rig, true);
        let NativeRequest::SetOwner {
            operation,
            window: Some(3),
            ..
        } = rig.held_owner.take().unwrap()
        else {
            panic!("expected annotation acquisition");
        };
        rig.event(AppEvent::Native(NativeEvent::Applied {
            operation,
            result: Err("grab denied".into()),
        }));
        rig.settle();
        assert!(rig.app.lifecycle.is_zoom_active());
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
        assert!(
            rig.following_requests
                .iter()
                .all(|(_, following)| !following)
        );
    }

    #[test]
    fn failed_spotlight_initialization_does_not_gate_startup_annotation_command() {
        let (mut app, _spot, _zoom, _native, annotation) = coordinator();
        let mut config = Config::default();
        config.annotation.enabled_on_start = true;
        app.config = Arc::new(
            ConfigService::new(PathBuf::from("test.toml"), config)
                .expect("valid test configuration"),
        );
        app.zoom_ready = true;
        app.native_ready = true;
        app.annotation_started = true;
        app.handle_event(AppEvent::SpotlightRuntime(SpotlightEvent::Exited(Err(
            "unavailable X11 spotlight backend".into(),
        ))))
        .unwrap();
        assert!(app.startup_applied);
        assert!(matches!(
            annotation.try_recv(),
            Ok(AnnotationRequest::Command(
                rpresent_annotation::AnnotationCommand::On
            ))
        ));
    }

    #[test]
    fn wrong_state_failure_cannot_complete_the_pending_suspension() {
        let mut rig = Rig::new();
        rig.command(AppCommand::Spotlight(SpotlightCommand::On));
        rig.pause_following = true;
        report_editing(&mut rig, true);
        let SpotlightRuntimeMessage::SetFollowing { operation, .. } =
            rig.held_following.take().unwrap()
        else {
            panic!("expected suspension");
        };
        rig.event(AppEvent::SpotlightRuntime(
            SpotlightEvent::FollowingApplied {
                operation,
                following: true,
                result: Err("obsolete resume failed".into()),
            },
        ));
        assert!(rig.app.lifecycle.is_pending_operation(operation));
        assert_eq!(rig.app.input.confirmed_target(), PointerOwner::Desktop);
    }

    #[test]
    fn replacement_ready_discards_an_old_following_acknowledgment() {
        for window in [2, 22] {
            let mut rig = Rig::new();
            rig.command(AppCommand::Spotlight(SpotlightCommand::On));
            rig.pause_following = true;
            report_editing(&mut rig, true);
            let SpotlightRuntimeMessage::SetFollowing { operation: old, .. } =
                rig.held_following.take().unwrap()
            else {
                panic!("expected old suspension");
            };
            rig.event(AppEvent::SpotlightRuntime(SpotlightEvent::Ready { window }));
            rig.settle();
            let SpotlightRuntimeMessage::SetFollowing {
                operation: new,
                following: false,
            } = rig.held_following.take().unwrap()
            else {
                panic!("expected replacement policy");
            };
            assert_ne!(old, new);
            rig.event(AppEvent::SpotlightRuntime(
                SpotlightEvent::FollowingApplied {
                    operation: old,
                    following: false,
                    result: Ok(()),
                },
            ));
            assert!(rig.app.lifecycle.is_pending_operation(new));
            assert!(!rig.app.layers.spotlight.mapped);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[path = "coordinator/native_tests.rs"]
mod native_tests;
