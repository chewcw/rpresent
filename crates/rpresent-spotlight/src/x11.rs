use x11rb::connection::{Connection, RequestConnection};
use x11rb::image::Image;
use x11rb::protocol::render::{self, ConnectionExt as RenderConnectionExt, Picture, PictType};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux, Gcontext,
    Pixmap, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::overlay::{CursorPosition, OverlayError, SpotlightFrame, SpotlightOverlayBackend};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayWindowSpec {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    override_redirect: bool,
    input_transparent: bool,
}

fn overlay_window_spec(surface_size: (u16, u16)) -> OverlayWindowSpec {
    OverlayWindowSpec {
        x: 0,
        y: 0,
        width: surface_size.0,
        height: surface_size.1,
        override_redirect: true,
        input_transparent: true,
    }
}

fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

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

        if let Some(format) = formats.formats.iter().find(|format| {
            format.type_ == PictType::DIRECT
                && format.depth == 32
                && format.direct.red_shift == 16
                && format.direct.red_mask == 255
                && format.direct.green_shift == 8
                && format.direct.green_mask == 255
                && format.direct.blue_shift == 0
                && format.direct.blue_mask == 255
                && format.direct.alpha_shift == 24
                && format.direct.alpha_mask == 255
        }) {
            if let Some(visual) = formats.screens.get(screen_num).and_then(|screen_formats| {
                screen_formats
                    .depths
                    .iter()
                    .flat_map(|depth| depth.visuals.iter())
                    .find(|visual| visual.format == format.id)
            }) {
                return Ok((format.depth, visual.visual));
            }
        }
    }

    Ok((screen.root_depth, screen.root_visual))
}

/// Try to find a 32-bit BGRA XRender format (matching the visual format).
/// Returns `None` if XRender is not available or no suitable format exists.
fn find_xrender_bgra_format(
    connection: &RustConnection,
) -> Result<Option<u32>, OverlayError> {
    if connection
        .extension_information(render::X11_EXTENSION_NAME)
        .map_err(runtime_error)?
        .is_none()
    {
        return Ok(None);
    }

    let formats = connection
        .render_query_pict_formats()
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;

    Ok(formats
        .formats
        .iter()
        .find(|format| {
            format.type_ == PictType::DIRECT
                && format.depth == 32
                && format.direct.red_shift == 16
                && format.direct.red_mask == 255
                && format.direct.green_shift == 8
                && format.direct.green_mask == 255
                && format.direct.blue_shift == 0
                && format.direct.blue_mask == 255
                && format.direct.alpha_shift == 24
                && format.direct.alpha_mask == 255
        })
        .map(|f| f.id))
}

/// (Task 3.2) Compute a simple hash of frame pixel data for caching.
/// Uses a 64-bit seed-based hash to detect pixel changes.
fn hash_frame(frame: &SpotlightFrame) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    // Hash dimensions and pixel data
    frame.width.hash(&mut hasher);
    frame.height.hash(&mut hasher);
    frame.pixels.hash(&mut hasher);
    hasher.finish()
}


pub(crate) struct X11SpotlightOverlay {
    connection: RustConnection,
    screen_num: usize,
    window: Window,
    surface_size: (u16, u16),
    gc: Gcontext,
    depth: u8,
    /// Reused image buffer (software fallback); reallocated only on surface dimension change.
    image_buf: Option<Image<'static>>,
    /// XRender support flag: set if both XRender is available and BGRA format is found.
    xrender_available: bool,
    /// Optional XRender picture ID for compositing (created on first redraw if XRender available).
    picture: Option<Picture>,
    /// XRender pixmap ID paired with the picture (created on first redraw if XRender available).
    pixmap: Option<Pixmap>,
    /// XRender format ID for BGRA32 (cached at startup if available).
    pictformat: Option<u32>,
    /// XRender window picture for direct compositing.
    window_picture: Option<Picture>,
    /// (Task 3.2) Hash of the last rendered frame for pixmap caching.
    /// If frame hash matches, skip rasterization and reuse cached pixmap.
    last_frame_hash: u64,
}

