use std::os::fd::{AsRawFd, RawFd};
use std::time::Instant;

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::shape::{self, SK, SO};
use x11rb::protocol::xproto::{
    self, ChangeWindowAttributesAux, ColormapAlloc, ConfigureWindowAux, ConnectionExt,
    CreateGCAux, CreateWindowAux, EventMask, ImageFormat, ImageOrder, VisualClass, Visualtype,
    WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::error::OverlayError;
use crate::geometry::Rect;
use crate::render::Renderer;
use crate::runtime::{AnnotationBackend, BackendEvent};

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
            .create_colormap(
                ColormapAlloc::NONE,
                colormap,
                root,
                visual_id,
            )
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

        Ok(Self {
            connection,
            root,
            window,
            colormap,
            gc,
            width,
            height,
            byte_order,
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
}

impl AnnotationBackend for X11AnnotationBackend {
    fn window_id(&self) -> u32 {
        self.window
    }

    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
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
                let data = pack_region(pixels, source_width, clip.x, y, clip.width as u16, rows, self.byte_order);
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

    fn poll_event(&mut self, _now: Instant) -> Result<Option<BackendEvent>, OverlayError> {
        while let Some(event) = self.connection.poll_for_event().map_err(lost)? {
            match event {
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
                        return Ok(Some(BackendEvent::Resized { width, height }));
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
                        return Ok(Some(BackendEvent::Resized { width, height }));
                    }
                }
                _ => {}
            }
        }
        Ok(None)
    }

    fn next_deadline(&self) -> Option<Instant> {
        None
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
    fn annotation_native_map_upload_unmap() {
        let mut backend = X11AnnotationBackend::new().expect("X11 backend requires a compositor");
        let mut renderer = Renderer::new(64, 64).unwrap();
        let mut session = crate::Session::new(&rpresent_config::AnnotationConfig::default()).unwrap();
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
        renderer.redraw(&mut session, renderer.full_bounds()).unwrap();

        backend.present(true).expect("map");
        backend
            .upload(&renderer, renderer.full_bounds())
            .expect("upload");
        backend.present(false).expect("unmap");
    }
}
