use x11rb::connection::Connection;
use x11rb::protocol::render::{self, ConnectionExt as RenderConnectionExt, PictType};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux, Gcontext,
    ImageFormat, Visualid, Window, WindowClass,
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

fn frame_to_put_image_bytes(frame: &SpotlightFrame) -> Vec<u8> {
    frame.pixels.clone()
}

fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

fn choose_visual(connection: &impl Connection, screen_num: usize) -> Result<(u8, Visualid), OverlayError> {
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

pub(crate) struct X11SpotlightOverlay {
    connection: RustConnection,
    screen_num: usize,
    window: Window,
    gc: Gcontext,
    depth: u8,
    surface_size: (u16, u16),
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

        let colormap = connection.generate_id().inspect_err(|error| {
            tracing::error!(screen_num, ?error, "failed to allocate spotlight colormap id");
        })
        .map_err(runtime_error)?;
        connection
            .create_colormap(ColormapAlloc::NONE, colormap, screen.root, visual_id)
            .inspect_err(|error| {
                tracing::error!(screen_num, ?error, "failed to create spotlight colormap");
            })
            .map_err(runtime_error)?;

        let window = connection.generate_id().inspect_err(|error| {
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

        let gc = connection.generate_id().inspect_err(|error| {
            tracing::error!(screen_num, ?error, "failed to allocate spotlight graphics context id");
        })
        .map_err(runtime_error)?;
        connection
            .create_gc(gc, window, &CreateGCAux::default().graphics_exposures(0))
            .inspect_err(|error| {
                tracing::error!(window, ?error, "failed to create spotlight graphics context");
            })
            .map_err(runtime_error)?;

        connection.flush().inspect_err(|error| {
            tracing::error!(window, ?error, "failed to flush spotlight X11 backend startup");
        })
        .map_err(runtime_error)?;

        tracing::debug!(
            screen_num,
            depth,
            width = spec.width,
            height = spec.height,
            transparent,
            "spotlight X11 backend started"
        );

        Ok(Self {
            connection,
            screen_num,
            window,
            gc,
            depth,
            surface_size: (spec.width, spec.height),
        })
    }

    fn flush(&self) -> Result<(), OverlayError> {
        self.connection.flush().inspect_err(|error| {
            tracing::error!(window = self.window, ?error, "failed to flush spotlight X11 backend");
        })
        .map_err(runtime_error)
    }
}

impl SpotlightOverlayBackend for X11SpotlightOverlay {
    fn show(&mut self) -> Result<(), OverlayError> {
        self.connection
            .map_window(self.window)
            .inspect_err(|error| {
                tracing::error!(window = self.window, ?error, "failed to show spotlight overlay");
            })
            .map_err(runtime_error)?;
        self.flush()
    }

    fn hide(&mut self) -> Result<(), OverlayError> {
        self.connection
            .unmap_window(self.window)
            .inspect_err(|error| {
                tracing::error!(window = self.window, ?error, "failed to hide spotlight overlay");
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
                tracing::error!(screen_num = self.screen_num, ?error, "failed to query spotlight cursor position");
            })
            .map_err(runtime_error)?
            .reply()
            .inspect_err(|error| {
                tracing::error!(screen_num = self.screen_num, ?error, "failed to read spotlight cursor position");
            })
            .map_err(runtime_error)?;

        Ok(CursorPosition {
            x: reply.root_x,
            y: reply.root_y,
        })
    }

    fn redraw(&mut self, frame: &SpotlightFrame) -> Result<(), OverlayError> {
        let bytes = frame_to_put_image_bytes(frame);
        self.connection
            .put_image(
                ImageFormat::Z_PIXMAP,
                self.window,
                self.gc,
                frame.width,
                frame.height,
                0,
                0,
                0,
                self.depth,
                &bytes,
            )
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
        self.flush()
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
    fn frame_to_put_image_bytes_keeps_the_alpha_payload() {
        let frame = SpotlightFrame {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 0, 255, 0, 0, 0, 0],
        };

        assert_eq!(frame_to_put_image_bytes(&frame), frame.pixels);
    }
}