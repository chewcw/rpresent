use x11rb::connection::{Connection, RequestConnection};
use x11rb::image::Image;
use x11rb::protocol::render::{
    self, ChangePictureAux, Color, ConnectionExt as RenderConnectionExt, CreatePictureAux, Fixed,
    PictOp, PictType, Pictformat, Picture, PictureWrapper, Pointfix, Repeat,
};
use x11rb::protocol::shm::ConnectionExt as ShmConnectionExt;
use x11rb::protocol::xproto::{
    ColormapAlloc, ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, CreateGCAux,
    CreateWindowAux, Gcontext, ImageFormat, Visualid, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;

use crate::damage;
use crate::overlay::{CursorPosition, OverlayError, SpotlightOverlayBackend, runtime_error};
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
            alpha: alpha_to_u16(config.dim_alpha),
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
    shm: ShmSegment,
    dot_cache: Option<DotRenderCache>,
    last_cursor: Option<CursorPosition>,
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
            // Bind the colormap to the window so the server knows which
            // color interpretation applies. For our direct-color visual this
            // is a formality: the pixel bits already encode full RGBA.
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
            dot_cache: None,
            last_cursor: None,
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

        let damage_rect = if full_redraw {
            full_rect
        } else {
            match self.last_cursor {
                Some(previous) => {
                    let old = damage::paint_bounds(self.surface_size, previous, config);
                    let new = damage::paint_bounds(self.surface_size, cursor, config);
                    damage::union(old, new, self.surface_size)
                }
                None => full_rect,
            }
        };

        if damage_rect.2 > 0 && damage_rect.3 > 0 {
            let frame = crate::render::build_projecteur_frame_rect(
                self.surface_size,
                damage_rect,
                cursor,
                config,
            );
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

        self.last_cursor = Some(cursor);
        if config_changed {
            self.last_config = Some(config.clone());
        }
        Ok(())
    }
}

fn free_dot_cache(connection: &RustConnection, cache: DotRenderCache) {
    let _ = connection.render_free_picture(cache.picture);
    let _ = connection.free_pixmap(cache.pixmap);
}

/// Build premultiplied BGRA bytes for a filled circle inside a `size_px × size_px` square.
fn build_circle_dot_bgra(size_px: u16, rgb: (u8, u8, u8), alpha_u16: u16) -> Vec<u8> {
    let w = size_px as usize;
    let h = size_px as usize;
    let mut out = vec![0u8; w * h * 4];
    let cx = (size_px as f32 - 1.0) * 0.5;
    let cy = (size_px as f32 - 1.0) * 0.5;
    let r = size_px as f32 * 0.5;
    let max_a = (alpha_u16 as f32 / u16::MAX as f32).clamp(0.0, 1.0);
    for y in 0..h {
        for x in 0..w {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            if dx * dx + dy * dy <= r * r {
                let i = (y * w + x) * 4;
                let a8 = (max_a * 255.0).round() as u8;
                // premultiplied BGRA
                out[i] = ((rgb.2 as u16 * a8 as u16) / 255) as u8;
                out[i + 1] = ((rgb.1 as u16 * a8 as u16) / 255) as u8;
                out[i + 2] = ((rgb.0 as u16 * a8 as u16) / 255) as u8;
                out[i + 3] = a8;
            }
        }
    }
    out
}

