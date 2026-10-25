use std::collections::VecDeque;
use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::shape::{self, SK, SO};
use x11rb::protocol::xproto::{
    self, AtomEnum, ChangeWindowAttributesAux, ColormapAlloc, ConfigureWindowAux, ConnectionExt,
    CreateGCAux, CreateWindowAux, EventMask, ImageFormat, ImageOrder, VisualClass, Visualtype,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::error::OverlayError;
use crate::geometry::Rect;
use crate::render::Renderer;
use crate::runtime::{AnnotationBackend, BackendEvent};
use crate::toolbar::DisplayGeometry;
use crate::x11_clipboard::{
    ClipboardAction, ClipboardTarget, PasteQueue, PasteRequest, PasteResult, PropertyKind,
    PropertyRead,
};

struct ClipboardAtoms {
    selection: u32,
    utf8: u32,
    string: u32,
    incr: u32,
    utf8_property: u32,
    string_property: u32,
}

impl ClipboardAtoms {
    fn new(connection: &RustConnection) -> Result<Self, OverlayError> {
        let intern = |name: &[u8]| {
            connection
                .intern_atom(false, name)
                .map_err(lost)?
                .reply()
                .map_err(lost)
                .map(|reply| reply.atom)
        };
        Ok(Self {
            selection: intern(b"CLIPBOARD")?,
            utf8: intern(b"UTF8_STRING")?,
            string: intern(b"STRING")?,
            incr: intern(b"INCR")?,
            utf8_property: intern(ClipboardTarget::Utf8String.property().as_bytes())?,
            string_property: intern(ClipboardTarget::String.property().as_bytes())?,
        })
    }

    fn target_atom(&self, target: ClipboardTarget) -> u32 {
        match target {
            ClipboardTarget::Utf8String => self.utf8,
            ClipboardTarget::String => self.string,
        }
    }

    fn property_atom(&self, property: &str) -> u32 {
        if property == ClipboardTarget::Utf8String.property() {
            self.utf8_property
        } else {
            self.string_property
        }
    }

    fn target_for_property(property: &str) -> ClipboardTarget {
        if property == ClipboardTarget::Utf8String.property() {
            ClipboardTarget::Utf8String
        } else {
            ClipboardTarget::String
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ByteOrder {
    Bgra,
    Argb,
}

pub struct X11AnnotationBackend {
    connection: RustConnection,
    root: u32,
    window: u32,
    colormap: u32,
    gc: u32,
    width: u32,
    height: u32,
    byte_order: ByteOrder,
    atoms: ClipboardAtoms,
    clipboard: PasteQueue,
    paste_events: VecDeque<PasteResult>,
}

impl X11AnnotationBackend {
    pub fn new() -> Result<X11AnnotationBackend, OverlayError> {
        let (connection, screen_number) =
            x11rb::connect(None).map_err(|error| OverlayError::Recoverable(error.to_string()))?;
        let (root, width, height, visual_id) = {
            let screen = &connection.setup().roots[screen_number];
            let visual = find_argb_visual(screen).ok_or_else(|| {
                OverlayError::Recoverable("no 32-bit ARGB visual is available".to_owned())
            })?;
            (
                screen.root,
                screen.width_in_pixels as u32,
                screen.height_in_pixels as u32,
                visual.visual_id,
            )
        };
        let compositor_atom = format!("_NET_WM_CM_S{screen_number}");
        let atom = connection
            .intern_atom(false, compositor_atom.as_bytes())
            .map_err(lost)?
            .reply()
            .map_err(lost)?
            .atom;
        let owner = connection
            .get_selection_owner(atom)
            .map_err(lost)?
            .reply()
            .map_err(lost)?
            .owner;
        if owner == x11rb::NONE {
            return Err(OverlayError::Recoverable(
                "a compositing manager is required for transparent annotations".to_owned(),
            ));
        }

        let colormap = connection.generate_id().map_err(lost)?;
        connection
            .create_colormap(ColormapAlloc::NONE, colormap, root, visual_id)
            .map_err(lost)?;
        let window = connection.generate_id().map_err(lost)?;
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY)
            .override_redirect(1)
            .colormap(colormap);
        connection
            .create_window(
                32,
                window,
                root,
                0,
                0,
                width as u16,
                height as u16,
                0,
                WindowClass::INPUT_OUTPUT,
                visual_id,
                &aux,
            )
            .map_err(lost)?;
        // Keep the window click-through even while editing; pointer ownership is
        // provided by the application's explicit grab.
        shape::rectangles(
            &connection,
            SO::SET,
            SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )
        .map_err(lost)?;
        connection
            .change_window_attributes(
                root,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::STRUCTURE_NOTIFY),
            )
            .map_err(lost)?;
        select_randr_notifications(&connection, root);
        let gc = connection.generate_id().map_err(lost)?;
        connection
            .create_gc(gc, window, &CreateGCAux::new())
            .map_err(lost)?;
        connection.flush().map_err(lost)?;

        let byte_order = match connection.setup().image_byte_order {
            ImageOrder::LSB_FIRST => ByteOrder::Bgra,
            ImageOrder::MSB_FIRST => ByteOrder::Argb,
            other => {
                return Err(OverlayError::Recoverable(format!(
                    "unsupported X11 image byte order {other:?}"
                )));
            }
        };

        let atoms = ClipboardAtoms::new(&connection)?;

        Ok(Self {
            connection,
            root,
            window,
            colormap,
            gc,
            width,
            height,
            byte_order,
            atoms,
            clipboard: PasteQueue::new(),
            paste_events: VecDeque::new(),
        })
    }

    fn round_trip(&self) -> Result<(), OverlayError> {
        self.connection
            .get_geometry(self.window)
            .map_err(lost)?
            .reply()
            .map_err(lost)?;
        Ok(())
    }

    fn flush_clipboard(&mut self) -> Result<(), OverlayError> {
        loop {
            let actions = self.clipboard.take_actions();
            if actions.is_empty() {
                break;
            }
            for action in actions {
                match action {
                    ClipboardAction::Convert { requestor, target } => {
                        self.connection
                            .convert_selection(
                                requestor,
                                self.atoms.selection,
                                self.atoms.target_atom(target),
                                self.atoms.property_atom(target.property()),
                                x11rb::CURRENT_TIME,
                            )
                            .map_err(lost)?;
                    }
                    ClipboardAction::Read {
                        requestor,
                        property,
                        delete,
                    } => {
                        let read = self.read_clipboard_property(requestor, property, delete)?;
                        self.clipboard
                            .property_read(requestor, property, read, Instant::now());
                    }
                    ClipboardAction::Destroy { requestor } => {
                        self.connection.destroy_window(requestor).map_err(lost)?;
                    }
                }
            }
        }
        self.paste_events.extend(self.clipboard.take_results());
        self.connection.flush().map_err(lost)
    }

    fn read_clipboard_property(
        &self,
        requestor: u32,
        property: &'static str,
        delete: bool,
    ) -> Result<PropertyRead, OverlayError> {
        let atom = self.atoms.property_atom(property);
        let long_length = (self.connection.maximum_request_bytes() / 4).max(1) as u32;
        let mut bytes = Vec::new();
        let mut offset = 0u32;
        let mut type_atom = 0u32;
        let mut format = 0u8;
        loop {
            let reply = self.connection.get_property(
                false,
                requestor,
                atom,
                AtomEnum::ANY,
                offset,
                long_length,
            );
            let reply = match reply {
                Ok(cookie) => match cookie.reply() {
                    Ok(reply) => reply,
                    Err(_) => break,
                },
                Err(_) => break,
            };
            type_atom = reply.type_;
            format = reply.format;
            if format == 0 {
                break;
            }
            bytes.extend_from_slice(&reply.value);
            if reply.bytes_after == 0 || reply.value.is_empty() {
                break;
            }
            offset += (reply.value.len() / 4) as u32;
        }
        if delete {
            let _ = self
                .connection
                .get_property(true, requestor, atom, AtomEnum::ANY, 0, 0)
                .map(|cookie| cookie.reply());
        }
        let target = ClipboardAtoms::target_for_property(property);
        let kind = if type_atom == 0 || format == 0 {
            PropertyKind::None
        } else if type_atom == self.atoms.incr {
            PropertyKind::Incr
        } else if type_atom == self.atoms.target_atom(target) {
            PropertyKind::Text
        } else {
            PropertyKind::Unknown
        };
        Ok(PropertyRead {
            kind,
            format,
            bytes,
        })
    }
}

impl AnnotationBackend for X11AnnotationBackend {
    fn window_id(&self) -> u32 {
        self.window
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn geometry(&self) -> Result<DisplayGeometry, OverlayError> {
        let pointer = self
            .connection
            .query_pointer(self.root)
            .map_err(lost)?
            .reply()
            .map_err(lost)?;
        let root = Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        };
        let mut monitors = query_monitors(&self.connection, self.root);
        if monitors.is_empty() {
            monitors.push(root);
        }
        Ok(DisplayGeometry {
            root,
            monitors,
            pointer: crate::Point {
                x: i32::from(pointer.root_x),
                y: i32::from(pointer.root_y),
            },
        })
    }

    fn present(&mut self, visible: bool) -> Result<(), OverlayError> {
        if visible {
            self.connection.map_window(self.window).map_err(lost)?;
        } else {
            self.connection.unmap_window(self.window).map_err(lost)?;
        }
        self.connection.flush().map_err(lost)?;
        self.round_trip()
    }

    fn upload(&mut self, renderer: &Renderer, damage: Rect) -> Result<(), OverlayError> {
        let window_bounds = Rect {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        };
        let Some(clip) = damage.intersect(&window_bounds) else {
            return Ok(());
        };
        let Some(clip) = clip.intersect(&Rect {
            x: 0,
            y: 0,
            width: renderer.width(),
            height: renderer.height(),
        }) else {
            return Ok(());
        };
        let maximum = self.connection.maximum_request_bytes().saturating_sub(64);
        let row_bytes = clip.width as usize * 4;
        let rows_per_request = (maximum / row_bytes).max(1) as i32;
        let columns_per_request = (maximum / 4).max(1) as i32;
        let source_width = renderer.width();
        let pixels = renderer.pixels();

        let mut y = clip.y;
        while y < clip.bottom() {
            if row_bytes <= maximum {
                let rows = (clip.bottom() - y).min(rows_per_request) as u16;
                let data = pack_region(
                    pixels,
                    source_width,
                    clip.x,
                    y,
                    clip.width as u16,
                    rows,
                    self.byte_order,
                );
                self.put_image(clip.x, y, clip.width as u16, rows, &data)?;
                y += rows as i32;
            } else {
                let mut x = clip.x;
                while x < clip.right() {
                    let columns = (clip.right() - x).min(columns_per_request) as u16;
                    let data = pack_region(pixels, source_width, x, y, columns, 1, self.byte_order);
                    self.put_image(x, y, columns, 1, &data)?;
                    x += columns as i32;
                }
                y += 1;
            }
        }
        self.connection.flush().map_err(lost)?;
        Ok(())
    }

    fn connection_fd(&self) -> Option<RawFd> {
        Some(self.connection.stream().as_raw_fd())
    }

    fn poll_event(&mut self, now: Instant) -> Result<Option<BackendEvent>, OverlayError> {
        self.clipboard.tick(now);
        self.flush_clipboard()?;
        if let Some(result) = self.paste_events.pop_front() {
            return Ok(Some(BackendEvent::Paste(result)));
        }
        while let Some(event) = self.connection.poll_for_event().map_err(lost)? {
            match event {
                x11rb::protocol::Event::SelectionNotify(selection) => {
                    if selection.selection != self.atoms.selection {
                        continue;
                    }
                    let target = if selection.target == self.atoms.utf8 {
                        ClipboardTarget::Utf8String
                    } else if selection.target == self.atoms.string {
                        ClipboardTarget::String
                    } else {
                        continue;
                    };
                    let property = if selection.property == 0 {
                        None
                    } else if selection.property == self.atoms.property_atom(target.property()) {
                        Some(target.property())
                    } else {
                        continue;
                    };
                    self.clipboard
                        .selection_notify(selection.requestor, target, property, now);
                    self.flush_clipboard()?;
                }
                x11rb::protocol::Event::PropertyNotify(property)
                    if property.state == xproto::Property::NEW_VALUE
                        && (property.atom == self.atoms.utf8_property
                            || property.atom == self.atoms.string_property) =>
                {
                    self.clipboard.property_changed(property.window, now);
                    self.flush_clipboard()?;
                }
                x11rb::protocol::Event::Expose(expose) if expose.window == self.window => {
                    if expose.count == 0 {
                        return Ok(Some(BackendEvent::Exposed));
                    }
                }
                x11rb::protocol::Event::ConfigureNotify(configure)
                    if configure.window == self.window =>
                {
                    let width = configure.width as u32;
                    let height = configure.height as u32;
                    if width != self.width || height != self.height {
                        self.width = width;
                        self.height = height;
                        return Ok(Some(BackendEvent::GeometryChanged(self.geometry()?)));
                    }
                }
                x11rb::protocol::Event::ConfigureNotify(configure)
                    if configure.window == self.root =>
                {
                    let width = configure.width as u32;
                    let height = configure.height as u32;
                    if width != self.width || height != self.height {
                        self.connection
                            .configure_window(
                                self.window,
                                &ConfigureWindowAux::new()
                                    .x(0)
                                    .y(0)
                                    .width(width)
                                    .height(height),
                            )
                            .map_err(lost)?;
                        self.connection.flush().map_err(lost)?;
                        self.width = width;
                        self.height = height;
                        return Ok(Some(BackendEvent::GeometryChanged(self.geometry()?)));
                    }
                }
                // RandR topology changes matter even when the root size is
                // unchanged; the toolbar must re-place against new monitors.
                x11rb::protocol::Event::RandrNotify(_)
                | x11rb::protocol::Event::RandrScreenChangeNotify(_) => {
                    return Ok(Some(BackendEvent::GeometryChanged(self.geometry()?)));
                }
                _ => {}
            }
            if let Some(result) = self.paste_events.pop_front() {
                return Ok(Some(BackendEvent::Paste(result)));
            }
        }
        Ok(None)
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.clipboard.next_deadline()
    }

    fn request_paste(&mut self, request: PasteRequest) -> Result<(), OverlayError> {
        let requestor = self.connection.generate_id().map_err(lost)?;
        let aux = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        self.connection
            .create_window(
                0,
                requestor,
                self.root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &aux,
            )
            .map_err(lost)?;
        self.clipboard.enqueue(request, requestor, Instant::now());
        self.flush_clipboard()
    }

    fn cancel_paste(&mut self, session: u64) {
        self.clipboard.cancel_session(session, Instant::now());
        let _ = self.flush_clipboard();
    }
}

impl X11AnnotationBackend {
    fn put_image(
        &self,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
        data: &[u8],
    ) -> Result<(), OverlayError> {
        self.connection
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                width,
                height,
                x as i16,
                y as i16,
                0,
                32,
                data,
            )
            .map_err(lost)?;
        Ok(())
    }
}

impl Drop for X11AnnotationBackend {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.free_gc(self.gc);
        let _ = self.connection.free_colormap(self.colormap);
        let _ = self.connection.flush();
    }
}

