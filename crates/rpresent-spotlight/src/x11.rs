use tiny_skia::{Color as SkiaColor, FillRule, Paint as SkiaPaint, Pixmap, Transform};
use x11rb::connection::{Connection, RequestConnection};
use x11rb::image::Image;
use x11rb::protocol::render::{
    self, ChangePictureAux, Color, ConnectionExt as RenderConnectionExt, CreatePictureAux, Fixed,
    PictOp, PictType, Pictformat, Picture, PictureWrapper, Pointfix, Repeat,
};
use x11rb::protocol::shape::{ConnectionExt as ShapeConnectionExt, SK, SO};
use x11rb::protocol::shm::ConnectionExt as ShmConnectionExt;
use x11rb::protocol::xproto::{
    ClipOrdering, ColormapAlloc, ConfigureWindowAux, ConnectionExt as XprotoConnectionExt,
    CreateGCAux, CreateWindowAux, Gcontext, ImageFormat, Rectangle, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::damage;
use crate::overlay::{CursorPosition, OverlayError, SpotlightOverlayBackend, runtime_error};
use crate::shape::{ShapeGeometry, build_circle_decorations, parse_rgb};
use crate::shm::ShmSegment;
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
        }) && let Some(visual) = formats.screens.get(screen_num).and_then(|screen_formats| {
            screen_formats
                .depths
                .iter()
                .flat_map(|depth| depth.visuals.iter())
                .find(|visual| visual.format == format.id)
        }) {
            return Ok((format.depth, visual.visual));
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
struct DecorationCacheKey {
    radius_px: u32,
    border_size_pct: u32,
    border_color_rgb: (u8, u8, u8),
    border_alpha_u16: u16,
    dot_size_px: u16,
    dot_color_rgb: (u8, u8, u8),
    dot_alpha_u16: u16,
}

fn decoration_cache_key(config: &SpotlightConfig, radius_px: u32) -> Option<DecorationCacheKey> {
    if !config.show_border && !config.show_center_dot {
        return None;
    }

    let border_color_rgb = if config.show_border {
        parse_rgb(&config.border_color)
    } else {
        (0, 0, 0)
    };
    let border_alpha_u16 = if config.show_border {
        alpha_to_u16(config.border_opacity)
    } else {
        0
    };
    let dot_size_px = if config.show_center_dot {
        config.dot_size_px.max(1).min(u16::MAX as u32) as u16
    } else {
        0
    };
    let dot_color_rgb = if config.show_center_dot {
        parse_rgb(&config.dot_color)
    } else {
        (0, 0, 0)
    };
    let dot_alpha_u16 = if config.show_center_dot {
        alpha_to_u16(config.dot_opacity)
    } else {
        0
    };

    Some(DecorationCacheKey {
        radius_px: if config.show_border { radius_px } else { 0 },
        border_size_pct: if config.show_border {
            config.border_size_pct.min(99)
        } else {
            0
        },
        border_color_rgb,
        border_alpha_u16,
        dot_size_px,
        dot_color_rgb,
        dot_alpha_u16,
    })
}

fn shade_fill_color(config: &SpotlightConfig) -> Color {
    let (red, green, blue) = parse_rgb(&config.shade_color);
    let alpha = alpha_to_u16(config.shade_opacity);
    Color {
        red: premultiply_u16(red, alpha),
        green: premultiply_u16(green, alpha),
        blue: premultiply_u16(blue, alpha),
        alpha,
    }
}

fn premultiply_u16(channel: u8, alpha: u16) -> u16 {
    ((u32::from(channel) * 257 * u32::from(alpha) + u32::from(u16::MAX) / 2) / u32::from(u16::MAX))
        as u16
}

fn create_gradient_picture<'a>(
    connection: &'a RustConnection,
    cursor: CursorPosition,
    config: &rpresent_config::SpotlightConfig,
    radius_px: u32,
) -> Result<PictureWrapper<&'a RustConnection>, OverlayError> {
    let inner = Pointfix {
        x: fixed_from_i16(cursor.x),
        y: fixed_from_i16(cursor.y),
    };
    let outer = inner;
    let inner_radius = fixed_from_u32(radius_px);
    let outer_radius = fixed_from_u32(radius_px.saturating_add(config.softness_px));
    let stops: [Fixed; 2] = [0, 1 << 16];
    let (red, green, blue) = parse_rgb(&config.shade_color);
    let red = u16::from(red) * 257;
    let green = u16::from(green) * 257;
    let blue = u16::from(blue) * 257;
    let colors = [
        Color {
            red,
            green,
            blue,
            alpha: 0,
        },
        Color {
            red,
            green,
            blue,
            alpha: alpha_to_u16(config.shade_opacity),
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
    Render {
        window_picture: Picture,
        gc: Gcontext,
        depth: u8,
    },
    Shm {
        gc: Gcontext,
        depth: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderStrategy {
    Render,
    Shm,
}

fn select_render_strategy(config: &rpresent_config::SpotlightConfig) -> RenderStrategy {
    let is_circle = matches!(config.shape, rpresent_config::SpotShape::Circle);

    if is_circle {
        RenderStrategy::Render
    } else {
        RenderStrategy::Shm
    }
}

struct DecorationRenderCache {
    key: DecorationCacheKey,
    pixmap: u32,
    picture: Picture,
    center_offset: i16,
    width: u16,
    height: u16,
}

struct HardCutoutCache {
    radius_px: u32,
    pixmap: u32,
    picture: Picture,
    center_offset: i16,
    width: u16,
    height: u16,
}

pub(crate) struct X11SpotlightOverlay {
    connection: RustConnection,
    screen_num: usize,
    window: Window,
    surface_size: (u16, u16),
    backend: BackendKind,
    shm: ShmSegment,
    decoration_cache: Option<DecorationRenderCache>,
    hard_cutout_cache: Option<HardCutoutCache>,
    last_paint_bounds: Option<damage::DamageRect>,
    last_config: Option<SpotlightConfig>,
    needs_full_redraw: bool,
}

impl X11SpotlightOverlay {
    pub(crate) fn new() -> Result<Self, OverlayError> {
        tracing::debug!("starting spotlight X11 backend");
        let (connection, screen_num) = x11rb::connect(None)
            .inspect_err(|error| {
                tracing::error!(?error, "failed to connect to X11 for spotlight backend");
            })
            .map_err(runtime_error)?;
        check_required_extensions(&connection)?;
        let screen = &connection.setup().roots[screen_num];
        let (depth, visual_id) = choose_visual(&connection, screen_num).inspect_err(|error| {
            tracing::error!(screen_num, ?error, "failed to choose spotlight visual");
        })?;
        check_surface_format(&connection, depth)?;
        let spec = overlay_window_spec((screen.width_in_pixels, screen.height_in_pixels));
        let transparent = spec.input_transparent && depth == 32;

        // X11 colormap: a lookup table mapping pixel values to RGB colors.
        // For our direct-color ARGB visual the color lives inside the pixel
        // bits themselves (red/green/blue/alpha from choose_visual), so the
        // colormap is not consulted at draw time. X11 still requires every
        // window to carry one, so we create it here and attach it below.
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
        // ColormapAlloc::NONE: create the colormap but pre-allocate no color
        // cells. Direct-color visuals never ask the server for color slots,
        // so there is nothing to reserve. This matches the ARGB pixel format
        // chosen in choose_visual(), where full RGBA is stored directly in
        // each 32-bit pixel rather than looked up from a table.
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
            // Bind the colormap to the window. The server uses it to read
            // the pixel format. For our direct-color visual, the pixel bits
            // already encode full RGBA, so the colormap is not consulted at
            // draw time.
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

        connection
            .shape_rectangles(
                SO::SET,
                SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[],
            )
            .map_err(runtime_error)?
            .check()
            .map_err(runtime_error)?;

        let backend = create_backend(&connection, window, screen_num, visual_id, depth)?;
        let shm = ShmSegment::new(&connection, spec.width, spec.height)?;

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
            shm,
            decoration_cache: None,
            hard_cutout_cache: None,
            last_paint_bounds: None,
            last_config: None,
            needs_full_redraw: true,
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

    fn refresh_surface_size(&mut self) -> Result<(), OverlayError> {
        let next = self.surface_size()?;
        if next != self.surface_size {
            self.connection
                .configure_window(
                    self.window,
                    &ConfigureWindowAux::new()
                        .width(u32::from(next.0))
                        .height(u32::from(next.1)),
                )
                .map_err(runtime_error)?
                .check()
                .map_err(runtime_error)?;
            self.surface_size = next;
            // Repaint after every resize, including shrinkage within the SHM capacity.
            self.needs_full_redraw = true;
        }
        Ok(())
    }

    fn redraw_shm(
        &mut self,
        gc: Gcontext,
        depth: u8,
        cursor: CursorPosition,
        config: &SpotlightConfig,
        radius_px: u32,
    ) -> Result<(), OverlayError> {
        let (surface_width, surface_height) = self.surface_size;
        let full_rect = (0_i16, 0_i16, surface_width, surface_height);
        let config_changed = self.last_config.as_ref() != Some(config);

        let mut full_redraw = std::mem::take(&mut self.needs_full_redraw) || config_changed;
        if self
            .shm
            .ensure_capacity(&self.connection, surface_width, surface_height)?
        {
            full_redraw = true;
        }

        let next_bounds = damage::paint_bounds(self.surface_size, cursor, config, radius_px);
        let damage_rect = if full_redraw {
            full_rect
        } else {
            self.last_paint_bounds
                .map(|previous| damage::union(previous, next_bounds, self.surface_size))
                .unwrap_or(full_rect)
        };

        if damage_rect.2 > 0 && damage_rect.3 > 0 {
            let frame =
                crate::render::build_projecteur_frame_rect(damage_rect, cursor, config, radius_px);
            crate::shm::blit_patch(
                self.shm.bytes_mut(),
                surface_width,
                &frame,
                (damage_rect.0 as u16, damage_rect.1 as u16),
            )?;
            self.connection
                .shm_put_image(
                    self.window,
                    gc,
                    surface_width,
                    surface_height,
                    damage_rect.0 as u16,
                    damage_rect.1 as u16,
                    damage_rect.2,
                    damage_rect.3,
                    damage_rect.0,
                    damage_rect.1,
                    depth,
                    u8::from(ImageFormat::Z_PIXMAP),
                    false,
                    self.shm.seg(),
                    0,
                )
                .map_err(runtime_error)?;
        }

        self.last_paint_bounds = Some(next_bounds);
        if config_changed {
            self.last_config = Some(config.clone());
        }
        Ok(())
    }
}

fn free_picture_cache(connection: &RustConnection, pixmap: u32, picture: Picture) {
    let _ = connection.render_free_picture(picture);
    let _ = connection.free_pixmap(pixmap);
}

fn free_decoration_cache(connection: &RustConnection, cache: DecorationRenderCache) {
    free_picture_cache(connection, cache.pixmap, cache.picture);
}

fn free_hard_cutout_cache(connection: &RustConnection, cache: HardCutoutCache) {
    free_picture_cache(connection, cache.pixmap, cache.picture);
}

fn argb32_pictformat(connection: &RustConnection) -> Result<Pictformat, OverlayError> {
    let formats = connection
        .render_query_pict_formats()
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;
    formats
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
        .map(|format| format.id)
        .ok_or_else(|| runtime_error("ARGB32 pictformat not found for spotlight cache"))
}

fn upload_picture_cache(
    connection: &RustConnection,
    window: Window,
    depth: u8,
    frame: &crate::overlay::SpotlightFrame,
) -> Result<(u32, Picture), OverlayError> {
    if depth != 32 {
        return Err(runtime_error(format!(
            "spotlight cache requires 32-bit depth, got {depth}"
        )));
    }

    let pictformat = argb32_pictformat(connection)?;
    let pixmap = connection.generate_id().map_err(runtime_error)?;
    if let Err(error) = connection.create_pixmap(depth, pixmap, window, frame.width, frame.height) {
        return Err(runtime_error(error));
    }

    let gc = match connection.generate_id() {
        Ok(id) => id,
        Err(error) => {
            let _ = connection.free_pixmap(pixmap);
            return Err(runtime_error(error));
        }
    };
    if let Err(error) =
        connection.create_gc(gc, pixmap, &CreateGCAux::default().graphics_exposures(0))
    {
        let _ = connection.free_pixmap(pixmap);
        return Err(runtime_error(error));
    }

    let mut image =
        match Image::allocate_native(frame.width, frame.height, depth, connection.setup()) {
            Ok(image) => image,
            Err(error) => {
                let _ = connection.free_gc(gc);
                let _ = connection.free_pixmap(pixmap);
                return Err(runtime_error(error));
            }
        };
    if image.data().len() != frame.pixels.len() {
        let _ = connection.free_gc(gc);
        let _ = connection.free_pixmap(pixmap);
        return Err(runtime_error(format!(
            "spotlight cache image size mismatch: image {} bytes, pixels {}",
            image.data().len(),
            frame.pixels.len()
        )));
    }
    image.data_mut().copy_from_slice(&frame.pixels);
    if let Err(error) = image.put(connection, pixmap, gc, 0, 0) {
        let _ = connection.free_gc(gc);
        let _ = connection.free_pixmap(pixmap);
        return Err(runtime_error(error));
    }
    let _ = connection.free_gc(gc);

    let picture = match connection.generate_id() {
        Ok(id) => id,
        Err(error) => {
            let _ = connection.free_pixmap(pixmap);
            return Err(runtime_error(error));
        }
    };
    if let Err(error) =
        connection.render_create_picture(picture, pixmap, pictformat, &CreatePictureAux::default())
    {
        let _ = connection.free_pixmap(pixmap);
        return Err(runtime_error(error));
    }

    Ok((pixmap, picture))
}

fn build_circle_cutout(radius_px: u32) -> (crate::overlay::SpotlightFrame, i16) {
    let extent = radius_px as u16 + 1;
    let side = extent * 2 + 1;
    let center = (extent as f32, extent as f32);
    let mut pixmap = Pixmap::new(u32::from(side), u32::from(side)).expect("valid cutout size");
    if let Some(geometry) = ShapeGeometry::new(rpresent_config::SpotShape::Circle, radius_px as f32)
        && let Some(path) = geometry.path_at(center)
    {
        let paint = SkiaPaint {
            shader: tiny_skia::Shader::SolidColor(SkiaColor::from_rgba8(255, 255, 255, 255)),
            anti_alias: true,
            ..SkiaPaint::default()
        };
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mut pixels = pixmap.data().to_vec();
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    (
        crate::overlay::SpotlightFrame {
            width: side,
            height: side,
            pixels,
        },
        extent as i16,
    )
}

fn ensure_hard_cutout_cache(
    connection: &RustConnection,
    window: Window,
    depth: u8,
    cache: &mut Option<HardCutoutCache>,
    radius_px: u32,
) -> Result<(Picture, i16, u16, u16), OverlayError> {
    if let Some(existing) = cache.as_ref()
        && existing.radius_px == radius_px
    {
        return Ok((
            existing.picture,
            existing.center_offset,
            existing.width,
            existing.height,
        ));
    }
    if let Some(old) = cache.take() {
        free_hard_cutout_cache(connection, old);
    }
    let (frame, center_offset) = build_circle_cutout(radius_px);
    let (pixmap, picture) = upload_picture_cache(connection, window, depth, &frame)?;
    let new_cache = HardCutoutCache {
        radius_px,
        pixmap,
        picture,
        center_offset,
        width: frame.width,
        height: frame.height,
    };
    let result = (
        new_cache.picture,
        new_cache.center_offset,
        new_cache.width,
        new_cache.height,
    );
    *cache = Some(new_cache);
    Ok(result)
}

fn ensure_decoration_cache(
    connection: &RustConnection,
    window: Window,
    depth: u8,
    cache: &mut Option<DecorationRenderCache>,
    config: &SpotlightConfig,
    radius_px: u32,
) -> Result<Option<(Picture, i16, u16, u16)>, OverlayError> {
    let Some(key) = decoration_cache_key(config, radius_px) else {
        if let Some(old) = cache.take() {
            free_decoration_cache(connection, old);
        }
        return Ok(None);
    };
    if let Some(existing) = cache.as_ref()
        && existing.key == key
    {
        return Ok(Some((
            existing.picture,
            existing.center_offset,
            existing.width,
            existing.height,
        )));
    }
    if let Some(old) = cache.take() {
        free_decoration_cache(connection, old);
    }
    let decoration_radius = if config.show_border { radius_px } else { 0 };
    let Some((frame, center_offset)) = build_circle_decorations(decoration_radius, config) else {
        return Ok(None);
    };
    let (pixmap, picture) = upload_picture_cache(connection, window, depth, &frame)?;
    let new_cache = DecorationRenderCache {
        key,
        pixmap,
        picture,
        center_offset,
        width: frame.width,
        height: frame.height,
    };
    let result = (
        new_cache.picture,
        new_cache.center_offset,
        new_cache.width,
        new_cache.height,
    );
    *cache = Some(new_cache);
    Ok(Some(result))
}

impl Drop for X11SpotlightOverlay {
    fn drop(&mut self) {
        let _ = self.connection.unmap_window(self.window);
        if let Some(cache) = self.decoration_cache.take() {
            free_decoration_cache(&self.connection, cache);
        }
        if let Some(cache) = self.hard_cutout_cache.take() {
            free_hard_cutout_cache(&self.connection, cache);
        }
        match self.backend {
            BackendKind::Render { gc, .. } | BackendKind::Shm { gc, .. } => {
                let _ = self.connection.free_gc(gc);
            }
        }
        self.shm.destroy(&self.connection);
        // Complete teardown before a replacement worker opens another X11 connection.
        if let Ok(cookie) = self.connection.get_input_focus() {
            let _ = cookie.reply();
        }
    }
}

impl SpotlightOverlayBackend for X11SpotlightOverlay {
    fn window_id(&self) -> u32 {
        self.window
    }

    fn show(&mut self) -> Result<(), OverlayError> {
        self.needs_full_redraw = true;
        self.connection
            .map_window(self.window)
            .inspect_err(|error| {
                tracing::error!(
                    window = self.window,
                    ?error,
                    "failed to show spotlight overlay"
                );
            })
            .map_err(runtime_error)?
            .check()
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
            .map_err(runtime_error)?
            .check()
            .map_err(runtime_error)?;
        self.flush()
    }

    fn surface_size(&self) -> Result<(u16, u16), OverlayError> {
        let root = self.connection.setup().roots[self.screen_num].root;
        let geometry = self
            .connection
            .get_geometry(root)
            .map_err(runtime_error)?
            .reply()
            .map_err(runtime_error)?;
        Ok((geometry.width, geometry.height))
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
        radius_px: u32,
    ) -> Result<(), OverlayError> {
        self.refresh_surface_size()?;
        let strategy = select_render_strategy(config);
        let (gc, depth, window_picture) = match &self.backend {
            BackendKind::Render {
                window_picture,
                gc,
                depth,
            } => (*gc, *depth, Some(*window_picture)),
            BackendKind::Shm { gc, depth } => (*gc, *depth, None),
        };

        let result = match (strategy, window_picture) {
            (RenderStrategy::Render, Some(window_picture)) => {
                // The XRender composite writes the whole window with SRC, so the next
                // SHM frame cannot reuse pixels from before this redraw.
                self.needs_full_redraw = true;
                redraw_render(
                    &self.connection,
                    self.window,
                    window_picture,
                    self.surface_size,
                    cursor,
                    config,
                    radius_px,
                    &mut self.decoration_cache,
                    &mut self.hard_cutout_cache,
                    depth,
                )
            }
            _ => self.redraw_shm(gc, depth, cursor, config, radius_px),
        };
        result.inspect_err(|error| {
            tracing::error!(
                window = self.window,
                ?error,
                "failed to redraw spotlight overlay"
            );
        })?;
        self.flush()
    }
}

/// Fail fast when the X server cannot support the spotlight backend.
///
/// The backend requires DAMAGE at startup even though the damage rectangle is computed
/// in this crate: a server without the extension is outside the supported configuration.
/// MIT-SHM 1.2 is the minimum version that provides `shm_create_segment`.
fn check_required_extensions(connection: &RustConnection) -> Result<(), OverlayError> {
    if connection
        .extension_information(x11rb::protocol::damage::X11_EXTENSION_NAME)
        .map_err(runtime_error)?
        .is_none()
    {
        return Err(runtime_error("the DAMAGE extension is unavailable"));
    }
    if connection
        .extension_information(x11rb::protocol::shm::X11_EXTENSION_NAME)
        .map_err(runtime_error)?
        .is_none()
    {
        return Err(runtime_error("the MIT-SHM extension is unavailable"));
    }
    if connection
        .extension_information(x11rb::protocol::shape::X11_EXTENSION_NAME)
        .map_err(runtime_error)?
        .is_none()
    {
        return Err(runtime_error("the SHAPE extension is unavailable"));
    }

    let version = connection
        .shm_query_version()
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;
    if version.major_version < 1 || (version.major_version == 1 && version.minor_version < 2) {
        return Err(runtime_error(format!(
            "MIT-SHM 1.2 is required, server reports {}.{}",
            version.major_version, version.minor_version
        )));
    }

    let shape_version = connection
        .shape_query_version()
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;
    if shape_version.major_version < 1
        || (shape_version.major_version == 1 && shape_version.minor_version < 1)
    {
        return Err(runtime_error(format!(
            "SHAPE 1.1 is required, server reports {}.{}",
            shape_version.major_version, shape_version.minor_version
        )));
    }

    Ok(())
}

/// The software renderer writes four bytes per pixel, so the chosen visual must use 32 bpp.
fn check_surface_format(connection: &RustConnection, depth: u8) -> Result<(), OverlayError> {
    let format = connection
        .setup()
        .pixmap_formats
        .iter()
        .find(|format| format.depth == depth)
        .ok_or_else(|| runtime_error(format!("no X11 pixmap format for depth {depth}")))?;
    if format.bits_per_pixel != 32 {
        return Err(runtime_error(format!(
            "spotlight requires a 32-bit-per-pixel visual, depth {depth} uses {}",
            format.bits_per_pixel
        )));
    }
    Ok(())
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
                        let gc = connection.generate_id().map_err(runtime_error)?;
                        connection
                            .create_gc(gc, window, &CreateGCAux::default().graphics_exposures(0))
                            .map_err(runtime_error)?;
                        tracing::debug!(window, pictformat, "using XRender spotlight backend");
                        Ok(BackendKind::Render {
                            window_picture: window_picture.into_picture(),
                            gc,
                            depth,
                        })
                    }
                    Err(error) => {
                        tracing::warn!(
                            window,
                            ?error,
                            "failed to create XRender spotlight picture; using the MIT-SHM backend"
                        );
                        create_shm_backend(connection, window, depth)
                    }
                },
                None => {
                    tracing::debug!(
                        window,
                        "XRender alpha format unavailable for spotlight visual; using the MIT-SHM backend"
                    );
                    create_shm_backend(connection, window, depth)
                }
            },
            Err(error) => {
                tracing::warn!(
                    window,
                    ?error,
                    "failed to query XRender formats; using the MIT-SHM backend"
                );
                create_shm_backend(connection, window, depth)
            }
        },
        None => create_shm_backend(connection, window, depth),
    }
}