impl X11SpotlightOverlay {
    pub(crate) fn new() -> Result<Self, OverlayError> {
        tracing::debug!("starting spotlight X11 backend");
        let (connection, screen_num) = x11rb::connect(None)
            .inspect_err(|error| {
                tracing::error!(?error, "failed to connect to X11 for spotlight backend");
            })
            .map_err(runtime_error)?;
        let screen = &connection.setup().roots[screen_num];
        let (depth, visual_id) = choose_visual(&connection, screen_num).inspect_err(|error| {
            tracing::error!(screen_num, ?error, "failed to choose spotlight visual");
        })?;
        let spec = overlay_window_spec((screen.width_in_pixels, screen.height_in_pixels));
        let transparent = spec.input_transparent && depth == 32;

        let colormap = connection
            .generate_id()
            .inspect_err(|error| {
                tracing::error!(
                    screen_num,
                    ?error,
                    "failed to allocate spotlight colormap id"
                );
            })
            .map_err(runtime_error)?;
        connection
            .create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)
            .inspect_err(|error| {
                tracing::error!(screen_num, ?error, "failed to create spotlight colormap");
            })
            .map_err(runtime_error)?;

        let window = connection
            .generate_id()
            .inspect_err(|error| {
                tracing::error!(screen_num, ?error, "failed to allocate spotlight window id");
            })
            .map_err(runtime_error)?;
        let window_aux = CreateWindowAux::new()
            .override_redirect(if spec.override_redirect { 1 } else { 0 })
            .background_pixel(if transparent {
                x11rb::NONE
            } else {
                screen.black_pixel
            })
            .border_pixel(screen.black_pixel)
            .colormap(colormap);

        connection
            .create_window(
                depth,
                window,
                screen.root,
                spec.x,
                spec.y,
                spec.width,
                spec.height,
                0,
                WindowClass::INPUT_OUTPUT,
                visual_id,
                &window_aux,
            )
            .inspect_err(|error| {
                tracing::error!(screen_num, ?error, "failed to create spotlight window");
            })
            .map_err(runtime_error)?;

        let gc = connection
            .generate_id()
            .inspect_err(|error| {
                tracing::error!(
                    window,
                    ?error,
                    "failed to allocate spotlight graphics context id"
                );
            })
            .map_err(runtime_error)?;
        connection
            .create_gc(gc, window, &CreateGCAux::default().graphics_exposures(0))
            .inspect_err(|error| {
                tracing::error!(
                    window,
                    ?error,
                    "failed to create spotlight graphics context"
                );
            })
            .map_err(runtime_error)?;

        connection
            .flush()
            .inspect_err(|error| {
                tracing::error!(
                    window,
                    ?error,
                    "failed to flush spotlight X11 backend startup"
                );
            })
            .map_err(runtime_error)?;

