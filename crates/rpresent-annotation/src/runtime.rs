use std::os::fd::RawFd;
use std::sync::Arc;
use std::time::Instant;

use rpresent_config::ConfigService;

use crate::command::AnnotationCommand;
use crate::error::OverlayError;
use crate::geometry::Rect;
use crate::render::Renderer;
use crate::session::{KeyInput, PointerInput, Session};
use crate::toolbar::DisplayGeometry;
use crate::x11_clipboard::{PasteRequest, PasteResult};

pub type AnnotationEventSink = Arc<dyn Fn(AnnotationEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationRequest {
    Command(AnnotationCommand),
    Pointer { epoch: u64, input: PointerInput },
    Key { epoch: u64, input: KeyInput },
    Ownership { epoch: Option<u64> },
    Cancel { operation: u64 },
    Present { operation: u64, visible: bool },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnotationStatus {
    pub generation: u64,
    pub window: Option<u32>,
    pub ready: bool,
    pub editing_requested: bool,
    pub has_content: bool,
    pub text_editing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationEvent {
    Status(AnnotationStatus),
    CommandRejected {
        command: String,
        error: String,
    },
    Presented {
        operation: u64,
        generation: u64,
        visible: bool,
        result: Result<(), String>,
    },
    Cancelled {
        operation: u64,
    },
    Failed {
        generation: u64,
        error: String,
        recoverable: bool,
    },
    Exited(Result<(), String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackendEvent {
    Exposed,
    GeometryChanged(DisplayGeometry),
    Paste(PasteResult),
}

/// Native presentation owned by the runtime. The trait stays private so the
/// application can only speak the request protocol.
pub(crate) trait AnnotationBackend {
    fn window_id(&self) -> u32;
    fn size(&self) -> (u32, u32);
    /// Root bounds, monitor rectangles, and a fresh pointer sample used for
    /// toolbar placement.
    fn geometry(&self) -> Result<DisplayGeometry, OverlayError>;
    fn present(&mut self, visible: bool) -> Result<(), OverlayError>;
    fn upload(&mut self, renderer: &Renderer, damage: Rect) -> Result<(), OverlayError>;
    fn connection_fd(&self) -> Option<RawFd>;
    fn poll_event(&mut self, now: Instant) -> Result<Option<BackendEvent>, OverlayError>;
    fn next_deadline(&self) -> Option<Instant>;
    fn request_paste(&mut self, request: PasteRequest) -> Result<(), OverlayError>;
    fn cancel_paste(&mut self, session: u64);
}

type BackendFactory = Box<dyn FnMut() -> Result<Box<dyn AnnotationBackend>, OverlayError>>;

pub(crate) struct AnnotationRuntime {
    session: Session,
    renderer: Option<Renderer>,
    backend: Option<Box<dyn AnnotationBackend>>,
    factory: BackendFactory,
    events: AnnotationEventSink,
    generation: u64,
    last_status: Option<AnnotationStatus>,
    visible: bool,
    force_full: bool,
    exited: bool,
    paste_session: Option<u64>,
}

impl AnnotationRuntime {
    pub(crate) fn new(
        config: Arc<ConfigService>,
        factory: BackendFactory,
        events: AnnotationEventSink,
    ) -> Result<Self, OverlayError> {
        let session = Session::new(config.annotation(), &config.bindings().annotation)
            .map_err(|error| OverlayError::Runtime(error.to_string()))?;
        let mut runtime = Self {
            session,
            renderer: None,
            backend: None,
            factory,
            events,
            generation: 0,
            last_status: None,
            visible: false,
            force_full: true,
            exited: false,
            paste_session: None,
        };
        runtime.recreate_backend();
        runtime.publish_status();
        Ok(runtime)
    }

    pub(crate) fn process(&mut self, request: AnnotationRequest, now: Instant) {
        if self.exited {
            return;
        }
        match request {
            AnnotationRequest::Command(command) => {
                if matches!(command, AnnotationCommand::On | AnnotationCommand::Toggle)
                    && self.backend.is_some()
                    && let Err(error) = self.refresh_geometry()
                {
                    self.backend_failed(error);
                }
                if matches!(command, AnnotationCommand::On) && self.backend.is_none() {
                    self.recreate_backend();
                    self.publish_status();
                }
                if let Err(error) = self.session.command(command.clone()) {
                    self.emit(AnnotationEvent::CommandRejected {
                        command: command.to_string(),
                        error,
                    });
                }
            }
            AnnotationRequest::Pointer { epoch, input } => {
                self.session.pointer(epoch, input);
            }
            AnnotationRequest::Key { epoch, input } => {
                self.session.key(epoch, input);
            }
            AnnotationRequest::Ownership { epoch } => {
                self.session.ownership(epoch);
            }
            AnnotationRequest::Cancel { operation } => {
                self.session.cancel();
                self.emit(AnnotationEvent::Cancelled { operation });
            }
            AnnotationRequest::Present { operation, visible } => {
                self.present(operation, visible);
            }
            AnnotationRequest::Shutdown => {
                self.session.cancel();
                self.backend = None;
                self.renderer = None;
                self.visible = false;
                self.exited = true;
                self.publish_status();
                self.emit(AnnotationEvent::Exited(Ok(())));
                return;
            }
        }
        let _ = now;
        self.service_pastes();
        self.publish_status();
    }

    pub(crate) fn flush(&mut self, now: Instant) {
        if self.exited {
            return;
        }
        self.service_pastes();
        self.poll_backend(now);
        let Some(renderer) = self.renderer.as_ref() else {
            self.session.take_damage();
            return;
        };
        let damage = if self.force_full {
            // A full frame supersedes any pending damage.
            self.session.take_damage();
            Some(renderer.full_bounds())
        } else {
            self.session.take_damage()
        };
        let Some(damage) = damage else {
            return;
        };
        if !self.visible {
            self.session.take_damage();
            self.force_full = true;
            return;
        }
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        self.force_full = false;
        let result = renderer
            .redraw(&mut self.session, damage)
            .and_then(|()| backend.upload(renderer, damage));
        if let Err(error) = result {
            self.backend_failed(error);
        }
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.backend
            .as_ref()
            .and_then(|backend| backend.next_deadline())
    }

    /// Sample root bounds, monitor rectangles, and a fresh pointer position
    /// from the backend into the session. Callers route errors through
    /// `backend_failed`; readiness is never published for a failed refresh.
    pub(crate) fn refresh_geometry(&mut self) -> Result<(), OverlayError> {
        let Some(backend) = self.backend.as_ref() else {
            return Err(OverlayError::Recoverable(
                "annotation backend unavailable".to_owned(),
            ));
        };
        let geometry = backend.geometry()?;
        self.session.update_display(geometry);
        Ok(())
    }

    pub(crate) fn is_exited(&self) -> bool {
        self.exited
    }

    pub(crate) fn connection_fd(&self) -> Option<RawFd> {
        self.backend
            .as_ref()
            .and_then(|backend| backend.connection_fd())
    }

    fn present(&mut self, operation: u64, visible: bool) {
        let generation = self.generation;
        let result = match self.backend.as_mut() {
            Some(backend) => backend.present(visible),
            None => Err(OverlayError::Recoverable(
                "annotation backend unavailable".to_owned(),
            )),
        };
        let reported = match &result {
            Ok(()) => Ok(()),
            Err(error) => Err(error.to_string()),
        };
        match result {
            Ok(()) => {
                self.visible = visible;
                self.force_full = true;
            }
            Err(error) => self.backend_failed(error),
        }
        self.emit(AnnotationEvent::Presented {
            operation,
            generation,
            visible,
            result: reported,
        });
    }

    fn poll_backend(&mut self, now: Instant) {
        loop {
            let Some(backend) = self.backend.as_mut() else {
                return;
            };
            let event = match backend.poll_event(now) {
                Ok(None) => return,
                Ok(Some(event)) => event,
                Err(error) => {
                    self.backend_failed(error);
                    return;
                }
            };
            match event {
                BackendEvent::Exposed => self.force_full = true,
                BackendEvent::GeometryChanged(geometry) => {
                    let root = geometry.root;
                    let resize = self.renderer.as_ref().is_none_or(|renderer| {
                        renderer.width() != root.width || renderer.height() != root.height
                    });
                    if resize {
                        match Renderer::new(root.width, root.height) {
                            Ok(renderer) => {
                                self.renderer = Some(renderer);
                                self.force_full = true;
                            }
                            Err(error) => {
                                self.backend_failed(error);
                                return;
                            }
                        }
                    }
                    // Pointer-only refreshes keep position and picker state;
                    // bounds changes cancel them inside `update_display`.
                    self.session.update_display(geometry);
                }
                BackendEvent::Paste(result) => self.session.accept_paste(result),
            }
        }
    }

    /// Cancel transfers for a draft that ended, forward queued paste requests,
    /// and keep the runtime bound to the draft that owns the current input.
    fn service_pastes(&mut self) {
        let draft = self.session.draft_id();
        if self.paste_session != draft {
            let previous = self.paste_session;
            self.paste_session = draft;
            if let (Some(session), Some(backend)) = (previous, self.backend.as_mut()) {
                backend.cancel_paste(session);
            }
        }
        let requests = self.session.take_paste_requests();
        for request in requests {
            let result = match self.backend.as_mut() {
                Some(backend) => backend.request_paste(request),
                None => Err(OverlayError::Recoverable(
                    "annotation backend unavailable".to_owned(),
                )),
            };
            if let Err(error) = result {
                self.session.accept_paste(PasteResult {
                    request,
                    text: Err(error.to_string()),
                });
            }
        }
    }

    fn recreate_backend(&mut self) {
        self.backend = None;
        self.renderer = None;
        self.visible = false;
        self.paste_session = None;
        let error = match (self.factory)() {
            Ok(backend) => {
                // Geometry is sampled before readiness is published so the
                // first placement uses the current pointer position.
                match backend.geometry() {
                    Err(error) => error,
                    Ok(geometry) => {
                        let (width, height) = backend.size();
                        match Renderer::new(width, height) {
                            Ok(renderer) => {
                                self.generation += 1;
                                self.backend = Some(backend);
                                self.renderer = Some(renderer);
                                self.force_full = true;
                                self.session.update_display(geometry);
                                return;
                            }
                            Err(error) => error,
                        }
                    }
                }
            }
            Err(error) => error,
        };
        let generation = self.generation;
        let recoverable = error.is_recoverable();
        self.emit(AnnotationEvent::Failed {
            generation,
            error: error.to_string(),
            recoverable,
        });
    }

    fn backend_failed(&mut self, error: OverlayError) {
        // Toolbar presses, drags, and pickers must not survive backend loss;
        // the committed position stays in the session.
        self.session.cancel_toolbar();
        let generation = self.generation;
        let recoverable = error.is_recoverable();
        self.backend = None;
        self.renderer = None;
        self.visible = false;
        self.paste_session = None;
        self.emit(AnnotationEvent::Failed {
            generation,
            error: error.to_string(),
            recoverable,
        });
        self.publish_status();
    }

    fn publish_status(&mut self) {
        let session = self.session.status();
        let status = AnnotationStatus {
            generation: self.generation,
            window: self.backend.as_ref().map(|backend| backend.window_id()),
            ready: self.backend.is_some(),
            editing_requested: session.editing_requested,
            has_content: session.has_content,
            text_editing: session.text_editing,
        };
        if self.last_status != Some(status) {
            self.last_status = Some(status);
            self.emit(AnnotationEvent::Status(status));
        }
    }

    fn emit(&self, event: AnnotationEvent) {
        (self.events)(event);
    }
}

#[cfg(target_os = "linux")]
pub fn run_overlay(
    config: Arc<ConfigService>,
    mut inbox: crate::transport::AnnotationInbox,
    events: AnnotationEventSink,
) -> Result<(), OverlayError> {
    let factory: BackendFactory = Box::new(|| {
        Ok(Box::new(crate::x11::X11AnnotationBackend::new()?) as Box<dyn AnnotationBackend>)
    });
    let mut runtime = AnnotationRuntime::new(config, factory, Arc::clone(&events))?;
    loop {
        let _ = inbox.drain_wake();
        loop {
            match inbox.try_recv() {
                Ok(request) => runtime.process(request, Instant::now()),
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    runtime.process(AnnotationRequest::Shutdown, Instant::now());
                    break;
                }
            }
        }
        runtime.flush(Instant::now());
        if runtime.is_exited() {
            return Ok(());
        }
        wait_for_events(&inbox, &runtime);
    }
}

#[cfg(target_os = "linux")]
fn wait_for_events(inbox: &crate::transport::AnnotationInbox, runtime: &AnnotationRuntime) {
    let mut descriptors = [
        libc::pollfd {
            fd: inbox.wake_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: runtime.connection_fd().unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let count = if descriptors[1].fd >= 0 { 2 } else { 1 };
    let timeout = runtime
        .next_deadline()
        .map(|deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32
        })
        .unwrap_or(-1);
    // SAFETY: the descriptor array is valid for the duration of the call.
    unsafe {
        libc::poll(descriptors.as_mut_ptr(), count, timeout);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn run_overlay(
    _config: Arc<ConfigService>,
    _inbox: crate::transport::AnnotationInbox,
    events: AnnotationEventSink,
) -> Result<(), OverlayError> {
    let error = OverlayError::UnsupportedPlatform;
    events(AnnotationEvent::Exited(Err(error.to_string())));
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use rpresent_config::ConfigService;
    use rpresent_config::{AnnotationConfig, Config};

    use crate::document::Point;

    struct Log {
        created: usize,
        presents: Vec<bool>,
        uploads: Vec<Rect>,
        fail_upload: bool,
        pastes: Vec<PasteRequest>,
        cancelled_pastes: Vec<u64>,
        auto_paste: bool,
        events: VecDeque<BackendEvent>,
        geometry: DisplayGeometry,
    }

    impl Default for Log {
        fn default() -> Self {
            Self {
                created: 0,
                presents: Vec::new(),
                uploads: Vec::new(),
                fail_upload: false,
                pastes: Vec::new(),
                cancelled_pastes: Vec::new(),
                auto_paste: false,
                events: VecDeque::new(),
                geometry: DisplayGeometry {
                    root: Rect {
                        x: 0,
                        y: 0,
                        width: 1280,
                        height: 720,
                    },
                    monitors: Vec::new(),
                    pointer: Point { x: 640, y: 360 },
                },
            }
        }
    }

    struct FakeBackend {
        log: Arc<Mutex<Log>>,
        fail_upload: bool,
    }

    impl AnnotationBackend for FakeBackend {
        fn window_id(&self) -> u32 {
            3
        }

        fn size(&self) -> (u32, u32) {
            let root = self.log.lock().unwrap().geometry.root;
            (root.width, root.height)
        }

        fn geometry(&self) -> Result<DisplayGeometry, OverlayError> {
            Ok(self.log.lock().unwrap().geometry.clone())
        }

        fn present(&mut self, visible: bool) -> Result<(), OverlayError> {
            self.log.lock().unwrap().presents.push(visible);
            Ok(())
        }

        fn upload(&mut self, _renderer: &Renderer, damage: Rect) -> Result<(), OverlayError> {
            if self.fail_upload {
                return Err(OverlayError::Recoverable("upload failed".to_owned()));
            }
            self.log.lock().unwrap().uploads.push(damage);
            Ok(())
        }

        fn connection_fd(&self) -> Option<RawFd> {
            None
        }

        fn poll_event(&mut self, _now: Instant) -> Result<Option<BackendEvent>, OverlayError> {
            Ok(self.log.lock().unwrap().events.pop_front())
        }

        fn next_deadline(&self) -> Option<Instant> {
            None
        }

        fn request_paste(&mut self, request: PasteRequest) -> Result<(), OverlayError> {
            let mut log = self.log.lock().unwrap();
            log.pastes.push(request);
            let auto = log.auto_paste;
            drop(log);
            if auto {
                self.log
                    .lock()
                    .unwrap()
                    .events
                    .push_back(BackendEvent::Paste(PasteResult {
                        request,
                        text: Ok("pasted".to_owned()),
                    }));
            }
            Ok(())
        }

        fn cancel_paste(&mut self, session: u64) {
            self.log.lock().unwrap().cancelled_pastes.push(session);
        }
    }

    #[allow(clippy::type_complexity)]
    fn setup(
        fail_upload: bool,
    ) -> (
        AnnotationRuntime,
        Arc<Mutex<Log>>,
        Arc<Mutex<Vec<AnnotationEvent>>>,
    ) {
        let config = Arc::new(
            ConfigService::new(PathBuf::from("test.toml"), Config::default())
                .expect("valid test configuration"),
        );
        let log = Arc::new(Mutex::new(Log {
            fail_upload,
            ..Log::default()
        }));
        let events = Arc::new(Mutex::new(Vec::new()));
        let factory_log = Arc::clone(&log);
        let factory = Box::new(move || {
            let mut guard = factory_log.lock().unwrap();
            guard.created += 1;
            let fail_upload = guard.fail_upload;
            drop(guard);
            Ok(Box::new(FakeBackend {
                log: Arc::clone(&factory_log),
                fail_upload,
            }) as Box<dyn AnnotationBackend>)
        });
        let sink_events = Arc::clone(&events);
        let sink: AnnotationEventSink =
            Arc::new(move |event| sink_events.lock().unwrap().push(event));
        let runtime = AnnotationRuntime::new(config, factory, sink).unwrap();
        (runtime, log, events)
    }

    fn now() -> Instant {
        Instant::now()
    }

    fn all_events(events: &Arc<Mutex<Vec<AnnotationEvent>>>) -> Vec<AnnotationEvent> {
        events.lock().unwrap().clone()
    }

    fn draw_dot(runtime: &mut AnnotationRuntime) {
        runtime.process(
            AnnotationRequest::Command(AnnotationCommand::Tool(
                rpresent_config::AnnotationTool::Pen,
            )),
            now(),
        );
        runtime.process(AnnotationRequest::Ownership { epoch: Some(1) }, now());
        runtime.process(
            AnnotationRequest::Pointer {
                epoch: 1,
                input: PointerInput::Down {
                    root: Point { x: 10, y: 10 },
                    time: std::time::Duration::ZERO,
                    modifiers: crate::Modifiers::default(),
                },
            },
            now(),
        );
        runtime.process(
            AnnotationRequest::Pointer {
                epoch: 1,
                input: PointerInput::Up {
                    root: Point { x: 10, y: 10 },
                    time: std::time::Duration::from_millis(10),
                    modifiers: crate::Modifiers::default(),
                },
            },
            now(),
        );
        runtime.flush(now());
    }

    fn pointer(runtime: &mut AnnotationRuntime, epoch: u64, root: Point, millis: u64) {
        runtime.process(
            AnnotationRequest::Pointer {
                epoch,
                input: PointerInput::Down {
                    root,
                    time: std::time::Duration::from_millis(millis),
                    modifiers: crate::Modifiers::default(),
                },
            },
            now(),
        );
        runtime.process(
            AnnotationRequest::Pointer {
                epoch,
                input: PointerInput::Up {
                    root,
                    time: std::time::Duration::from_millis(millis + 10),
                    modifiers: crate::Modifiers::default(),
                },
            },
            now(),
        );
    }

    fn pointer_event(runtime: &mut AnnotationRuntime, epoch: u64, input: PointerInput) {
        runtime.process(AnnotationRequest::Pointer { epoch, input }, now());
    }

    fn toolbar_center(runtime: &AnnotationRuntime, target: crate::toolbar::Target) -> Point {
        let layout = runtime.session.toolbar_layout().unwrap();
        let bounds = layout
            .controls
            .iter()
            .find(|control| control.target == target)
            .unwrap_or_else(|| panic!("missing control {target:?}"))
            .bounds;
        Point {
            x: bounds.x + bounds.width as i32 / 2,
            y: bounds.y + bounds.height as i32 / 2,
        }
    }

    /// Present the overlay for editing with ownership on `epoch`.
    fn show_editing(runtime: &mut AnnotationRuntime) {
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        runtime.process(
            AnnotationRequest::Present {
                operation: 1,
                visible: true,
            },
            now(),
        );
        runtime.process(AnnotationRequest::Ownership { epoch: Some(1) }, now());
        runtime.flush(now());
    }

    #[test]
    fn initial_status_reports_a_ready_hidden_backend() {
        let (_runtime, log, events) = setup(false);
        assert_eq!(log.lock().unwrap().created, 1);
        assert_eq!(
            all_events(&events).first(),
            Some(&AnnotationEvent::Status(AnnotationStatus {
                generation: 1,
                window: Some(3),
                ready: true,
                editing_requested: false,
                has_content: false,
                text_editing: false,
            }))
        );
    }

    #[test]
    fn present_acknowledges_with_the_backend_generation() {
        let (mut runtime, log, events) = setup(false);
        runtime.process(
            AnnotationRequest::Present {
                operation: 7,
                visible: true,
            },
            now(),
        );
        let all = all_events(&events);
        assert!(all.contains(&AnnotationEvent::Presented {
            operation: 7,
            generation: 1,
            visible: true,
            result: Ok(()),
        }));
        assert_eq!(log.lock().unwrap().presents, vec![true]);
    }

    #[test]
    fn drawing_uploads_only_the_damaged_region() {
        let (mut runtime, log, events) = setup(false);
        runtime.process(
            AnnotationRequest::Present {
                operation: 1,
                visible: true,
            },
            now(),
        );
        runtime.flush(now());
        log.lock().unwrap().uploads.clear();
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        draw_dot(&mut runtime);
        let log = log.lock().unwrap();
        assert_eq!(log.uploads.len(), 1);
        let upload = log.uploads[0];
        assert!(upload.width < 100 || upload.height < 100);
        drop(log);
        assert!(all_events(&events).iter().any(|event| matches!(
            event,
            AnnotationEvent::Status(status) if status.has_content
        )));
    }

    #[test]
    fn keyboard_document_history_repaints_before_leaving_editing() {
        let (mut runtime, log, _) = setup(false);
        show_editing(&mut runtime);
        draw_dot(&mut runtime);
        let renderer = runtime.renderer.as_ref().unwrap();
        let alpha = ((10 * renderer.width() + 10) * 4 + 3) as usize;
        assert!(renderer.pixels()[alpha] > 0);

        for (expression, visible) in [
            ("Ctrl+z", false),
            ("Ctrl+Z", true),
            ("Ctrl+z", false),
            ("Ctrl+Z", true),
        ] {
            log.lock().unwrap().uploads.clear();
            let shortcut = expression
                .parse::<rpresent_config::shortcuts::KeyInput>()
                .unwrap();
            runtime.process(
                AnnotationRequest::Key {
                    epoch: 1,
                    input: KeyInput {
                        key: shortcut.key(),
                        text: None,
                        modifiers: crate::Modifiers {
                            ctrl: true,
                            shift: shortcut
                                .mods()
                                .contains(rpresent_config::shortcuts::Mods::SHIFT),
                            ..crate::Modifiers::default()
                        },
                        shortcut: Some(shortcut),
                    },
                },
                now(),
            );
            runtime.flush(now());

            assert_eq!(
                runtime.session.document().objects().len(),
                usize::from(visible)
            );
            assert_eq!(
                runtime.renderer.as_ref().unwrap().pixels()[alpha] > 0,
                visible
            );
            assert!(
                log.lock()
                    .unwrap()
                    .uploads
                    .iter()
                    .any(|rect| { rect.contains(Point { x: 10, y: 10 }) })
            );
            assert!(runtime.session.status().editing_requested);
        }
    }

    #[test]
    fn keyboard_draft_history_repaints_before_leaving_editing() {
        let (mut runtime, log, _) = setup(false);
        show_editing(&mut runtime);
        runtime.process(
            AnnotationRequest::Command(AnnotationCommand::Tool(
                rpresent_config::AnnotationTool::Text,
            )),
            now(),
        );
        pointer(&mut runtime, 1, Point { x: 10, y: 10 }, 0);

        for (letter, ctrl, shift, expected) in [
            ('a', false, false, "a"),
            ('b', false, false, "ab"),
            ('z', true, false, "a"),
            ('y', true, false, "ab"),
            ('z', true, false, "a"),
            ('Z', true, true, "ab"),
        ] {
            log.lock().unwrap().uploads.clear();
            runtime.process(
                AnnotationRequest::Key {
                    epoch: 1,
                    input: KeyInput {
                        key: rpresent_config::shortcuts::Key::Char(letter),
                        text: Some(letter),
                        modifiers: crate::Modifiers {
                            ctrl,
                            shift,
                            ..crate::Modifiers::default()
                        },
                        shortcut: None,
                    },
                },
                now(),
            );
            runtime.flush(now());

            assert_eq!(
                runtime
                    .session
                    .text_edit
                    .as_ref()
                    .unwrap()
                    .draft
                    .plain_text(),
                expected
            );
            let renderer = runtime.renderer.as_ref().unwrap();
            let mut reference = Renderer::new(renderer.width(), renderer.height()).unwrap();
            reference
                .redraw(&mut runtime.session, reference.full_bounds())
                .unwrap();
            assert!(
                renderer.pixels() == reference.pixels(),
                "stale draft after {letter}"
            );
            assert!(!log.lock().unwrap().uploads.is_empty());
            assert!(runtime.session.status().editing_requested);
        }
    }

    #[test]
    fn cancel_acknowledges_the_operation() {
        let (mut runtime, _, events) = setup(false);
        runtime.process(AnnotationRequest::Cancel { operation: 9 }, now());
        assert!(all_events(&events).contains(&AnnotationEvent::Cancelled { operation: 9 }));
    }

    #[test]
    fn invalid_commands_are_rejected_without_dropping_the_backend() {
        let (mut runtime, _, events) = setup(false);
        runtime.process(
            AnnotationRequest::Command(AnnotationCommand::Width(crate::Preset::Value(7))),
            now(),
        );
        let all = all_events(&events);
        assert!(matches!(
            all.last(),
            Some(AnnotationEvent::CommandRejected { command, .. })
                if command == "annotation=width=7"
        ));
        let status = all
            .iter()
            .rev()
            .cloned()
            .find_map(|event| match event {
                AnnotationEvent::Status(status) => Some(status),
                _ => None,
            })
            .unwrap();
        assert!(status.ready);
        assert_eq!(status.generation, 1);
    }

    #[test]
    fn failed_upload_drops_native_resources_and_on_retries() {
        let (mut runtime, log, events) = setup(true);
        runtime.process(
            AnnotationRequest::Present {
                operation: 1,
                visible: true,
            },
            now(),
        );
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        draw_dot(&mut runtime);

        let all = all_events(&events);
        assert!(all.iter().any(|event| matches!(
            event,
            AnnotationEvent::Failed {
                recoverable: true,
                ..
            }
        )));
        let failed_status = all
            .iter()
            .rev()
            .cloned()
            .find_map(|event| match event {
                AnnotationEvent::Status(status) => Some(status),
                _ => None,
            })
            .unwrap();
        assert!(!failed_status.ready);
        assert_eq!(failed_status.window, None);
        assert!(failed_status.has_content);

        log.lock().unwrap().fail_upload = false;
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        let all = all_events(&events);
        let recovered = all
            .iter()
            .rev()
            .cloned()
            .find_map(|event| match event {
                AnnotationEvent::Status(status) => Some(status),
                _ => None,
            })
            .unwrap();
        assert!(recovered.ready);
        assert_eq!(recovered.generation, 2);
        assert_eq!(log.lock().unwrap().created, 2);
    }

    #[test]
    fn shutdown_cancels_and_exits_exactly_once() {
        let (mut runtime, _, events) = setup(false);
        runtime.process(AnnotationRequest::Shutdown, now());
        runtime.process(AnnotationRequest::Shutdown, now());
        let exits = all_events(&events)
            .iter()
            .filter(|event| matches!(event, AnnotationEvent::Exited(_)))
            .count();
        assert_eq!(exits, 1);
        assert!(runtime.is_exited());
    }

    #[test]
    fn paste_requests_are_forwarded_and_results_reach_the_draft() {
        let (mut runtime, log, _) = setup(false);
        log.lock().unwrap().auto_paste = true;
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        runtime.process(
            AnnotationRequest::Command(AnnotationCommand::Tool(
                rpresent_config::AnnotationTool::Text,
            )),
            now(),
        );
        runtime.process(AnnotationRequest::Ownership { epoch: Some(2) }, now());
        runtime.process(
            AnnotationRequest::Pointer {
                epoch: 2,
                input: PointerInput::Down {
                    root: Point { x: 5, y: 5 },
                    time: std::time::Duration::ZERO,
                    modifiers: crate::Modifiers::default(),
                },
            },
            now(),
        );
        runtime.process(
            AnnotationRequest::Key {
                epoch: 2,
                input: KeyInput {
                    key: rpresent_config::shortcuts::Key::Char('v'),
                    text: Some('v'),
                    modifiers: crate::Modifiers {
                        ctrl: true,
                        ..crate::Modifiers::default()
                    },
                    shortcut: None,
                },
            },
            now(),
        );
        runtime.flush(now());
        assert_eq!(log.lock().unwrap().pastes.len(), 1);
        assert_eq!(
            runtime
                .session
                .text_edit
                .as_ref()
                .unwrap()
                .draft
                .plain_text(),
            "pasted"
        );
        let session = log.lock().unwrap().pastes[0].session;
        runtime.process(
            AnnotationRequest::Key {
                epoch: 2,
                input: KeyInput {
                    key: rpresent_config::shortcuts::Key::Esc,
                    text: None,
                    modifiers: crate::Modifiers::default(),
                    shortcut: None,
                },
            },
            now(),
        );
        runtime.flush(now());
        assert_eq!(log.lock().unwrap().cancelled_pastes, vec![session]);
    }

    #[test]
    fn toolbar_is_visible_for_empty_editing_but_not_viewing() {
        let (mut runtime, log, events) = setup(false);
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        runtime.process(
            AnnotationRequest::Present {
                operation: 10,
                visible: true,
            },
            now(),
        );
        runtime.process(AnnotationRequest::Ownership { epoch: Some(1) }, now());
        runtime.flush(now());
        assert!(runtime.session.toolbar_layout().is_some());
        assert!(runtime.session.document().objects().is_empty());
        assert!(events.lock().unwrap().iter().any(|event| matches!(
            event, AnnotationEvent::Status(status)
                if status.editing_requested && !status.has_content
        )));
        let uploads = log.lock().unwrap().uploads.len();
        runtime.flush(now());
        assert_eq!(log.lock().unwrap().uploads.len(), uploads);
        runtime.process(AnnotationRequest::Command(AnnotationCommand::Off), now());
        assert!(runtime.session.toolbar_layout().is_none());
    }

    #[test]
    fn text_editing_status_tracks_the_draft() {
        let (mut runtime, _, events) = setup(false);
        let config = AnnotationConfig::default();
        assert_eq!(config.text_size_px, 24);
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        runtime.process(
            AnnotationRequest::Command(AnnotationCommand::Tool(
                rpresent_config::AnnotationTool::Text,
            )),
            now(),
        );
        runtime.process(AnnotationRequest::Ownership { epoch: Some(2) }, now());
        runtime.process(
            AnnotationRequest::Pointer {
                epoch: 2,
                input: PointerInput::Down {
                    root: Point { x: 5, y: 5 },
                    time: std::time::Duration::ZERO,
                    modifiers: crate::Modifiers::default(),
                },
            },
            now(),
        );
        let all = all_events(&events);
        let status = all
            .iter()
            .rev()
            .cloned()
            .find_map(|event| match event {
                AnnotationEvent::Status(status) => Some(status),
                _ => None,
            })
            .unwrap();
        assert!(status.text_editing);
    }

    #[test]
    fn ownership_loss_cancels_a_pending_toolbar_press() {
        let (mut runtime, _, _) = setup(false);
        show_editing(&mut runtime);
        let bar_before = runtime.session.toolbar_layout().unwrap().bar;
        let handle = toolbar_center(&runtime, crate::toolbar::Target::Handle);
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Down {
                root: handle,
                time: std::time::Duration::ZERO,
                modifiers: crate::Modifiers::default(),
            },
        );
        runtime.process(AnnotationRequest::Ownership { epoch: None }, now());
        let far = Point { x: 100, y: 400 };
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Move {
                root: far,
                time: std::time::Duration::from_millis(20),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Up {
                root: far,
                time: std::time::Duration::from_millis(30),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert_eq!(runtime.session.toolbar_layout().unwrap().bar, bar_before);
        // Config::default() uses Arrow as the default annotation tool.
        assert_eq!(
            runtime.session.tool(),
            rpresent_config::AnnotationTool::Arrow
        );
        assert!(runtime.session.document().objects().is_empty());
    }

    #[test]
    fn capture_hides_toolbar_and_the_latest_preference_controls_restore() {
        let (mut runtime, log, _) = setup(false);
        show_editing(&mut runtime);
        assert!(runtime.session.toolbar_layout().is_some());

        // Hidden for capture; the editing preference changes while hidden.
        runtime.process(
            AnnotationRequest::Present {
                operation: 2,
                visible: false,
            },
            now(),
        );
        runtime.process(AnnotationRequest::Command(AnnotationCommand::Off), now());
        let uploads = log.lock().unwrap().uploads.len();
        for _ in 0..3 {
            runtime.flush(now());
        }
        assert_eq!(
            log.lock().unwrap().uploads.len(),
            uploads,
            "no upload while hidden"
        );
        assert!(runtime.session.toolbar_layout().is_none());

        runtime.process(
            AnnotationRequest::Present {
                operation: 3,
                visible: true,
            },
            now(),
        );
        runtime.flush(now());
        assert!(
            runtime.session.toolbar_layout().is_none(),
            "the latest preference (Off) controls visibility after restore"
        );

        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        runtime.flush(now());
        assert!(runtime.session.toolbar_layout().is_some());
    }

    #[test]
    fn upload_failure_keeps_committed_content_and_drops_toolbar_state() {
        let (mut runtime, log, events) = setup(true);
        // Enter editing without flushing: the very first upload is the one
        // that must fail with the picker open and a press latched.
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        runtime.process(
            AnnotationRequest::Present {
                operation: 1,
                visible: true,
            },
            now(),
        );
        runtime.process(AnnotationRequest::Ownership { epoch: Some(1) }, now());
        runtime.process(
            AnnotationRequest::Command(AnnotationCommand::Tool(
                rpresent_config::AnnotationTool::Pen,
            )),
            now(),
        );

        // Committed artwork outside the toolbar.
        pointer(&mut runtime, 1, Point { x: 10, y: 10 }, 0);
        assert_eq!(runtime.session.document().objects().len(), 1);

        // An open picker with a value press latched at the moment of failure.
        let at = toolbar_center(
            &runtime,
            crate::toolbar::Target::Picker(crate::toolbar::PickerKind::Color),
        );
        pointer(&mut runtime, 1, at, 100);
        assert!(runtime.session.toolbar_layout().unwrap().picker.is_some());
        let color_before = runtime.session.color();
        let value = toolbar_center(&runtime, crate::toolbar::Target::Color(color_before));
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Down {
                root: value,
                time: std::time::Duration::from_millis(150),
                modifiers: crate::Modifiers::default(),
            },
        );

        // The upload fails: native resources drop, toolbar state cancels.
        runtime.flush(now());
        assert!(all_events(&events).iter().any(|event| matches!(
            event,
            AnnotationEvent::Failed {
                recoverable: true,
                ..
            }
        )));
        assert!(runtime.session.toolbar_layout().unwrap().picker.is_none());
        assert!(runtime.session.gesture.is_none());
        assert_eq!(runtime.session.document().objects().len(), 1);

        // The coordinator revokes ownership after the failure.
        runtime.process(AnnotationRequest::Ownership { epoch: None }, now());

        log.lock().unwrap().fail_upload = false;
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        let status = all_events(&events)
            .iter()
            .rev()
            .cloned()
            .find_map(|event| match event {
                AnnotationEvent::Status(status) => Some(status),
                _ => None,
            })
            .unwrap();
        assert!(status.ready);
        assert_eq!(status.generation, 2);
        assert_eq!(runtime.session.document().objects().len(), 1);
        assert!(runtime.session.toolbar_layout().is_some());
        assert!(runtime.session.toolbar_layout().unwrap().picker.is_none());

        pointer_event(
            &mut runtime,
            1,
            PointerInput::Up {
                root: value,
                time: std::time::Duration::from_millis(210),
                modifiers: crate::Modifiers::default(),
            },
        );
        assert_eq!(
            runtime.session.color(),
            color_before,
            "a latched press must not complete after recovery"
        );
        assert_eq!(runtime.session.toolbar_layout().unwrap().picker, None);
    }

    #[test]
    fn removing_the_toolbar_monitor_moves_the_bar_into_a_remaining_monitor() {
        let (mut runtime, log, _) = setup(false);
        {
            let mut guard = log.lock().unwrap();
            guard.geometry.monitors = vec![
                Rect {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                Rect {
                    x: 640,
                    y: 0,
                    width: 640,
                    height: 480,
                },
            ];
            guard.geometry.pointer = Point { x: 320, y: 200 };
        }
        runtime.process(AnnotationRequest::Command(AnnotationCommand::On), now());
        let bar = runtime.session.toolbar_layout().unwrap().bar;
        assert!(
            bar.x >= 0 && bar.right() <= 640 && bar.y >= 0 && bar.bottom() <= 480,
            "bar starts on the first monitor: {bar:?}"
        );

        {
            let mut guard = log.lock().unwrap();
            guard.geometry.monitors = vec![Rect {
                x: 640,
                y: 0,
                width: 640,
                height: 480,
            }];
            guard.geometry.pointer = Point { x: 960, y: 200 };
            let geometry = guard.geometry.clone();
            guard
                .events
                .push_back(BackendEvent::GeometryChanged(geometry));
        }
        runtime.flush(now());
        let bar = runtime.session.toolbar_layout().unwrap().bar;
        assert!(
            bar.x >= 640 && bar.right() <= 1280 && bar.y >= 0 && bar.bottom() <= 480,
            "the entire bar must move into the remaining monitor: {bar:?}"
        );
    }

    #[test]
    fn monitor_change_without_root_resize_uploads_no_full_frame() {
        let (mut runtime, log, _) = setup(false);
        {
            let mut guard = log.lock().unwrap();
            guard.geometry.monitors = vec![
                Rect {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                },
                Rect {
                    x: 640,
                    y: 0,
                    width: 640,
                    height: 480,
                },
            ];
            guard.geometry.pointer = Point { x: 320, y: 200 };
        }
        show_editing(&mut runtime);
        log.lock().unwrap().uploads.clear();

        // Shrink the first monitor; the root dimensions stay identical.
        {
            let mut guard = log.lock().unwrap();
            guard.geometry.monitors = vec![
                Rect {
                    x: 0,
                    y: 0,
                    width: 400,
                    height: 480,
                },
                Rect {
                    x: 640,
                    y: 0,
                    width: 640,
                    height: 480,
                },
            ];
            let geometry = guard.geometry.clone();
            guard
                .events
                .push_back(BackendEvent::GeometryChanged(geometry));
        }
        runtime.flush(now());

        let uploads = log.lock().unwrap().uploads.clone();
        assert!(
            !uploads.is_empty(),
            "the toolbar must repaint against the new monitor bounds"
        );
        for upload in &uploads {
            assert!(
                !(upload.x == 0 && upload.y == 0 && upload.width == 1280 && upload.height == 720),
                "monitor changes must not reallocate the renderer for a full frame: {upload:?}"
            );
        }
    }

    #[test]
    fn pointer_only_geometry_refresh_preserves_position_and_picker() {
        let (mut runtime, log, _) = setup(false);
        show_editing(&mut runtime);

        // Complete a toolbar move.
        let handle = toolbar_center(&runtime, crate::toolbar::Target::Handle);
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Down {
                root: handle,
                time: std::time::Duration::ZERO,
                modifiers: crate::Modifiers::default(),
            },
        );
        let moved = Point { x: 400, y: 300 };
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Move {
                root: moved,
                time: std::time::Duration::from_millis(10),
                modifiers: crate::Modifiers::default(),
            },
        );
        pointer_event(
            &mut runtime,
            1,
            PointerInput::Up {
                root: moved,
                time: std::time::Duration::from_millis(20),
                modifiers: crate::Modifiers::default(),
            },
        );
        let bar_after_move = runtime.session.toolbar_layout().unwrap().bar;

        // Open a picker.
        let at = toolbar_center(
            &runtime,
            crate::toolbar::Target::Picker(crate::toolbar::PickerKind::Color),
        );
        pointer(&mut runtime, 1, at, 100);
        assert!(runtime.session.toolbar_layout().unwrap().picker.is_some());

        // Refresh only the pointer sample: bounds are unchanged.
        {
            let mut guard = log.lock().unwrap();
            guard.geometry.pointer = Point { x: 100, y: 600 };
        }
        runtime.refresh_geometry().unwrap();
        let layout = runtime.session.toolbar_layout().unwrap();
        assert_eq!(
            layout.bar, bar_after_move,
            "a pointer-only refresh must keep the remembered position"
        );
        assert!(
            layout.picker.is_some(),
            "a pointer-only refresh must keep the open picker"
        );
    }
}