/// Monitor rectangles for the root window via RandR, following the zoom
/// adapter's compatibility pattern. Missing RandR is not an error: callers
/// fall back to the root rectangle.
fn query_monitors(connection: &RustConnection, root: u32) -> Vec<Rect> {
    use randr::ConnectionExt as _;
    use x11rb::protocol::randr;

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
            .map(|monitor| Rect {
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
        .map(|info| Rect {
            x: i32::from(info.x),
            y: i32::from(info.y),
            width: u32::from(info.width),
            height: u32::from(info.height),
        })
        .collect()
}

/// Subscribe to RandR topology changes when the extension is available.
/// Missing RandR silently skips subscription; it is never a startup error.
fn select_randr_notifications(connection: &RustConnection, root: u32) {
    use randr::{ConnectionExt as _, NotifyMask};
    use x11rb::protocol::randr;

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

fn find_argb_visual(screen: &xproto::Screen) -> Option<&Visualtype> {
    screen.allowed_depths.iter().find_map(|depth| {
        if depth.depth != 32 {
            return None;
        }
        depth.visuals.iter().find(|visual| {
            visual.class == VisualClass::TRUE_COLOR
                && visual.red_mask == 0x00ff_0000
                && visual.green_mask == 0x0000_ff00
                && visual.blue_mask == 0x0000_00ff
        })
    })
}

fn pack_region(
    pixels: &[u8],
    source_width: u32,
    x: i32,
    y: i32,
    width: u16,
    height: u16,
    byte_order: ByteOrder,
) -> Vec<u8> {
    let mut data = vec![0u8; width as usize * height as usize * 4];
    let row_bytes = width as usize * 4;
    for row in 0..height as usize {
        let source_start = ((y as usize + row) * source_width as usize + x as usize) * 4;
        let source = &pixels[source_start..source_start + row_bytes];
        let destination = &mut data[row * row_bytes..(row + 1) * row_bytes];
        match byte_order {
            ByteOrder::Bgra => rgba_to_bgra(source, destination),
            ByteOrder::Argb => rgba_to_argb(source, destination),
        }
    }
    data
}

/// Convert RGBA rows to the BGRA byte order expected by little-endian servers.
pub(crate) fn rgba_to_bgra(source: &[u8], destination: &mut [u8]) {
    for (source, destination) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
        destination[0] = source[2];
        destination[1] = source[1];
        destination[2] = source[0];
        destination[3] = source[3];
    }
}

/// Convert RGBA rows to the ARGB byte order expected by big-endian servers.
pub(crate) fn rgba_to_argb(source: &[u8], destination: &mut [u8]) {
    for (source, destination) in source.chunks_exact(4).zip(destination.chunks_exact_mut(4)) {
        destination[0] = source[3];
        destination[1] = source[0];
        destination[2] = source[1];
        destination[3] = source[2];
    }
}

fn lost(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::ConnectionLost(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_conversion_preserves_premultiplied_alpha() {
        let mut destination = [0; 8];
        rgba_to_bgra(&[10, 20, 30, 40, 0, 0, 0, 0], &mut destination);
        assert_eq!(destination, [30, 20, 10, 40, 0, 0, 0, 0]);
    }

    #[test]
    fn big_endian_upload_conversion_keeps_alpha_first() {
        let mut destination = [0; 4];
        rgba_to_argb(&[10, 20, 30, 40], &mut destination);
        assert_eq!(destination, [40, 10, 20, 30]);
    }

    #[test]
    #[ignore = "requires an X11 session with a compositing manager"]
    fn annotation_window_has_empty_input_shape() {
        let backend = X11AnnotationBackend::new().expect("X11 backend requires a compositor");
        let reply = shape::get_rectangles(&backend.connection, backend.window, SK::INPUT)
            .expect("shape query")
            .reply()
            .expect("shape reply");
        assert_eq!(reply.rectangles_len(), 0);
        assert_eq!(backend.width, backend.size().0);
    }

    #[test]
    #[ignore = "requires an X11 session with a compositing manager"]
    fn annotation_native_clipboard_round_trip() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::thread;
        use std::time::Duration;

        use x11rb::protocol::Event;
        use x11rb::protocol::xproto::{PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent};

        let (owner_connection, screen) = x11rb::connect(None).expect("owner needs an X11 session");
        let root = owner_connection.setup().roots[screen].root;
        let utf8 = owner_connection
            .intern_atom(false, b"UTF8_STRING")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let clipboard = owner_connection
            .intern_atom(false, b"CLIPBOARD")
            .unwrap()
            .reply()
            .unwrap()
            .atom;
        let owner_window = owner_connection.generate_id().unwrap();
        owner_connection
            .create_window(
                0,
                owner_window,
                root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_ONLY,
                0,
                &CreateWindowAux::new(),
            )
            .unwrap();
        owner_connection
            .set_selection_owner(owner_window, clipboard, x11rb::CURRENT_TIME)
            .unwrap();
        owner_connection.flush().unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let owner_stop = Arc::clone(&stop);
        let owner = thread::spawn(move || {
            while !owner_stop.load(Ordering::Acquire) {
                if let Ok(Some(Event::SelectionRequest(request))) =
                    owner_connection.poll_for_event()
                {
                    let property = if request.target == utf8 {
                        owner_connection
                            .change_property(
                                PropMode::REPLACE,
                                request.requestor,
                                request.property,
                                utf8,
                                8,
                                5,
                                b"hello",
                            )
                            .unwrap();
                        request.property
                    } else {
                        0
                    };
                    let notify = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: request.time,
                        requestor: request.requestor,
                        selection: request.selection,
                        target: request.target,
                        property,
                    };
                    owner_connection
                        .send_event(false, request.requestor, EventMask::NO_EVENT, notify)
                        .unwrap();
                    owner_connection.flush().unwrap();
                }
                thread::sleep(Duration::from_millis(2));
            }
            let _ = owner_connection.destroy_window(owner_window);
            let _ = owner_connection.flush();
        });

        let mut backend = X11AnnotationBackend::new().expect("X11 backend requires a compositor");
        backend
            .request_paste(PasteRequest {
                session: 1,
                sequence: 1,
            })
            .expect("paste request");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut result = None;
        while Instant::now() < deadline && result.is_none() {
            match backend.poll_event(Instant::now()) {
                Ok(Some(BackendEvent::Paste(paste))) => result = Some(paste),
                Ok(_) => thread::sleep(Duration::from_millis(2)),
                Err(error) => panic!("clipboard poll failed: {error}"),
            }
        }
        stop.store(true, Ordering::Release);
        owner.join().unwrap();
        let result = result.expect("clipboard result before the deadline");
        assert_eq!(result.text, Ok("hello".to_owned()));
    }

    #[test]
    #[ignore = "requires an X11 session with a compositing manager"]
    fn annotation_native_map_upload_unmap() {
        let mut backend = X11AnnotationBackend::new().expect("X11 backend requires a compositor");
        let mut renderer = Renderer::new(64, 64).unwrap();
        let mut session =
            crate::Session::new(&rpresent_config::AnnotationConfig::default()).unwrap();
        session.command(crate::AnnotationCommand::On).unwrap();
        session.ownership(Some(1));
        session.pointer(
            1,
            crate::PointerInput::Down {
                root: crate::Point { x: 4, y: 4 },
                time: std::time::Duration::ZERO,
            },
        );
        session.pointer(
            1,
            crate::PointerInput::Up {
                root: crate::Point { x: 60, y: 60 },
                time: std::time::Duration::from_millis(10),
            },
        );
        renderer
            .redraw(&mut session, renderer.full_bounds())
            .unwrap();

        backend.present(true).expect("map");
        backend
            .upload(&renderer, renderer.full_bounds())
            .expect("upload");
        backend.present(false).expect("unmap");
    }
}
