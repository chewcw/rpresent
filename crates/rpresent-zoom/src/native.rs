use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::Event;
use x11rb::protocol::randr;
use x11rb::protocol::xproto::{
    ChangeWindowAttributesAux, ConnectionExt as _, EventMask, ImageFormat, VisualClass, Window,
};
use x11rb::rust_connection::RustConnection;

use crate::OverlayError;
use crate::ZoomCommand;
use crate::capture::{CaptureError, CapturedFrame};
use crate::glx::GlxRenderer;
use crate::runtime::{ZoomEvent, ZoomEventSink, ZoomRuntimeRequest};
use crate::session::{MonitorRect, PointerInput, ZoomRequest, ZoomSession};
use crate::transport::ZoomInbox;

const FRAME_INTERVAL: Duration = Duration::from_nanos(16_666_667);
const MAX_ADVANCE: Duration = Duration::from_millis(100);

/// Run the GL-owning zoom worker. Preparation, commit, and teardown arrive as
/// explicit requests; the worker never decides which feature owns input.
pub fn run_overlay(
    config_service: Arc<rpresent_config::ConfigService>,
    mut inbox: ZoomInbox,
    events: ZoomEventSink,
) -> Result<(), OverlayError> {
    let result = run_inner(config_service, &mut inbox, Arc::clone(&events));
    match &result {
        Ok(()) => events(ZoomEvent::Exited(Ok(()))),
        Err(error) => events(ZoomEvent::Exited(Err(error.to_string()))),
    }
    result
}

