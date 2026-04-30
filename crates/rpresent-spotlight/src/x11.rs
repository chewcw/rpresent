use x11rb::connection::{Connection, RequestConnection};
use x11rb::image::Image;
use x11rb::protocol::render::{
    self, ChangePictureAux, Color, ConnectionExt as RenderConnectionExt, CreatePictureAux,
    Fixed, PictOp, PictType, Picture, PictureWrapper, Pictformat, Pointfix, Repeat,
};
use x11rb::protocol::xproto::{
    ColormapAlloc, ConnectionExt as XprotoConnectionExt, CreateGCAux, CreateWindowAux, Gcontext,
    Rectangle, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::overlay::{CursorPosition, OverlayError, SpotlightOverlayBackend};
use rpresent_config::SpotlightConfig;

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

fn find_visual_format(
    reply: &render::QueryPictFormatsReply,
    screen_num: usize,
    visual_id: Visualid,
) -> Option<Pictformat> {
    let screen_formats = reply.screens.get(screen_num)?;
    let visual_format = screen_formats
        .depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual == visual_id)?;
    let format = reply
        .formats
        .iter()
        .find(|format| format.id == visual_format.format)?;

    if format.type_ == PictType::DIRECT
        && format.depth == 32
        && format.direct.red_shift == 16
        && format.direct.red_mask == 255
        && format.direct.green_shift == 8
        && format.direct.green_mask == 255
        && format.direct.blue_shift == 0
        && format.direct.blue_mask == 255
        && format.direct.alpha_shift == 24
        && format.direct.alpha_mask == 255
    {
        Some(format.id)
    } else {
        None
    }
}

fn fixed_from_i16(value: i16) -> Fixed {
    i32::from(value) << 16
}

fn fixed_from_u32(value: u32) -> Fixed {
    let clamped = value.min((i32::MAX >> 16) as u32);
    (clamped as i32) << 16
}

fn alpha_to_u16(alpha: f32) -> u16 {
    (alpha.clamp(0.0, 1.0) * u16::MAX as f32).round() as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DotCacheKey {
    size_px: u16,
    color_rgb: (u8, u8, u8),
    alpha_u16: u16,
}

fn dot_cache_key(config: &SpotlightConfig) -> Option<DotCacheKey> {
    if !config.show_center_dot {
        return None;
    }

    let size_px = config.dot_size_px.max(1).min(u16::MAX as u32) as u16;
    let color_rgb = parse_hex_rgb(&config.dot_color).unwrap_or((255, 255, 255));
    let alpha_u16 = alpha_to_u16(config.dot_opacity);

    Some(DotCacheKey {
        size_px,
        color_rgb,
        alpha_u16,
    })
}

fn parse_hex_rgb(color: &str) -> Option<(u8, u8, u8)> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

fn legacy_appearance(config: &rpresent_config::SpotlightConfig) -> crate::overlay::SpotlightAppearance {
    crate::overlay::SpotlightAppearance {
        radius_px: config.radius_px,
        softness_px: config.softness_px,
        dim_alpha: config.dim_alpha,
    }
}

fn create_gradient_picture<'a>(
    connection: &'a RustConnection,
    cursor: CursorPosition,
    appearance: &crate::overlay::SpotlightAppearance,
) -> Result<PictureWrapper<&'a RustConnection>, OverlayError> {
    let inner = Pointfix {
        x: fixed_from_i16(cursor.x),
        y: fixed_from_i16(cursor.y),
    };
    let outer = inner;
    let inner_radius = fixed_from_u32(appearance.radius_px);
    let outer_radius = fixed_from_u32(appearance.radius_px.saturating_add(appearance.softness_px));
    let stops: [Fixed; 2] = [0, 1 << 16];
    let colors = [
        Color {
            red: 0,
            green: 0,
            blue: 0,
            alpha: 0,
        },
        Color {
            red: 0,
            green: 0,
            blue: 0,
            alpha: alpha_to_u16(appearance.dim_alpha),
        },
    ];

    let gradient = PictureWrapper::create_radial_gradient(
        connection,
        inner,
        outer,
        inner_radius,
        outer_radius,
        &stops,
        &colors,
    )
    .map_err(runtime_error)?;

    connection
        .render_change_picture(
            gradient.picture(),
            &ChangePictureAux::new().repeat(Repeat::PAD),
        )
        .inspect_err(|error| {
            tracing::error!(cursor = ?cursor, ?error, "failed to pad spotlight gradient");
        })
        .map_err(runtime_error)?;

    Ok(gradient)
}