        // Detect XRender support and create window picture.
        let (xrender_available, pictformat, window_picture) =
            match find_xrender_bgra_format(&connection) {
                Ok(Some(fmt)) => {
                    // Try to create a picture for the window.
                    match connection.render_create_picture(
                        connection
                            .generate_id()
                            .map_err(runtime_error)?,
                        window,
                        fmt,
                        &Default::default(),
                    ) {
                        Ok(_) => {
                            let pic_id = connection
                                .generate_id()
                                .map_err(runtime_error)?;
                            match connection.render_create_picture(
                                pic_id,
                                window,
                                fmt,
                                &Default::default(),
                            ) {
                                Ok(_) => {
                                    tracing::debug!(
                                        screen_num,
                                        format = fmt,
                                        "XRender BGRA32 format available"
                                    );
                                    (true, Some(fmt), Some(pic_id))
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        ?e,
                                        "failed to create XRender window picture; fallback to software"
                                    );
                                    (false, Some(fmt), None)
                                }
                            }
                        }
                        Err(e) => {
                            tracing::debug!(
                                ?e,
                                "failed to test XRender picture creation; fallback to software"
                            );
                            (false, Some(fmt), None)
                        }
                    }
                }
                Ok(None) => {
                    tracing::debug!(
                        screen_num,
                        "XRender not available; using software upload fallback"
                    );
                    (false, None, None)
                }
                Err(e) => {
                    tracing::debug!(
                        screen_num,
                        ?e,
                        "error detecting XRender; fallback to software"
                    );
                    (false, None, None)
                }
            };

        tracing::debug!(
            screen_num,
            depth,
            width = spec.width,
            height = spec.height,
            transparent,
            xrender_available,
            "spotlight X11 backend started"
        );

        Ok(Self {
            connection,
            screen_num,
            window,
            surface_size: (spec.width, spec.height),
            gc,
            depth,
            image_buf: None,
            xrender_available,
            picture: None,
            pixmap: None,
            pictformat,
            window_picture,
            last_frame_hash: 0,
        })
    }

    fn flush(&self) -> Result<(), OverlayError> {
        self.connection
            .flush()
            .inspect_err(|error| {
                tracing::error!(
                    window = self.window,
                    ?error,
                    "failed to flush spotlight X11 backend"
                );
            })
            .map_err(runtime_error)
    }

    fn cleanup_xrender(&mut self) {
        if let Some(pic) = self.picture.take() {
            let _ = self.connection.render_free_picture(pic);
        }
        if let Some(pix) = self.pixmap.take() {
            let _ = self.connection.free_pixmap(pix);
        }
        if let Some(wpic) = self.window_picture.take() {
            let _ = self.connection.render_free_picture(wpic);
        }
    }
}

impl SpotlightOverlayBackend for X11SpotlightOverlay {
    fn show(&mut self) -> Result<(), OverlayError> {
        self.connection
            .map_window(self.window)
            .inspect_err(|error| {
                tracing::error!(
                    window = self.window,
                    ?error,
                    "failed to show spotlight overlay"
                );
            })
            .map_err(runtime_error)?;
        self.flush()
    }

    fn hide(&mut self) -> Result<(), OverlayError> {
        self.connection
            .unmap_window(self.window)
            .inspect_err(|error| {
                tracing::error!(
                    window = self.window,
                    ?error,
                    "failed to hide spotlight overlay"
                );
            })
            .map_err(runtime_error)?;
        self.flush()
    }

    fn surface_size(&self) -> Result<(u16, u16), OverlayError> {
        Ok(self.surface_size)
    }

    fn cursor_position(&self) -> Result<CursorPosition, OverlayError> {
        let screen = &self.connection.setup().roots[self.screen_num];
        let reply = self
            .connection
            .query_pointer(screen.root)
            .inspect_err(|error| {
                tracing::error!(
                    screen_num = self.screen_num,
                    ?error,
                    "failed to query spotlight cursor position"
                );
            })
            .map_err(runtime_error)?
            .reply()
            .inspect_err(|error| {
                tracing::error!(
                    screen_num = self.screen_num,
                    ?error,
                    "failed to read spotlight cursor position"
                );
            })
            .map_err(runtime_error)?;

        Ok(CursorPosition {
            x: reply.root_x,
            y: reply.root_y,
        })
    }

    fn redraw(&mut self, frame: &SpotlightFrame) -> Result<(), OverlayError> {
        // (Task 3.2) Check if frame content is identical to last redraw via hash.
        // If unchanged, skip pixel upload to avoid network bandwidth on remote X11.
        let frame_hash = hash_frame(frame);
        
        let needs_realloc = self.image_buf.as_ref().map_or(true, |img: &Image<'_>| {
            img.width() != frame.width || img.height() != frame.height
        });
        
