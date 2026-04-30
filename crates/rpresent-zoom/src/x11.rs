use x11rb::connection::Connection;
use x11rb::image::Image;
use x11rb::protocol::render::{self, ConnectionExt as RenderConnectionExt, PictType};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, ImageFormat, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::controller::ZoomState;
use crate::overlay::{zoom_sample_rect, OverlayError};

fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

/// Find an ARGB32 visual via XRender; fall back to root visual.
fn choose_visual(
    connection: &impl Connection,
    screen_num: usize,
) -> Result<(u8, Visualid), OverlayError> {
    let screen = &connection.setup().roots[screen_num];

    if connection
        .extension_information(render::X11_EXTENSION_NAME)
        .map_err(runtime_error)?
        .is_some()
    {
        let formats = connection
            .render_query_pict_formats()
            .map_err(runtime_error)?
            .reply()
            .map_err(runtime_error)?;

        if let Some(format) = formats.formats.iter().find(|f| {
            f.type_ == PictType::DIRECT
                && f.depth == 32
                && f.direct.red_shift == 16
                && f.direct.red_mask == 255
                && f.direct.green_shift == 8
                && f.direct.green_mask == 255
                && f.direct.blue_shift == 0
                && f.direct.blue_mask == 255
                && f.direct.alpha_shift == 24
                && f.direct.alpha_mask == 255
        }) {
            if let Some(visual) = formats
                .screens
                .get(screen_num)
                .and_then(|sf| {
                    sf.depths
                        .iter()
                        .flat_map(|d| d.visuals.iter())
                        .find(|v| v.format == format.id)
                })
            {
                return Ok((format.depth, visual.visual));
            }
        }
    }

    Ok((screen.root_depth, screen.root_visual))
}

pub(crate) struct X11ZoomOverlay {
    connection: RustConnection,
    root: Window,
    window: Window,
    gc: Gcontext,
    depth: u8,
    screen_width: u16,
    screen_height: u16,
    /// Reusable upload buffer; allocated once on first render.
    image_buffer: Option<Image<'static>>,
    visible: bool,
}

impl X11ZoomOverlay {
    pub(crate) fn new() -> Result<Self, OverlayError> {
        tracing::debug!("connecting to X11 for zoom overlay backend");

        let (connection, screen_num) = x11rb::connect(None)
            .inspect_err(|e| tracing::error!(?e, "failed to connect to X11 for zoom backend"))
            .map_err(runtime_error)?;

        let screen = &connection.setup().roots[screen_num];
        let screen_width = screen.width_in_pixels;
        let screen_height = screen.height_in_pixels;
        let root = screen.root;

        let (depth, visual_id) = choose_visual(&connection, screen_num)
            .inspect_err(|e| tracing::error!(?e, "failed to choose ARGB32 visual for zoom"))?;

        // Colormap required when depth differs from the parent (root).
        let colormap = connection.generate_id().map_err(runtime_error)?;
        connection
            .create_colormap(ColormapAlloc::NONE, colormap, root, visual_id)
            .inspect_err(|e| tracing::error!(?e, "failed to create zoom colormap"))
            .map_err(runtime_error)?;

        // Overlay window: override_redirect, no event mask (input passthrough).
        let window = connection.generate_id().map_err(runtime_error)?;
        let aux = CreateWindowAux::new()
            .override_redirect(1)
            .border_pixel(screen.black_pixel)
            .background_pixel(x11rb::NONE)
            .colormap(colormap)
            .event_mask(EventMask::NO_EVENT);

        connection
            .create_window(
                depth,
                window,
                root,
                0,
                0,
                screen_width,
                screen_height,
                0,
                WindowClass::INPUT_OUTPUT,
                visual_id,
                &aux,
            )
            .inspect_err(|e| tracing::error!(?e, "failed to create zoom overlay window"))
            .map_err(runtime_error)?;

        let gc = connection.generate_id().map_err(runtime_error)?;
        connection
            .create_gc(gc, window, &CreateGCAux::default().graphics_exposures(0))
            .inspect_err(|e| tracing::error!(?e, "failed to create zoom GC"))
            .map_err(runtime_error)?;

        connection.flush().map_err(runtime_error)?;

        tracing::debug!(
            screen_num,
            depth,
            screen_width,
            screen_height,
            window,
            "zoom X11 backend started"
        );

        Ok(Self {
            connection,
            root,
            window,
            gc,
            depth,
            screen_width,
            screen_height,
            image_buffer: None,
            visible: false,
        })
    }