fn create_shm_backend(
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

    Ok(BackendKind::Shm { gc, depth })
}

#[allow(clippy::too_many_arguments)]
fn redraw_render(
    connection: &RustConnection,
    window: Window,
    window_picture: Picture,
    surface_size: (u16, u16),
    cursor: CursorPosition,
    config: &SpotlightConfig,
    radius_px: u32,
    decoration_cache: &mut Option<DecorationRenderCache>,
    hard_cutout_cache: &mut Option<HardCutoutCache>,
    depth: u8,
) -> Result<(), OverlayError> {
    let full_rect = [Rectangle {
        x: 0,
        y: 0,
        width: surface_size.0,
        height: surface_size.1,
    }];

    if config.show_spot_shade {
        if config.softness_px == 0 {
            connection
                .render_fill_rectangles(
                    PictOp::SRC,
                    window_picture,
                    shade_fill_color(config),
                    &full_rect,
                )
                .map_err(runtime_error)?
                .check()
                .map_err(runtime_error)?;

            let (cutout_picture, center_offset, width, height) =
                ensure_hard_cutout_cache(connection, window, depth, hard_cutout_cache, radius_px)?;
            let dst_x = (i32::from(cursor.x) - i32::from(center_offset))
                .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let dst_y = (i32::from(cursor.y) - i32::from(center_offset))
                .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            render::composite(
                connection,
                PictOp::OUT_REVERSE,
                cutout_picture,
                x11rb::NONE,
                window_picture,
                0,
                0,
                0,
                0,
                dst_x,
                dst_y,
                width,
                height,
            )
            .inspect_err(|error| {
                tracing::error!(window, ?error, "failed to composite spotlight cutout");
            })
            .map_err(runtime_error)?;
        } else {
            if let Some(old) = hard_cutout_cache.take() {
                free_hard_cutout_cache(connection, old);
            }
            let gradient = create_gradient_picture(connection, cursor, config, radius_px)?;
            render::composite(
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
        }
    } else {
        if let Some(old) = hard_cutout_cache.take() {
            free_hard_cutout_cache(connection, old);
        }
        connection
            .render_fill_rectangles(
                PictOp::CLEAR,
                window_picture,
                Color {
                    red: 0,
                    green: 0,
                    blue: 0,
                    alpha: 0,
                },
                &full_rect,
            )
            .map_err(runtime_error)?
            .check()
            .map_err(runtime_error)?;
    }

    if let Some((decoration_picture, center_offset, width, height)) = ensure_decoration_cache(
        connection,
        window,
        depth,
        decoration_cache,
        config,
        radius_px,
    )? {
        let dst_x = (i32::from(cursor.x) - i32::from(center_offset))
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let dst_y = (i32::from(cursor.y) - i32::from(center_offset))
            .clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        render::composite(
            connection,
            PictOp::OVER,
            decoration_picture,
            x11rb::NONE,
            window_picture,
            0,
            0,
            0,
            0,
            dst_x,
            dst_y,
            width,
            height,
        )
        .inspect_err(|error| {
            tracing::error!(window, ?error, "failed to composite spotlight decorations");
        })
        .map_err(runtime_error)?;
    }

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
    fn default_circle_uses_the_xrender_path() {
        let config = rpresent_config::SpotlightConfig::default();
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::Render
        ));
    }

    #[test]
    fn non_circle_shape_uses_the_shm_path() {
        let config = rpresent_config::SpotlightConfig {
            shape: rpresent_config::SpotShape::Square {
                corner_radius_pct: 0,
            },
            ..rpresent_config::SpotlightConfig::default()
        };
        assert!(matches!(
            select_render_strategy(&config),
            RenderStrategy::Shm
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
    fn decoration_cache_key_covers_border_and_dot_settings() {
        let base = rpresent_config::SpotlightConfig::default();
        let base_key = decoration_cache_key(&base, 40).unwrap();

        for config in [
            rpresent_config::SpotlightConfig {
                show_border: false,
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                border_size_pct: 20,
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                border_color: "#ff0000".to_owned(),
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                border_opacity: 0.4,
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                show_center_dot: false,
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                dot_size_px: 12,
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                dot_color: "#00ff00".to_owned(),
                ..base.clone()
            },
            rpresent_config::SpotlightConfig {
                dot_opacity: 0.4,
                ..base.clone()
            },
        ] {
            assert_ne!(decoration_cache_key(&config, 40), Some(base_key));
        }

        let dot_only = rpresent_config::SpotlightConfig {
            show_border: false,
            ..base.clone()
        };
        assert_eq!(
            decoration_cache_key(&dot_only, 40),
            decoration_cache_key(&dot_only, 80)
        );
        assert_ne!(
            decoration_cache_key(&base, 40),
            decoration_cache_key(&base, 80)
        );

        let hidden = rpresent_config::SpotlightConfig {
            show_border: false,
            show_center_dot: false,
            ..base
        };
        assert_eq!(decoration_cache_key(&hidden, 40), None);
    }

    #[test]
    fn shade_fill_color_is_premultiplied_and_uses_the_shared_rgb_parser() {
        let config = rpresent_config::SpotlightConfig {
            shade_color: "#804020".to_owned(),
            shade_opacity: 0.5,
            ..Default::default()
        };
        let color = shade_fill_color(&config);
        assert_eq!(color.red, 16_448);
        assert_eq!(color.green, 8_224);
        assert_eq!(color.blue, 4_112);
        assert_eq!(color.alpha, 32_768);
    }

    #[test]
    fn hard_cutout_geometry_changes_with_radius() {
        let (small, small_offset) = build_circle_cutout(40);
        let (large, large_offset) = build_circle_cutout(80);

        assert!(large.width > small.width);
        assert_eq!(small.width, small.height);
        assert_eq!(large.width, large.height);
        assert_eq!(small_offset, (small.width / 2) as i16);
        assert_eq!(large_offset, (large.width / 2) as i16);
        assert_ne!(small.pixels, large.pixels);
    }

    #[test]
    fn hard_cutout_geometry_is_independent_of_cursor_position() {
        let (first, first_offset) = build_circle_cutout(40);
        let (second, second_offset) = build_circle_cutout(40);

        assert_eq!(first, second);
        assert_eq!(first_offset, second_offset);
    }

    #[test]
    #[ignore = "requires an X server with RENDER and an ARGB32 format"]
    fn xrender_circle_matches_software_layers() {
        assert_circle_render_parity();
    }

    fn assert_circle_render_parity() {
        let (connection, screen_num) = x11rb::connect(None).expect("connect to the X server");
        let screen = &connection.setup().roots[screen_num];
        let surface_size = (128_u16, 96_u16);
        let target_pixmap = connection.generate_id().expect("allocate target pixmap id");
        connection
            .create_pixmap(
                32,
                target_pixmap,
                screen.root,
                surface_size.0,
                surface_size.1,
            )
            .expect("create ARGB32 target pixmap")
            .check()
            .expect("create target pixmap request");
        let target_picture = connection
            .generate_id()
            .expect("allocate target picture id");
        connection
            .render_create_picture(
                target_picture,
                target_pixmap,
                argb32_pictformat(&connection).expect("find ARGB32 picture format"),
                &CreatePictureAux::default(),
            )
            .expect("create target picture")
            .check()
            .expect("create target picture request");

        let mut decoration_cache = None;
        let mut hard_cutout_cache = None;
        let center = CursorPosition { x: 64, y: 48 };
        let soft_shade = SpotlightConfig {
            shade_color: "#804020".to_owned(),
            shade_opacity: 0.4,
            softness_px: 20,
            show_border: false,
            show_center_dot: false,
            ..Default::default()
        };

        let cases = [
            ("colored soft shade", soft_shade.clone(), center, 40),
            (
                "hard shade",
                SpotlightConfig {
                    softness_px: 0,
                    show_border: false,
                    show_center_dot: false,
                    ..soft_shade.clone()
                },
                center,
                40,
            ),
            (
                "border only",
                SpotlightConfig {
                    show_spot_shade: false,
                    show_center_dot: false,
                    show_border: true,
                    border_color: "#20a0e0".to_owned(),
                    border_size_pct: 15,
                    border_opacity: 0.7,
                    ..Default::default()
                },
                center,
                40,
            ),
            (
                "dot only",
                SpotlightConfig {
                    show_spot_shade: false,
                    show_border: false,
                    show_center_dot: true,
                    dot_size_px: 12,
                    dot_color: "#e02070".to_owned(),
                    dot_opacity: 0.8,
                    ..Default::default()
                },
                center,
                40,
            ),
            (
                "all disabled",
                SpotlightConfig {
                    show_spot_shade: false,
                    show_border: false,
                    show_center_dot: false,
                    ..Default::default()
                },
                center,
                40,
            ),
            (
                "all enabled",
                SpotlightConfig {
                    shade_color: "#102040".to_owned(),
                    shade_opacity: 0.45,
                    softness_px: 12,
                    border_color: "#40d080".to_owned(),
                    border_opacity: 0.65,
                    dot_color: "#e0a020".to_owned(),
                    dot_opacity: 0.85,
                    dot_size_px: 10,
                    ..Default::default()
                },
                center,
                40,
            ),
            (
                "large dot",
                SpotlightConfig {
                    show_spot_shade: false,
                    show_border: false,
                    show_center_dot: true,
                    dot_size_px: 100,
                    dot_color: "#8030c0".to_owned(),
                    dot_opacity: 0.5,
                    ..Default::default()
                },
                center,
                40,
            ),
            ("radius 40", soft_shade.clone(), center, 40),
            ("radius 80", soft_shade.clone(), center, 80),
            ("radius 30", soft_shade.clone(), center, 30),
            (
                "cursor moved",
                soft_shade.clone(),
                CursorPosition { x: 35, y: 25 },
                40,
            ),
            (
                "edge position",
                soft_shade.clone(),
                CursorPosition { x: 2, y: 3 },
                40,
            ),
        ];

        for (name, config, cursor, radius_px) in cases {
            redraw_render(
                &connection,
                screen.root,
                target_picture,
                surface_size,
                cursor,
                &config,
                radius_px,
                &mut decoration_cache,
                &mut hard_cutout_cache,
                32,
            )
            .unwrap_or_else(|error| panic!("{name}: XRender redraw failed: {error}"));
            connection.flush().expect("flush XRender commands");

            let image = Image::get(
                &connection,
                target_pixmap,
                0,
                0,
                surface_size.0,
                surface_size.1,
            )
            .unwrap_or_else(|error| panic!("{name}: read target pixmap: {error}"))
            .0;
            let expected =
                crate::render::build_projecteur_frame(surface_size, cursor, &config, radius_px);
            for y in 0..surface_size.1 {
                for x in 0..surface_size.0 {
                    let actual_pixel = image.get_pixel(x, y);
                    let actual = [
                        actual_pixel as u8,
                        (actual_pixel >> 8) as u8,
                        (actual_pixel >> 16) as u8,
                        (actual_pixel >> 24) as u8,
                    ];
                    let expected = expected.bgra_at(x, y);
                    let distance = (f32::from(x) - f32::from(cursor.x))
                        .hypot(f32::from(y) - f32::from(cursor.y));
                    let mut boundaries = vec![radius_px as f32];
                    if config.show_spot_shade && config.softness_px > 0 {
                        boundaries.push(radius_px as f32 + config.softness_px as f32);
                    }
                    if config.show_border {
                        boundaries.push(
                            radius_px as f32
                                * (1.0 - config.border_size_pct.min(99) as f32 / 100.0),
                        );
                    }
                    if config.show_center_dot {
                        boundaries.push(config.dot_size_px as f32 / 2.0);
                    }
                    // XRender evaluates radial gradients in fixed-point pixel space, so its
                    // sample can differ slightly from the integer-coordinate software sample.
                    let tolerance = if boundaries
                        .iter()
                        .any(|boundary| (distance - boundary).abs() <= 1.5)
                    {
                        10
                    } else {
                        8
                    };
                    for channel in 0..4 {
                        let difference = actual[channel].abs_diff(expected[channel]);
                        assert!(
                            difference <= tolerance,
                            "{name}: pixel ({x}, {y}) channel {channel} differs: actual={actual:?}, expected={expected:?}"
                        );
                    }
                }
            }
        }

        let cache_config = SpotlightConfig {
            show_spot_shade: false,
            show_center_dot: true,
            dot_size_px: 10,
            ..Default::default()
        };
        redraw_render(
            &connection,
            screen.root,
            target_picture,
            surface_size,
            center,
            &cache_config,
            40,
            &mut decoration_cache,
            &mut hard_cutout_cache,
            32,
        )
        .expect("render cache reuse setup");
        connection.flush().expect("flush cache setup");
        let first_picture = decoration_cache.as_ref().map(|cache| cache.picture);
        redraw_render(
            &connection,
            screen.root,
            target_picture,
            surface_size,
            CursorPosition { x: 20, y: 20 },
            &cache_config,
            40,
            &mut decoration_cache,
            &mut hard_cutout_cache,
            32,
        )
        .expect("render cursor-only cache reuse");
        assert_eq!(
            first_picture,
            decoration_cache.as_ref().map(|cache| cache.picture)
        );

        if let Some(cache) = decoration_cache.take() {
            free_decoration_cache(&connection, cache);
        }
        if let Some(cache) = hard_cutout_cache.take() {
            free_hard_cutout_cache(&connection, cache);
        }
        connection
            .render_free_picture(target_picture)
            .expect("free target picture")
            .check()
            .expect("free target picture request");
        connection
            .free_pixmap(target_pixmap)
            .expect("free target pixmap")
            .check()
            .expect("free target pixmap request");
    }
}