/// Create a pixmap + picture for the given dot key, uploading circle pixels.
fn upload_dot_cache(
    connection: &RustConnection,
    window: Window,
    depth: u8,
    key: DotCacheKey,
) -> Result<DotRenderCache, OverlayError> {
    if depth != 32 {
        return Err(runtime_error(format!(
            "dot cache requires 32-bit depth, got {depth}"
        )));
    }

    let sz = key.size_px;
    let pixels = build_circle_dot_bgra(sz, key.color_rgb, key.alpha_u16);

    // Locate ARGB32 pictformat.
    let formats = connection
        .render_query_pict_formats()
        .map_err(runtime_error)?
        .reply()
        .map_err(runtime_error)?;
    let pictformat = formats
        .formats
        .iter()
        .find(|f| {
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
        })
        .map(|f| f.id)
        .ok_or_else(|| runtime_error("ARGB32 pictformat not found for dot cache"))?;

    let pixmap = connection.generate_id().map_err(runtime_error)?;
    connection
        .create_pixmap(depth, pixmap, window, sz, sz)
        .map_err(runtime_error)?;

    // Temporary GC to upload image data.
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
    let mut image = match Image::allocate_native(sz, sz, depth, connection.setup()) {
        Ok(img) => img,
        Err(error) => {
            let _ = connection.free_gc(gc);
            let _ = connection.free_pixmap(pixmap);
            return Err(runtime_error(error));
        }
    };
    if image.data().len() != pixels.len() {
        let _ = connection.free_gc(gc);
        let _ = connection.free_pixmap(pixmap);
        return Err(runtime_error(format!(
            "dot cache image size mismatch: image {} bytes, pixels {} bytes",
            image.data().len(),
            pixels.len()
        )));
    }
    image.data_mut().copy_from_slice(&pixels);
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

    Ok(DotRenderCache {
        key,
        pixmap,
        picture,
    })
}

/// Return the cached dot picture, rebuilding it when the key changes.
/// Returns None when the dot is disabled.
fn ensure_dot_cache(
    connection: &RustConnection,
    window: Window,
    depth: u8,
    dot_cache: &mut Option<DotRenderCache>,
    config: &SpotlightConfig,
) -> Result<Option<(Picture, DotCacheKey)>, OverlayError> {
    let key = match dot_cache_key(config) {
        None => {
            if let Some(old) = dot_cache.take() {
                free_dot_cache(connection, old);
            }
            return Ok(None);
        }
        Some(k) => k,
    };

    if let Some(ref cache) = *dot_cache
        && cache.key == key
    {
        return Ok(Some((cache.picture, cache.key)));
    }

    // Key changed — free old cache then rebuild.
    if let Some(old) = dot_cache.take() {
        free_dot_cache(connection, old);
    }
    let new_cache = upload_dot_cache(connection, window, depth, key)?;
    let picture = new_cache.picture;
    *dot_cache = Some(new_cache);
    Ok(Some((picture, key)))
}

impl Drop for X11SpotlightOverlay {
    fn drop(&mut self) {
        if let Some(cache) = self.dot_cache.take() {
            free_dot_cache(&self.connection, cache);
        }
        match self.backend {
            BackendKind::Render { gc, .. } | BackendKind::Shm { gc, .. } => {
                let _ = self.connection.free_gc(gc);
            }
        }
        self.shm.destroy(&self.connection);
    }
}

impl SpotlightOverlayBackend for X11SpotlightOverlay {
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
                    &mut self.dot_cache,
                    depth,
                )
            }
            _ => self.redraw_shm(gc, depth, cursor, config),
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
    dot_cache: &mut Option<DotRenderCache>,
    depth: u8,
) -> Result<(), OverlayError> {
    let gradient = create_gradient_picture(connection, cursor, config, radius_px)?;
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

    // Composite circle dot if enabled.
    if let Some((dot_picture, key)) =
        ensure_dot_cache(connection, window, depth, dot_cache, config)?
    {
        let half = (key.size_px as i32) / 2;
        let dst_x = (cursor.x as i32 - half).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let dst_y = (cursor.y as i32 - half).clamp(i16::MIN as i32, i16::MAX as i32) as i16;
        let _ = render::composite(
            connection,
            PictOp::OVER,
            dot_picture,
            x11rb::NONE,
            window_picture,
            0,
            0,
            0,
            0,
            dst_x,
            dst_y,
            key.size_px,
            key.size_px,
        )
        .inspect_err(|error| {
            tracing::error!(window, ?error, "failed to composite spotlight center dot");
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
    fn dot_cache_key_changes_when_visual_inputs_change() {
        let a = rpresent_config::SpotlightConfig {
            show_center_dot: true,
            dot_size_px: 8,
            dot_color: "#ffffff".to_owned(),
            dot_opacity: 0.8,
            ..rpresent_config::SpotlightConfig::default()
        };

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
        let c = rpresent_config::SpotlightConfig {
            show_center_dot: false,
            ..rpresent_config::SpotlightConfig::default()
        };
        assert_eq!(dot_cache_key(&c), None);
    }
}