    pub(crate) fn show(&mut self) -> Result<(), OverlayError> {
        if !self.visible {
            tracing::debug!(window = self.window, "showing zoom overlay");
            self.connection
                .map_window(self.window)
                .map_err(runtime_error)?;
            self.connection.flush().map_err(runtime_error)?;
            self.visible = true;
        }
        Ok(())
    }

    pub(crate) fn hide(&mut self) -> Result<(), OverlayError> {
        if self.visible {
            tracing::debug!(window = self.window, "hiding zoom overlay");
            self.connection
                .unmap_window(self.window)
                .map_err(runtime_error)?;
            self.connection.flush().map_err(runtime_error)?;
            self.visible = false;
        }
        Ok(())
    }

    /// Query cursor position on the root window.
    pub(crate) fn query_pointer(&self) -> Result<(f32, f32), OverlayError> {
        let reply = self
            .connection
            .query_pointer(self.root)
            .map_err(runtime_error)?
            .reply()
            .map_err(runtime_error)?;
        Ok((reply.root_x as f32, reply.root_y as f32))
    }

    /// Capture the magnified region and upload it to the overlay window.
    pub(crate) fn render(
        &mut self,
        state: &ZoomState,
    ) -> Result<(), OverlayError> {
        // Determine pivot: follow cursor or use state pivot.
        let pivot = if state.follow_cursor {
            self.query_pointer().unwrap_or(state.pivot)
        } else {
            state.pivot
        };

        let effective_pivot = ZoomState {
            pivot,
            enabled: state.enabled,
            scale: state.scale,
            follow_cursor: state.follow_cursor,
            capture_target: String::new(),
        };

        let rect =
            zoom_sample_rect(self.screen_width as u32, self.screen_height as u32, &effective_pivot);

        let src_x = rect.left as i16;
        let src_y = rect.top as i16;
        let src_w = (rect.width as u16).max(1);
        let src_h = (rect.height as u16).max(1);

        tracing::trace!(
            src_x,
            src_y,
            src_w,
            src_h,
            scale = state.scale,
            "capturing zoom source region"
        );

        // Capture source region from root window (ZPixmap = packed pixels).
        let captured = self
            .connection
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                src_x,
                src_y,
                src_w,
                src_h,
                !0u32,
            )
            .inspect_err(|e| tracing::error!(?e, "get_image failed for zoom capture"))
            .map_err(runtime_error)?
            .reply()
            .map_err(runtime_error)?;

        let dst_w = self.screen_width as usize;
        let dst_h = self.screen_height as usize;

        // Stride may include scanline padding; compute per-row byte count.
        let src_stride = captured.data.len() / src_h as usize;

        // Allocate upload buffer once (screen size is fixed for the session).
        if self.image_buffer.is_none() {
            let image = Image::allocate_native(
                self.screen_width,
                self.screen_height,
                self.depth,
                self.connection.setup(),
            )
            .map_err(runtime_error)?;
            self.image_buffer = Some(image);
        }

        let buffer = self
            .image_buffer
            .as_mut()
            .expect("image buffer was just allocated");

        // Destination stride may also include padding.
        let dst_stride = buffer.data().len() / dst_h;
        let buf_data = buffer.data_mut();

        // Nearest-neighbor scale: map each destination pixel to a source pixel.
        for dy in 0..dst_h {
            let sy = (dy * src_h as usize / dst_h).min(src_h as usize - 1);
            for dx in 0..dst_w {
                let sx = (dx * src_w as usize / dst_w).min(src_w as usize - 1);
                // 4 bytes/pixel assumed (bpp=32, the standard on modern X11 for depth 24 and 32).
                let src_idx = sy * src_stride + sx * 4;
                let dst_idx = dy * dst_stride + dx * 4;
                debug_assert!(src_idx + 3 < captured.data.len());
                debug_assert!(dst_idx + 3 < buf_data.len());
                buf_data[dst_idx] = captured.data[src_idx];
                buf_data[dst_idx + 1] = captured.data[src_idx + 1];
                buf_data[dst_idx + 2] = captured.data[src_idx + 2];
                // Force fully opaque so the compositor renders us on top.
                buf_data[dst_idx + 3] = 255;
            }
        }

        buffer
            .put(&self.connection, self.window, self.gc, 0, 0)
            .inspect_err(|e| tracing::error!(?e, "failed to upload zoom frame via PutImage"))
            .map_err(runtime_error)?;

        self.connection.flush().map_err(runtime_error)?;

        Ok(())
    }
}

impl Drop for X11ZoomOverlay {
    fn drop(&mut self) {
        let _ = self.connection.free_gc(self.gc);
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
    }
}
