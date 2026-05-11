use x11rb::connection::Connection;
use x11rb::image::Image;
use x11rb::protocol::render::{self, ConnectionExt as RenderConnectionExt, PictType};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux, EventMask,
    Gcontext, ImageFormat, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::capture::{CapturedFrame, CaptureError, FrameSource, ZoomCapture};
use crate::controller::{ZoomRuntimeConfig, ZoomRuntimeMessage};
use crate::navigation::{Camera, MouseState, NavigationConfig};
use crate::overlay::{OverlayError, screenshot_placement};
use crate::screenshot::{CaptureMode, ScreenshotState};

fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

struct X11FrameSource {
    connection: RustConnection,
    root: Window,
    screen_width: u16,
    screen_height: u16,
}

impl X11FrameSource {
    fn new(screen_width: u16, screen_height: u16) -> Result<Self, OverlayError> {
        tracing::debug!(screen_width, screen_height, "connecting to X11 for zoom screenshot capture");

        let (connection, screen_num) = x11rb::connect(None)
            .inspect_err(|e| tracing::error!(?e, "failed to connect to X11 for zoom screenshot capture"))
            .map_err(runtime_error)?;

        let root = connection.setup().roots[screen_num].root;

        Ok(Self {
            connection,
            root,
            screen_width,
            screen_height,
        })
    }
}

impl FrameSource for X11FrameSource {
    fn capture_frame(&mut self) -> Result<CapturedFrame, CaptureError> {
        capture_root_frame(
            &self.connection,
            self.root,
            self.screen_width,
            self.screen_height,
        )
    }
}

fn capture_root_frame(
    connection: &impl Connection,
    root: Window,
    screen_width: u16,
    screen_height: u16,
) -> Result<CapturedFrame, CaptureError> {
    if screen_height == 0 {
        return Err(CaptureError::Backend("captured image has zero height".to_owned()));
    }

    let captured = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            root,
            0,
            0,
            screen_width,
            screen_height,
            !0u32,
        )
        .map_err(|error| CaptureError::Backend(error.to_string()))?
        .reply()
        .map_err(|error| CaptureError::Backend(error.to_string()))?;


    let row_bytes = screen_width as usize * 4;
    let src_stride = captured.data.len() / screen_height as usize;
    if src_stride < row_bytes || captured.data.len() < src_stride * screen_height as usize {
        return Err(CaptureError::Backend(
            "unexpected XImage row layout for zoom screenshot capture".to_owned(),
        ));
    }

    let mut rgba = vec![0u8; row_bytes * screen_height as usize];
    for row in 0..screen_height as usize {
        let src_start = row * src_stride;
        let src_end = src_start + row_bytes;
        let dst_start = row * row_bytes;
        let dst_end = dst_start + row_bytes;
        rgba[dst_start..dst_end].copy_from_slice(&captured.data[src_start..src_end]);
    }

    for pixel in rgba.chunks_exact_mut(4) {
        pixel[3] = 255;
    }

    Ok(CapturedFrame::new(
        screen_width as u32,
        screen_height as u32,
        rgba,
    ))
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
    capture: ZoomCapture<X11FrameSource>,
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
    capture_state: ScreenshotState,
    cached_frame: Option<CapturedFrame>,
    /// Reusable upload buffer; allocated once on first render.
    image_buffer: Option<Image<'static>>,
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
        let capture = ZoomCapture::new(
            X11FrameSource::new(screen_width, screen_height)
                .inspect_err(|e| tracing::error!(?e, "failed to create zoom screenshot capture source"))?,
        );

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
            window,
            gc,
            depth,
            screen_width,
            screen_height,
            enabled: false,
            runtime,
            camera,
            capture,
            dragging: false,
            last_cursor: center,
            capture_state: ScreenshotState::new(CaptureMode::Static),
            cached_frame: None,
            image_buffer: None,
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
                if enabled {
                    self.capture_state.request_refresh();
                    if self.capture_state.should_refresh(0.0) {
                        let frame = self.capture.next_frame().map_err(runtime_error)?;
                        self.cached_frame = Some(frame);
                    }
                    self.enabled = true;
                    self.show()?;
                } else {
                    self.enabled = false;
                    self.dragging = false;
                    self.cached_frame = None;
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

        self.render_current_frame()
    }

    fn render_current_frame(&mut self) -> Result<(), OverlayError> {
        let frame = self
            .cached_frame
            .as_ref()
            .ok_or_else(|| OverlayError::Runtime("zoom screenshot is missing".to_owned()))?;

        let src_pixmap = tiny_skia::PixmapRef::from_bytes(&frame.rgba, frame.width, frame.height)
            .ok_or_else(|| OverlayError::Runtime("failed to create source pixmap".to_owned()))?;

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

        let dst_pixmap = self
            .scaled_frame
            .as_mut()
            .expect("scaled frame pixmap was just allocated");

        let mut paint = tiny_skia::PixmapPaint::default();
        paint.quality = tiny_skia::FilterQuality::Bilinear;
        let placement = screenshot_placement(
            self.screen_width as u32,
            self.screen_height as u32,
            self.camera.scale,
            self.camera.position,
        );
        let transform = tiny_skia::Transform::from_scale(placement.scale, placement.scale)
            .post_translate(placement.translate_x, placement.translate_y);

        dst_pixmap.fill(tiny_skia::Color::from_rgba8(0, 0, 0, 255));
        dst_pixmap.draw_pixmap(0, 0, src_pixmap, &paint, transform, None);

        let dst_pixels = dst_pixmap.data();
        let buffer = self
            .image_buffer
            .as_mut()
            .expect("image buffer was just allocated");
        let dst_h = self.screen_height as usize;
        let dst_w = self.screen_width as usize;
        let dst_row_bytes = dst_w * 4;
        let dst_stride = buffer.data().len() / dst_h;
        if dst_stride < dst_row_bytes {
            return Err(OverlayError::Runtime(
                "unexpected destination image stride for 32-bit zoom path".to_owned(),
            ));
        }

        let buf_data = buffer.data_mut();
        for row in 0..dst_h {
            let src_row_start = row * dst_row_bytes;
            let src_row_end = src_row_start + dst_row_bytes;
            let dst_row_start = row * dst_stride;
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
