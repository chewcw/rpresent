use x11rb::connection::Connection;
use x11rb::image::Image;
use x11rb::protocol::render::{self, ConnectionExt as RenderConnectionExt, PictType};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, ImageFormat, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::controller::{ZoomRuntimeConfig, ZoomRuntimeMessage};
use crate::navigation::{Camera, MouseState, NavigationConfig, apply_follow_cursor};
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
    enabled: bool,
    runtime: ZoomRuntimeConfig,
    camera: Camera,
    dragging: bool,
    last_cursor: (f32, f32),
    /// Reusable upload buffer; allocated once on first render.
    image_buffer: Option<Image<'static>>,
    /// Reusable source buffer for captured region (RGBA/BGRA bytes).
    src_storage: Vec<u8>,
    /// Reusable scaled frame pixmap at full screen size.
    scaled_frame: Option<tiny_skia::Pixmap>,
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

        if depth != 32 {
            return Err(OverlayError::Runtime(format!(
                "zoom X11 backend requires a 32-bit visual, but selected depth is {depth}; enable a 32-bit composited visual or disable zoom overlay"
            )));
        }

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

        let runtime = ZoomRuntimeConfig {
            initial_scale: 2.0,
            min_scale: 1.0,
            max_scale: 6.0,
            scroll_speed: 0.25,
            pan_friction: 4.0,
            scale_friction: 5.0,
            follow_cursor: true,
            capture_target: "active-monitor".to_owned(),
        };
        let center = (screen_width as f32 / 2.0, screen_height as f32 / 2.0);
        let camera = Camera::new(runtime.initial_scale, center);

        Ok(Self {
            connection,
            root,
            window,
            gc,
            depth,
            screen_width,
            screen_height,
            enabled: false,
            runtime,
            camera,
            dragging: false,
            last_cursor: center,
            image_buffer: None,
            src_storage: Vec::new(),
            scaled_frame: None,
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

    pub(crate) fn apply_message(&mut self, message: ZoomRuntimeMessage) -> Result<(), OverlayError> {
        match message {
            ZoomRuntimeMessage::Configure(runtime) => {
                self.runtime = runtime;
                let center = (self.screen_width as f32 / 2.0, self.screen_height as f32 / 2.0);
                self.camera = Camera::new(self.runtime.initial_scale, center);
                self.last_cursor = center;
                self.dragging = false;
            }
            ZoomRuntimeMessage::SetEnabled(enabled) => {
                self.enabled = enabled;
                if enabled {
                    self.show()?;
                } else {
                    self.dragging = false;
                    self.hide()?;
                }
            }
            ZoomRuntimeMessage::Scroll { delta, pivot } => {
                self.camera.scale_pivot = pivot;
                self.camera.delta_scale += delta * self.runtime.scroll_speed;
            }
            ZoomRuntimeMessage::DragStart { x, y } => {
                self.dragging = true;
                self.last_cursor = (x, y);
                self.camera.velocity = (0.0, 0.0);
            }
            ZoomRuntimeMessage::DragMove { x, y } => {
                if self.dragging {
                    self.camera.drag_move(self.last_cursor, (x, y));
                    self.last_cursor = (x, y);
                }
            }
            ZoomRuntimeMessage::DragEnd => {
                self.dragging = false;
            }
            ZoomRuntimeMessage::Reset => {
                let center = (self.screen_width as f32 / 2.0, self.screen_height as f32 / 2.0);
                self.camera = Camera::new(self.runtime.initial_scale, center);
                self.last_cursor = center;
                self.dragging = false;
            }
        }

        Ok(())
    }

    pub(crate) fn tick(&mut self, dt: f32) -> Result<(), OverlayError> {
        if !self.enabled {
            return Ok(());
        }

        if self.runtime.follow_cursor && let Ok(pointer) = self.query_pointer() {
            apply_follow_cursor(&mut self.camera, pointer, true, self.dragging);
        }

        let nav_cfg = NavigationConfig {
            min_scale: self.runtime.min_scale,
            max_scale: self.runtime.max_scale,
            drag_friction: self.runtime.pan_friction,
            scale_friction: self.runtime.scale_friction,
        };
        let mouse = MouseState {
            drag: self.dragging,
        };
        self.camera.update(
            &nav_cfg,
            dt,
            &mouse,
            (self.screen_width as f32, self.screen_height as f32),
        );

        let min_x = self.screen_width as f32 / (2.0 * self.camera.scale.max(1e-6));
        let max_x = self.screen_width as f32 - min_x;
        let min_y = self.screen_height as f32 / (2.0 * self.camera.scale.max(1e-6));
        let max_y = self.screen_height as f32 - min_y;
        self.camera.position.0 = self.camera.position.0.clamp(min_x, max_x);
        self.camera.position.1 = self.camera.position.1.clamp(min_y, max_y);

        self.render_current_frame()
    }

    fn render_current_frame(&mut self) -> Result<(), OverlayError> {
        let rect = zoom_sample_rect(
            self.screen_width as u32,
            self.screen_height as u32,
            self.camera.scale,
            self.camera.position,
        );

        let src_x = rect.left as i16;
        let src_y = rect.top as i16;
        let src_w = (rect.width as u16).max(1);
        let src_h = (rect.height as u16).max(1);

        tracing::trace!(
            src_x,
            src_y,
            src_w,
            src_h,
            scale = self.camera.scale,
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
        if src_h == 0 {
            return Err(OverlayError::Runtime("captured image has zero height".to_owned()));
        }
        let src_stride = captured.data.len() / src_h as usize;
        let src_row_bytes = src_w as usize * 4;
        let src_required = src_stride.saturating_mul(src_h as usize);
        if src_stride < src_row_bytes || captured.data.len() < src_required {
            return Err(OverlayError::Runtime(
                "unexpected XImage row layout for 32-bit zoom path".to_owned(),
            ));
        }

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

        if self.scaled_frame.is_none() {
            let pixmap = tiny_skia::Pixmap::new(self.screen_width as u32, self.screen_height as u32)
                .ok_or_else(|| OverlayError::Runtime("failed to allocate destination pixmap".to_owned()))?;
            self.scaled_frame = Some(pixmap);
        }

        let src_len = src_w as usize * src_h as usize * 4;
        if self.src_storage.len() != src_len {
            self.src_storage.resize(src_len, 0);
        }

        for sy in 0..src_h as usize {
            let src_row_start = sy * src_stride;
            let src_row_end = src_row_start + src_row_bytes;
            let dst_row_start = sy * src_row_bytes;
            let dst_row_end = dst_row_start + src_row_bytes;
            self.src_storage[dst_row_start..dst_row_end]
                .copy_from_slice(&captured.data[src_row_start..src_row_end]);
        }

        let src_pixmap = tiny_skia::PixmapRef::from_bytes(&self.src_storage, src_w as u32, src_h as u32)
            .ok_or_else(|| OverlayError::Runtime("failed to create source pixmap".to_owned()))?;
        let dst_pixmap = self
            .scaled_frame
            .as_mut()
            .expect("scaled frame pixmap was just allocated");

        let mut paint = tiny_skia::PixmapPaint::default();
        paint.quality = tiny_skia::FilterQuality::Bilinear;
        let scale_x = self.screen_width as f32 / src_w as f32;
        let scale_y = self.screen_height as f32 / src_h as f32;
        dst_pixmap.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        dst_pixmap.draw_pixmap(
            0,
            0,
            src_pixmap,
            &paint,
            tiny_skia::Transform::from_scale(scale_x, scale_y),
            None,
        );

        let dst_pixels = dst_pixmap.data();
        let buffer = self
            .image_buffer
            .as_mut()
            .expect("image buffer was just allocated");
        // Destination stride may also include padding.
        let dst_stride = buffer.data().len() / dst_h;
        let dst_row_bytes = dst_w * 4;
        if dst_stride < dst_row_bytes {
            return Err(OverlayError::Runtime(
                "unexpected destination image stride for 32-bit zoom path".to_owned(),
            ));
        }
        let buf_data = buffer.data_mut();
        for dy in 0..dst_h {
            let src_row_start = dy * dst_row_bytes;
            let src_row_end = src_row_start + dst_row_bytes;
            let dst_row_start = dy * dst_stride;
            let dst_row_end = dst_row_start + dst_row_bytes;
            buf_data[dst_row_start..dst_row_end]
                .copy_from_slice(&dst_pixels[src_row_start..src_row_end]);
        }

        for row in 0..dst_h {
            let row_start = row * dst_stride;
            let row_end = row_start + dst_stride;
            for px in buf_data[row_start..row_end].chunks_exact_mut(4) {
                px[3] = 255;
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