fn run_inner(
    config_service: Arc<rpresent_config::ConfigService>,
    inbox: &mut ZoomInbox,
    events: ZoomEventSink,
) -> Result<(), OverlayError> {
    let (connection, screen_num) = x11rb::connect(None).map_err(runtime_error)?;
    let screen = connection
        .setup()
        .roots
        .get(screen_num)
        .ok_or_else(|| runtime_error("X11 screen disappeared during startup"))?;
    let root = screen.root;
    let root_rect = MonitorRect {
        x: 0,
        y: 0,
        width: u32::from(screen.width_in_pixels),
        height: u32::from(screen.height_in_pixels),
    };
    select_randr_notifications(&connection, root);
    let navigation = config_service.zoom().into();
    let mut session = ZoomSession::new(navigation);
    let x11_fd = connection.stream().as_raw_fd();
    let mut window = None;
    let mut selected = None;
    let mut close_requested = false;
    let mut last_frame_start = Instant::now();
    let mut deadline = None;

    // Idle runtime ready: no window, no capture, no active publication.
    events(ZoomEvent::Ready);

    loop {
        inbox.drain_wake().map_err(runtime_error)?;
        match handle_requests(
            &connection,
            screen_num,
            root,
            root_rect,
            &mut session,
            &mut window,
            &mut selected,
            inbox,
            &events,
        )? {
            LoopAction::Exit => break,
            LoopAction::Continue => {}
        }

        while let Some(event) = connection.poll_for_event().map_err(runtime_error)? {
            match event {
                Event::Expose(_) => session.mark_dirty(),
                Event::RandrNotify(_) => {
                    let monitor_lost = window.as_ref().is_some_and(|active| {
                        !query_monitors(&connection, root).contains(&active.monitor)
                    });
                    if monitor_lost && !close_requested {
                        close_requested = true;
                        events(ZoomEvent::CloseRequested);
                    }
                }
                _ => {}
            }
        }
        if window.is_none() {
            close_requested = false;
        }

        let now = Instant::now();
        if let Some(active_window) = window.as_mut() {
            let frame_due = deadline.is_some_and(|value| now >= value);
            if frame_due || session.take_dirty() || session.is_animating() {
                let elapsed = now
                    .saturating_duration_since(last_frame_start)
                    .min(MAX_ADVANCE);
                last_frame_start = now;
                if elapsed > Duration::ZERO {
                    session.advance(elapsed);
                }
                if let Some(camera) = session.camera() {
                    active_window.renderer.draw(
                        camera.scale(),
                        camera.translation(),
                        (active_window.monitor.width, active_window.monitor.height),
                    )?;
                }
                deadline = session
                    .is_animating()
                    .then_some(last_frame_start + FRAME_INTERVAL);
            }
        } else {
            deadline = None;
            last_frame_start = now;
        }

        let timeout = poll_timeout_ms(Instant::now(), deadline);
        let mut descriptors = [
            libc::pollfd {
                fd: x11_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: inbox.wake_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let result =
            unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            inbox.publish_active(false);
            return Err(runtime_error(error));
        }
        if descriptors[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            inbox.drain_wake().map_err(runtime_error)?;
        }
        if descriptors[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            inbox.publish_active(false);
            return Err(runtime_error("X11 connection closed"));
        }
    }

    close_window(&connection, &mut window)?;
    session.closed();
    inbox.publish_active(false);
    Ok(())
}

enum LoopAction {
    Continue,
    Exit,
}

#[allow(clippy::too_many_arguments)]
fn handle_requests(
    connection: &RustConnection,
    screen_num: usize,
    root: Window,
    root_rect: MonitorRect,
    session: &mut ZoomSession,
    window: &mut Option<NativeWindow>,
    selected: &mut Option<MonitorRect>,
    inbox: &mut ZoomInbox,
    events: &ZoomEventSink,
) -> Result<LoopAction, OverlayError> {
    loop {
        let request = match inbox.try_recv() {
            Ok(request) => request,
            Err(crossbeam_channel::TryRecvError::Empty) => break,
            Err(crossbeam_channel::TryRecvError::Disconnected) => return Ok(LoopAction::Exit),
        };

        match request {
            ZoomRuntimeRequest::SelectMonitor { operation } => {
                match select_monitor_request(connection, root, root_rect) {
                    Ok(monitor) => {
                        *selected = Some(monitor);
                        events(ZoomEvent::Selected { operation, monitor });
                    }
                    Err(error) => {
                        tracing::error!(?error, "zoom monitor selection failed");
                        events(ZoomEvent::Failed {
                            operation,
                            error: error.to_string(),
                        });
                    }
                }
            }
            ZoomRuntimeRequest::Prepare { operation, monitor } => {
                match prepare(connection, screen_num, root, monitor, session) {
                    Ok(native_window) => {
                        let window_id = native_window.window;
                        *window = Some(native_window);
                        events(ZoomEvent::Prepared {
                            operation,
                            window: window_id,
                        });
                    }
                    Err(error) => {
                        tracing::error!(?error, "zoom preparation failed");
                        events(ZoomEvent::Failed {
                            operation,
                            error: error.to_string(),
                        });
                    }
                }
            }
            ZoomRuntimeRequest::Commit { operation } => {
                if window.is_some() {
                    inbox.publish_active(true);
                    events(ZoomEvent::Committed { operation });
                } else {
                    events(ZoomEvent::Failed {
                        operation,
                        error: "no prepared zoom window to commit".to_owned(),
                    });
                }
            }
            ZoomRuntimeRequest::Close { operation } => {
                close_window(connection, window)?;
                session.closed();
                inbox.publish_active(false);
                events(ZoomEvent::Closed { operation });
            }
            ZoomRuntimeRequest::Reset { operation } => {
                if session.is_active() {
                    session.request(ZoomRequest::Command(ZoomCommand::Reset));
                }
                events(ZoomEvent::Reset { operation });
            }
            ZoomRuntimeRequest::Scroll {
                delta,
                root: position,
            } => {
                if session.is_active() {
                    session.request(ZoomRequest::Scroll {
                        delta,
                        root: position,
                    });
                }
            }
            ZoomRuntimeRequest::Pointer(input) => session.pointer(input),
            ZoomRuntimeRequest::CancelMotion { operation } => {
                session.pointer(PointerInput::Cancel);
                events(ZoomEvent::MotionCancelled { operation });
            }
        }
    }

    Ok(LoopAction::Continue)
}

fn select_monitor_request(
    connection: &RustConnection,
    root: Window,
    root_rect: MonitorRect,
) -> Result<MonitorRect, OverlayError> {
    let pointer = connection
        .query_pointer(root)
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;
    Ok(select_monitor(
        &query_monitors(connection, root),
        (i32::from(pointer.root_x), i32::from(pointer.root_y)),
        root_rect,
    ))
}

fn prepare(
    connection: &RustConnection,
    screen_num: usize,
    root: Window,
    monitor: MonitorRect,
    session: &mut ZoomSession,
) -> Result<NativeWindow, OverlayError> {
    let frame = capture_frame(connection, screen_num, root, monitor)?;
    let mut renderer = unsafe { GlxRenderer::new(screen_num as i32)? };
    let screen = &connection.setup().roots[screen_num];
    let window = renderer.create_window(
        root.into(),
        monitor.x,
        monitor.y,
        monitor.width,
        monitor.height,
        screen.black_pixel,
    )?;

    match attach_and_map(connection, &mut renderer, window, monitor, session, &frame) {
        Ok(()) => Ok(NativeWindow {
            window,
            monitor,
            renderer,
        }),
        Err(error) => {
            // A failure after creation must not leave an orphan mapped window,
            // and dropping the renderer releases its drawable and context.
            let _ = connection.unmap_window(window);
            let _ = connection.flush();
            session.closed();
            Err(error)
        }
    }
}

fn attach_and_map(
    connection: &RustConnection,
    renderer: &mut GlxRenderer,
    window: Window,
    monitor: MonitorRect,
    session: &mut ZoomSession,
    frame: &CapturedFrame,
) -> Result<(), OverlayError> {
    connection
        .change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::EXPOSURE),
        )
        .map_err(runtime_error)?
        .check()
        .map_err(runtime_error)?;
    connection.flush().map_err(runtime_error)?;

    renderer.attach(window)?;
    renderer.upload_frame(frame)?;
    session.activated(monitor);
    if let Some(camera) = session.camera() {
        renderer.draw(
            camera.scale(),
            camera.translation(),
            (monitor.width, monitor.height),
        )?;
    }
    renderer.map_window();
    connection.flush().map_err(runtime_error)
}