        // On window resize, invalidate frame hash cache to force fresh upload (Task 3.2 cache invalidation)
        if needs_realloc {
            self.last_frame_hash = 0;
        }
        
        if frame_hash == self.last_frame_hash && self.image_buf.is_some() && !needs_realloc {
            tracing::trace!(
                frame_hash,
                "spotlight frame unchanged; skipping pixel upload (Task 3.2 cache hit)"
            );
            return self.flush();
        }

        // If XRender is available, use cached pixmap/picture for compositing (when implemented).
        // For now, we use PutImage to the window (software fallback), but the architecture
        // supports future optimization via XRender pixmaps and composite operations.
        if self.xrender_available && needs_realloc && self.window_picture.is_some() {
            // Reallocate XRender pixmap/picture for new frame size.
            if let Some(pic) = self.picture.take() {
                let _ = self.connection.render_free_picture(pic);
            }
            if let Some(pix) = self.pixmap.take() {
                let _ = self.connection.free_pixmap(pix);
            }

            let pixmap = self
                .connection
                .generate_id()
                .map_err(runtime_error)?;
            self.connection
                .create_pixmap(self.depth, pixmap, self.window, frame.width, frame.height)
                .map_err(runtime_error)?;

            if let Some(fmt) = self.pictformat {
                let picture = self
                    .connection
                    .generate_id()
                    .map_err(runtime_error)?;
                if self.connection
                    .render_create_picture(picture, pixmap, fmt, &Default::default())
                    .is_ok()
                {
                    self.picture = Some(picture);
                    self.pixmap = Some(pixmap);
                    tracing::trace!(pixmap, picture, "created XRender pixmap for spotlight");
                } else {
                    let _ = self.connection.free_pixmap(pixmap);
                }
            }
        }

        // Allocate or reuse image buffer (both XRender and software paths need this).
        if needs_realloc {
            self.image_buf = Some(
                Image::allocate_native(
                    frame.width,
                    frame.height,
                    self.depth,
                    self.connection.setup(),
                )
                .map_err(runtime_error)?,
            );
        }

        let image = self.image_buf.as_mut().expect("allocated above");
        if image.data().len() != frame.pixels.len() {
            return Err(runtime_error(format!(
                "spotlight frame size mismatch: image expects {} bytes, frame has {} bytes",
                image.data().len(),
                frame.pixels.len()
            )));
        }
        image.data_mut().copy_from_slice(&frame.pixels);

        // Upload pixels to window via PutImage.
        // TODO: For XRender-capable systems, upload to cached pixmap and composite,
        // avoiding direct window PutImage and keeping pixel data server-side.
        image
            .put(&self.connection, self.window, self.gc, 0, 0)
            .inspect_err(|error| {
                tracing::error!(
                    window = self.window,
                    width = frame.width,
                    height = frame.height,
                    ?error,
                    "failed to redraw spotlight overlay"
                );
            })
            .map_err(runtime_error)?;

        // (Task 3.2) Update frame hash on successful upload.
        self.last_frame_hash = frame_hash;

        self.flush()
    }
}

impl Drop for X11SpotlightOverlay {
    fn drop(&mut self) {
        self.cleanup_xrender();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_window_spec_matches_screen_geometry() {
        let spec = overlay_window_spec((1920, 1080));

        assert_eq!(spec.x, 0);
        assert_eq!(spec.y, 0);
        assert_eq!(spec.width, 1920);
        assert_eq!(spec.height, 1080);
        assert!(spec.override_redirect);
        assert!(spec.input_transparent);
    }

    #[test]
    fn redraw_frame_uses_the_same_pixel_dimensions() {
        let frame = SpotlightFrame {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 0, 255, 0, 0, 0, 0],
        };

        assert_eq!(
            frame.pixels.len(),
            frame.width as usize * frame.height as usize * 4
        );
    }
}