enum BackendKind {
    Render { window_picture: Picture },
    Software { gc: Gcontext, depth: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderStrategy {
    XrenderRadial,
    SoftwareFrame,
}

fn select_render_strategy(config: &rpresent_config::SpotlightConfig) -> RenderStrategy {
    let is_circle = matches!(config.shape, rpresent_config::SpotShape::Circle);
    let rotated = config.spot_rotation_deg != 0.0;

    if is_circle && !rotated {
        RenderStrategy::XrenderRadial
    } else {
        RenderStrategy::SoftwareFrame
    }
}

struct DotRenderCache {
    key: DotCacheKey,
    pixmap: u32,
    picture: Picture,
}

pub(crate) struct X11SpotlightOverlay {
    connection: RustConnection,
    screen_num: usize,
    window: Window,
    surface_size: (u16, u16),
    backend: BackendKind,
    dot_cache: Option<DotRenderCache>,
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

        let backend = create_backend(&connection, window, screen_num, visual_id, depth)?;

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
            surface_size: (spec.width, spec.height),
            backend,
            dot_cache: None,
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

}

fn free_dot_cache(connection: &RustConnection, cache: DotRenderCache) {
    let _ = connection.render_free_picture(cache.picture);
    let _ = connection.free_pixmap(cache.pixmap);
}

impl Drop for X11SpotlightOverlay {
    fn drop(&mut self) {
        if let Some(cache) = self.dot_cache.take() {
            free_dot_cache(&self.connection, cache);
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

    fn redraw(
        &mut self,
        cursor: CursorPosition,
        config: &SpotlightConfig,
    ) -> Result<(), OverlayError> {
        let result = match self.backend {
            BackendKind::Render { window_picture } => {
                let appearance = legacy_appearance(config);
                redraw_render(
                    &self.connection,
                    self.window,
                    window_picture,
                    self.surface_size,
                    cursor,
                    &appearance,
                    config,
                )
            }
            BackendKind::Software { gc, depth } => {
                redraw_software(
                    &self.connection,
                    self.window,
                    gc,
                    depth,
                    self.surface_size,
                    cursor,
                    config,
                )
            }
        };
        result.inspect_err(|error| {
            tracing::error!(window = self.window, ?error, "failed to redraw spotlight overlay");
        })?;
        self.flush()
    }
}

fn create_backend(
    connection: &RustConnection,
    window: Window,
    screen_num: usize,
    visual_id: Visualid,
    depth: u8,
) -> Result<BackendKind, OverlayError> {
    match connection
        .extension_information(render::X11_EXTENSION_NAME)
        .map_err(runtime_error)?
    {
        Some(_) => match connection
            .render_query_pict_formats()
            .map_err(runtime_error)?
            .reply()
        {
            Ok(reply) => match find_visual_format(&reply, screen_num, visual_id) {
                Some(pictformat) => match PictureWrapper::create_picture(
                    connection,
                    window,
                    pictformat,
                    &CreatePictureAux::default(),
                ) {
                    Ok(window_picture) => {
                        tracing::debug!(window, pictformat, "using XRender spotlight backend");
                        Ok(BackendKind::Render {
                            window_picture: window_picture.into_picture(),
                        })
                    }
                    Err(error) => {
                        tracing::warn!(
                            window,
                            ?error,
                            "failed to create XRender spotlight picture; falling back to software backend"
                        );
                        create_software_backend(connection, window, depth)
                    }
                },
                None => {
                    tracing::debug!(
                        window,
                        "XRender alpha format unavailable for spotlight visual; using software backend"
                    );
                    create_software_backend(connection, window, depth)
                }
            },
            Err(error) => {
                tracing::warn!(
                    window,
                    ?error,
                    "failed to query XRender formats; using software backend"
                );
                create_software_backend(connection, window, depth)
            }
        },
        None => create_software_backend(connection, window, depth),
    }
}

fn create_software_backend(
    connection: &RustConnection,
    window: Window,
    depth: u8,
) -> Result<BackendKind, OverlayError> {
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

    Ok(BackendKind::Software { gc, depth })
}

fn redraw_render(
    connection: &RustConnection,
    window: Window,
    window_picture: Picture,
    surface_size: (u16, u16),
    cursor: CursorPosition,
    appearance: &crate::overlay::SpotlightAppearance,
    config: &SpotlightConfig,
) -> Result<(), OverlayError> {
    let gradient = create_gradient_picture(connection, cursor, appearance)?;
    let _ = render::composite(
        connection,
        PictOp::SRC,
        gradient.picture(),
        x11rb::NONE,
        window_picture,
        0,
        0,
        0,
        0,
        0,
        0,
        surface_size.0,
        surface_size.1,
    )
    .inspect_err(|error| {
        tracing::error!(window, ?error, "failed to composite spotlight overlay");
    })
    .map_err(runtime_error)?;

    if config.show_center_dot {
        let dot_size = config.dot_size_px.max(1);
        let half = (dot_size / 2) as i32;
        let x = (cursor.x as i32 - half).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let y = (cursor.y as i32 - half).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let rect = Rectangle {
            x,
            y,
            width: dot_size.min(u16::MAX as u32) as u16,
            height: dot_size.min(u16::MAX as u32) as u16,
        };

        let (r, g, b) = parse_hex_rgb(&config.dot_color).unwrap_or((255, 255, 255));
        let color = Color {
            red: u16::from(r) * 257,
            green: u16::from(g) * 257,
            blue: u16::from(b) * 257,
            alpha: alpha_to_u16(config.dot_opacity),
        };

        let _ = render::fill_rectangles(
            connection,
            PictOp::OVER,
            window_picture,
            color,
            &[rect],
        )
        .inspect_err(|error| {
            tracing::error!(window, ?error, "failed to draw spotlight center-dot rectangle");
        })
        .map_err(runtime_error)?;
    }

    Ok(())
}

fn redraw_software(
    connection: &RustConnection,
    window: Window,
    gc: Gcontext,
    depth: u8,
    surface_size: (u16, u16),
    cursor: CursorPosition,
    config: &rpresent_config::SpotlightConfig,
) -> Result<(), OverlayError> {
    let frame = crate::render::build_projecteur_frame(surface_size, cursor, config);
    let mut image = Image::allocate_native(frame.width, frame.height, depth, connection.setup())
        .map_err(runtime_error)?;
    if image.data().len() != frame.pixels.len() {
        return Err(runtime_error(format!(
            "spotlight frame size mismatch: image expects {} bytes, frame has {} bytes",
            image.data().len(),
            frame.pixels.len()
        )));
    }
    image.data_mut().copy_from_slice(&frame.pixels);
    let _ = image
        .put(connection, window, gc, 0, 0)
        .inspect_err(|error| {
            tracing::error!(
                window,
                width = frame.width,
                height = frame.height,
                ?error,
                "failed to redraw spotlight overlay"
            );
        })
        .map_err(runtime_error)?;

    Ok(())
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
        let frame = crate::overlay::SpotlightFrame {
            width: 2,
            height: 1,
            pixels: vec![0, 0, 0, 255, 0, 0, 0, 0],
        };

        assert_eq!(
            frame.pixels.len(),
            frame.width as usize * frame.height as usize * 4
        );
    }

    #[test]
    fn default_circle_prefers_xrender_radial() {
        // Default config has show_border=true, show_center_dot=true — XRender fires anyway
        // and silently omits those features (same as 54ace59 behavior).
        let config = rpresent_config::SpotlightConfig::default();
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::XrenderRadial
        ));
    }

    #[test]
    fn rotated_circle_falls_back_to_software() {
        let mut config = rpresent_config::SpotlightConfig::default();
        config.spot_rotation_deg = 45.0;
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::SoftwareFrame
        ));
    }

    #[test]
    fn non_circle_shape_falls_back_to_software() {
        let mut config = rpresent_config::SpotlightConfig::default();
        config.shape = rpresent_config::SpotShape::Square { corner_radius_pct: 0 };
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::SoftwareFrame
        ));
    }

    #[test]
    fn alpha_to_u16_scales_correctly() {
        assert_eq!(alpha_to_u16(0.0), 0);
        assert_eq!(alpha_to_u16(1.0), u16::MAX);
        // Midpoint: 0.5 * 65535 = 32767.5 → rounds to 32768
        assert_eq!(alpha_to_u16(0.5), 32768);
    }

    #[test]
    fn fixed_from_i16_shifts_left_16() {
        assert_eq!(fixed_from_i16(1), 1 << 16);
        assert_eq!(fixed_from_i16(0), 0);
    }

    #[test]
    fn fixed_from_u32_clamps_large_values() {
        // i32::MAX >> 16 = 32767
        let max_safe: u32 = (i32::MAX >> 16) as u32;
        assert_eq!(fixed_from_u32(max_safe), (max_safe as i32) << 16);
        // Values above max should be clamped
        assert_eq!(fixed_from_u32(max_safe + 1), (max_safe as i32) << 16);
    }

    #[test]
    fn legacy_appearance_maps_config_fields() {
        let config = rpresent_config::SpotlightConfig {
            radius_px: 300,
            softness_px: 90,
            dim_alpha: 0.8,
            ..rpresent_config::SpotlightConfig::default()
        };

        let appearance = legacy_appearance(&config);

        assert_eq!(appearance.radius_px, 300);
        assert_eq!(appearance.softness_px, 90);
        assert!((appearance.dim_alpha - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn dot_cache_key_changes_when_visual_inputs_change() {
        let mut a = rpresent_config::SpotlightConfig::default();
        a.show_center_dot = true;
        a.dot_size_px = 8;
        a.dot_color = "#ffffff".to_owned();
        a.dot_opacity = 0.8;

        let mut b = a.clone();
        b.dot_size_px = 9;
        assert_ne!(dot_cache_key(&a), dot_cache_key(&b));

        b = a.clone();
        b.dot_color = "#ff0000".to_owned();
        assert_ne!(dot_cache_key(&a), dot_cache_key(&b));

        b = a.clone();
        b.dot_opacity = 0.5;
        assert_ne!(dot_cache_key(&a), dot_cache_key(&b));
    }

    #[test]
    fn dot_cache_key_none_when_dot_is_hidden() {
        let mut c = rpresent_config::SpotlightConfig::default();
        c.show_center_dot = false;
        assert_eq!(dot_cache_key(&c), None);
    }
}