fn close_window(
    connection: &RustConnection,
    window: &mut Option<NativeWindow>,
) -> Result<(), OverlayError> {
    let Some(native_window) = window.take() else {
        return Ok(());
    };
    let _ = connection.unmap_window(native_window.window);
    connection.flush().map_err(runtime_error)?;
    drop(native_window);
    Ok(())
}

struct NativeWindow {
    window: Window,
    monitor: MonitorRect,
    renderer: GlxRenderer,
}

fn capture_frame(
    connection: &RustConnection,
    screen_num: usize,
    root: Window,
    monitor: MonitorRect,
) -> Result<CapturedFrame, OverlayError> {
    let reply = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            root,
            monitor.x as i16,
            monitor.y as i16,
            monitor.width as u16,
            monitor.height as u16,
            u32::MAX,
        )
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;
    let screen = &connection.setup().roots[screen_num];
    let visual = screen
        .allowed_depths
        .iter()
        .find(|depth| depth.depth == reply.depth)
        .and_then(|depth| {
            depth
                .visuals
                .iter()
                .find(|visual| visual.visual_id == reply.visual)
        })
        .or_else(|| {
            screen
                .allowed_depths
                .iter()
                .flat_map(|depth| depth.visuals.iter())
                .find(|visual| visual.visual_id == screen.root_visual)
        })
        .ok_or_else(|| runtime_error("root visual is not available"))?;
    if visual.class != VisualClass::TRUE_COLOR && visual.class != VisualClass::DIRECT_COLOR {
        return Err(runtime_error("unsupported non-TrueColor root visual"));
    }
    let format = connection
        .setup()
        .pixmap_formats
        .iter()
        .find(|format| format.depth == reply.depth)
        .ok_or_else(|| runtime_error("root pixmap format is not available"))?;
    let bytes_per_pixel = usize::from(format.bits_per_pixel).div_ceil(8);
    let row_stride = (monitor.width as usize)
        .checked_mul(usize::from(format.bits_per_pixel))
        .and_then(|bits| {
            bits.div_ceil(usize::from(format.scanline_pad))
                .checked_mul(usize::from(format.scanline_pad))
        })
        .map(|bits| bits / 8)
        .ok_or_else(|| runtime_error("root image stride overflow"))?;
    let expected = row_stride
        .checked_mul(monitor.height as usize)
        .ok_or_else(|| runtime_error("root image size overflow"))?;
    if bytes_per_pixel == 0 || reply.data.len() < expected {
        return Err(runtime_error("root image has an invalid row layout"));
    }
    let mut bgra = Vec::with_capacity(monitor.width as usize * monitor.height as usize * 4);
    for y in 0..monitor.height as usize {
        for x in 0..monitor.width as usize {
            let offset = y * row_stride + x * bytes_per_pixel;
            let value = read_pixel(
                &reply.data[offset..offset + bytes_per_pixel],
                connection.setup().image_byte_order,
            );
            bgra.push(extract_channel(value, visual.blue_mask));
            bgra.push(extract_channel(value, visual.green_mask));
            bgra.push(extract_channel(value, visual.red_mask));
            bgra.push(255);
        }
    }
    CapturedFrame::new(monitor.width, monitor.height, bgra).map_err(capture_error)
}

fn query_monitors(connection: &RustConnection, root: Window) -> Vec<MonitorRect> {
    use randr::ConnectionExt as _;

    let has_randr = connection
        .extension_information(randr::X11_EXTENSION_NAME)
        .ok()
        .flatten()
        .is_some();
    if !has_randr {
        return Vec::new();
    }
    if let Ok(cookie) = connection.randr_get_monitors(root, true)
        && let Ok(reply) = cookie.reply()
    {
        let monitors = reply
            .monitors
            .into_iter()
            .filter(|monitor| monitor.width > 0 && monitor.height > 0)
            .map(|monitor| MonitorRect {
                x: i32::from(monitor.x),
                y: i32::from(monitor.y),
                width: u32::from(monitor.width),
                height: u32::from(monitor.height),
            })
            .collect::<Vec<_>>();
        if !monitors.is_empty() {
            return monitors;
        }
    }
    let Ok(resources_cookie) = connection.randr_get_screen_resources_current(root) else {
        return Vec::new();
    };
    let Ok(resources) = resources_cookie.reply() else {
        return Vec::new();
    };
    resources
        .crtcs
        .into_iter()
        .filter_map(|crtc| {
            connection
                .randr_get_crtc_info(crtc, resources.config_timestamp)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
        })
        .filter(|info| info.width > 0 && info.height > 0)
        .map(|info| MonitorRect {
            x: i32::from(info.x),
            y: i32::from(info.y),
            width: u32::from(info.width),
            height: u32::from(info.height),
        })
        .collect()
}

fn select_randr_notifications(connection: &RustConnection, root: Window) {
    use randr::{ConnectionExt as _, NotifyMask};

    let has_randr = connection
        .extension_information(randr::X11_EXTENSION_NAME)
        .ok()
        .flatten()
        .is_some();
    if !has_randr {
        return;
    }
    let mask = NotifyMask::SCREEN_CHANGE
        | NotifyMask::CRTC_CHANGE
        | NotifyMask::OUTPUT_CHANGE
        | NotifyMask::RESOURCE_CHANGE;
    match connection.randr_select_input(root, mask) {
        Ok(cookie) => {
            if let Err(error) = cookie.check() {
                tracing::debug!(?error, "could not subscribe to RandR monitor changes");
            }
        }
        Err(error) => {
            tracing::debug!(?error, "could not subscribe to RandR monitor changes");
        }
    }
}

fn read_pixel(bytes: &[u8], order: x11rb::protocol::xproto::ImageOrder) -> u32 {
    let mut value = 0_u32;
    if order == x11rb::protocol::xproto::ImageOrder::LSB_FIRST {
        for (shift, byte) in bytes.iter().enumerate() {
            value |= u32::from(*byte) << (shift * 8);
        }
    } else {
        for byte in bytes {
            value = (value << 8) | u32::from(*byte);
        }
    }
    value
}

fn extract_channel(value: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    let channel = (value & mask) >> shift;
    ((u64::from(channel) * 255 + u64::from(max) / 2) / u64::from(max)) as u8
}

fn capture_error(error: CaptureError) -> OverlayError {
    runtime_error(error)
}

fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

fn select_monitor(monitors: &[MonitorRect], pointer: (i32, i32), root: MonitorRect) -> MonitorRect {
    if let Some(monitor) = monitors.iter().copied().find(|monitor| {
        pointer.0 >= monitor.x
            && pointer.1 >= monitor.y
            && pointer.0 < monitor.x + monitor.width as i32
            && pointer.1 < monitor.y + monitor.height as i32
    }) {
        return monitor;
    }
    monitors
        .iter()
        .copied()
        .min_by_key(|monitor| {
            let dx = if pointer.0 < monitor.x {
                monitor.x - pointer.0
            } else if pointer.0 >= monitor.x + monitor.width as i32 {
                pointer.0 - (monitor.x + monitor.width as i32 - 1)
            } else {
                0
            };
            let dy = if pointer.1 < monitor.y {
                monitor.y - pointer.1
            } else if pointer.1 >= monitor.y + monitor.height as i32 {
                pointer.1 - (monitor.y + monitor.height as i32 - 1)
            } else {
                0
            };
            dx.saturating_mul(dx).saturating_add(dy.saturating_mul(dy))
        })
        .unwrap_or(root)
}

fn poll_timeout_ms(now: Instant, deadline: Option<Instant>) -> i32 {
    match deadline {
        None => -1,
        Some(deadline) => {
            let remaining = deadline.saturating_duration_since(now);
            remaining
                .as_millis()
                .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
                .min(i32::MAX as u128) as i32
        }
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod native_tests;

impl From<&rpresent_config::ZoomConfig> for crate::navigation::NavigationConfig {
    fn from(config: &rpresent_config::ZoomConfig) -> Self {
        Self {
            initial_scale: f64::from(config.initial_scale),
            min_scale: f64::from(config.min_scale),
            max_scale: f64::from(config.max_scale),
            scroll_speed: f64::from(config.scroll_speed),
            drag_friction: f64::from(config.drag_friction),
            scale_friction: f64::from(config.scale_friction),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_selects_a_monitor_with_a_negative_origin() {
        let root = MonitorRect {
            x: 0,
            y: 0,
            width: 3840,
            height: 1080,
        };
        let left = MonitorRect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let right = MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(select_monitor(&[left, right], (-100, 50), root), left);
    }

    #[test]
    fn empty_monitor_data_uses_root() {
        let root = MonitorRect {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
        };
        assert_eq!(select_monitor(&[], (0, 0), root), root);
    }

    #[test]
    fn timeout_rounds_submilliseconds_up() {
        let now = Instant::now();
        assert_eq!(poll_timeout_ms(now, None), -1);
        assert_eq!(
            poll_timeout_ms(now, Some(now + Duration::from_millis(2))),
            2
        );
    }
}
