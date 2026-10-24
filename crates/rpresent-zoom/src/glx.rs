use std::ffi::CString;
use std::os::raw::{c_int, c_uchar, c_ulong, c_void};
use std::ptr;

use glow::HasContext;
use glow::PixelUnpackData;
use x11::glx;
use x11::xlib;

use crate::OverlayError;
use crate::capture::CapturedFrame;

fn runtime_error(error: impl std::fmt::Display) -> OverlayError {
    OverlayError::Runtime(error.to_string())
}

fn load_gl_proc(symbol: &str) -> *const c_void {
    let Ok(symbol) = CString::new(symbol) else {
        return ptr::null();
    };

    unsafe {
        glx::glXGetProcAddressARB(symbol.as_ptr() as *const c_uchar)
            .map_or(ptr::null(), |proc| proc as *const () as *const c_void)
    }
}

fn bytes_of<T>(slice: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

fn compile_shader(
    gl: &glow::Context,
    kind: u32,
    source: &str,
) -> Result<glow::Shader, OverlayError> {
    unsafe {
        let shader = gl.create_shader(kind).map_err(runtime_error)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);

        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(runtime_error(format!(
                "GL shader compilation failed: {log}"
            )));
        }

        Ok(shader)
    }
}

fn create_program(
    gl: &glow::Context,
    vertex_source: &str,
    fragment_source: &str,
) -> Result<glow::Program, OverlayError> {
    unsafe {
        let program = gl.create_program().map_err(runtime_error)?;
        let vertex_shader = match compile_shader(gl, glow::VERTEX_SHADER, vertex_source) {
            Ok(shader) => shader,
            Err(error) => {
                gl.delete_program(program);
                return Err(error);
            }
        };
        let fragment_shader = match compile_shader(gl, glow::FRAGMENT_SHADER, fragment_source) {
            Ok(shader) => shader,
            Err(error) => {
                gl.delete_shader(vertex_shader);
                gl.delete_program(program);
                return Err(error);
            }
        };

        gl.attach_shader(program, vertex_shader);
        gl.attach_shader(program, fragment_shader);
        gl.link_program(program);

        gl.detach_shader(program, vertex_shader);
        gl.detach_shader(program, fragment_shader);
        gl.delete_shader(vertex_shader);
        gl.delete_shader(fragment_shader);

        if !gl.get_program_link_status(program) {
            let log = gl.get_program_info_log(program);
            gl.delete_program(program);
            return Err(runtime_error(format!("GL program link failed: {log}")));
        }

        Ok(program)
    }
}

fn choose_fbconfig(
    display: *mut xlib::Display,
    screen: c_int,
) -> Result<glx::GLXFBConfig, OverlayError> {
    unsafe {
        let mut count: c_int = 0;
        let attribs = [
            glx::GLX_X_RENDERABLE as c_int,
            1,
            glx::GLX_DRAWABLE_TYPE as c_int,
            glx::GLX_WINDOW_BIT as c_int,
            glx::GLX_RENDER_TYPE as c_int,
            glx::GLX_RGBA_BIT as c_int,
            glx::GLX_DOUBLEBUFFER as c_int,
            1,
            glx::GLX_RED_SIZE as c_int,
            8,
            glx::GLX_GREEN_SIZE as c_int,
            8,
            glx::GLX_BLUE_SIZE as c_int,
            8,
            glx::GLX_ALPHA_SIZE as c_int,
            0,
            0,
        ];

        let configs = glx::glXChooseFBConfig(display, screen, attribs.as_ptr(), &mut count);
        if configs.is_null() || count <= 0 {
            return Err(runtime_error(
                "failed to choose an opaque GLX framebuffer configuration",
            ));
        }

        let chosen = *configs;
        let _ = xlib::XFree(configs as *mut c_void);

        Ok(chosen)
    }
}

pub(crate) struct GlxRenderer {
    display: *mut xlib::Display,
    context: glx::GLXContext,
    gl: Option<glow::Context>,
    program: Option<glow::Program>,
    vao: Option<glow::VertexArray>,
    vbo: Option<glow::Buffer>,
    texture: Option<glow::Texture>,
    u_screen_size: Option<glow::UniformLocation>,
    u_frame_size: Option<glow::UniformLocation>,
    u_translate: Option<glow::UniformLocation>,
    u_scale: Option<glow::UniformLocation>,
    u_texture: Option<glow::UniformLocation>,
    frame_size: (f32, f32),
    window: xlib::Window,
    colormap: xlib::Colormap,
    depth: u8,
    visual_id: u32,
}

impl GlxRenderer {
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn new(screen: i32) -> Result<Self, OverlayError> {
        let display = xlib::XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err(runtime_error("failed to open X11 display for GLX renderer"));
        }

        let result = (|| -> Result<Self, OverlayError> {
            let mut major_version = 0;
            let mut minor_version = 0;
            if glx::glXQueryVersion(display, &mut major_version, &mut minor_version) == 0 {
                return Err(runtime_error("failed to query GLX version"));
            }
            if major_version < 1 || (major_version == 1 && minor_version < 3) {
                return Err(runtime_error(format!(
                    "GLX 1.3 or newer is required, found {major_version}.{minor_version}"
                )));
            }

            let fb_config = choose_fbconfig(display, screen)?;
            let visual_info = glx::glXGetVisualFromFBConfig(display, fb_config);
            if visual_info.is_null() {
                return Err(runtime_error("failed to query GLX visual id"));
            }
            let visual_id = (*visual_info).visualid;
            let depth = (*visual_info).depth;
            let _ = xlib::XFree(visual_info as *mut c_void);

            let context = glx::glXCreateNewContext(
                display,
                fb_config,
                glx::GLX_RGBA_TYPE,
                ptr::null_mut(),
                1,
            );
            if context.is_null() {
                return Err(runtime_error("failed to create GLX context"));
            }

            Ok(Self {
                display,
                context,
                gl: None,
                program: None,
                vao: None,
                vbo: None,
                texture: None,
                u_screen_size: None,
                u_frame_size: None,
                u_translate: None,
                u_scale: None,
                u_texture: None,
                frame_size: (1.0, 1.0),
                window: 0,
                colormap: 0,
                depth: depth as u8,
                visual_id: visual_id as u32,
            })
        })();

        if result.is_err() {
            let _ = xlib::XCloseDisplay(display);
        }

        result
    }

    /// Returns the depth and visual id of the opaque framebuffer configuration.
    #[allow(dead_code)]
    pub fn visual(&self) -> (u8, u32) {
        (self.depth, self.visual_id)
    }

    /// Creates the drawable with the same Xlib connection that owns GLX.
    /// X11 resource IDs are server-wide, but GLX still validates the drawable
    /// against the connection that created the context, so creation must stay
    /// on this connection.
    pub fn create_window(
        &mut self,
        root: xlib::Window,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        background: u32,
    ) -> Result<u32, OverlayError> {
        unsafe {
            let mut template: xlib::XVisualInfo = std::mem::zeroed();
            template.visualid = self.visual_id as xlib::VisualID;
            let mut count = 0;
            let infos =
                xlib::XGetVisualInfo(self.display, xlib::VisualIDMask, &mut template, &mut count);
            if infos.is_null() || count <= 0 {
                return Err(runtime_error("failed to resolve the GLX visual"));
            }
            let visual = (*infos).visual;
            let colormap = xlib::XCreateColormap(self.display, root, visual, xlib::AllocNone);
            let mut attributes: xlib::XSetWindowAttributes = std::mem::zeroed();
            attributes.background_pixel = background as c_ulong;
            attributes.border_pixel = background as c_ulong;
            attributes.colormap = colormap;
            attributes.override_redirect = 1;
            let mask = xlib::CWBackPixel
                | xlib::CWBorderPixel
                | xlib::CWColormap
                | xlib::CWOverrideRedirect;
            let window = xlib::XCreateWindow(
                self.display,
                root,
                x,
                y,
                width,
                height,
                0,
                self.depth as c_int,
                xlib::InputOutput as u32,
                visual,
                mask,
                &mut attributes,
            );
            let _ = xlib::XFree(infos as *mut c_void);
            if window == 0 {
                let _ = xlib::XFreeColormap(self.display, colormap);
                return Err(runtime_error("failed to create the GLX window"));
            }
            self.window = window;
            self.colormap = colormap;
            let _ = xlib::XSync(self.display, 0);
            Ok(window as u32)
        }
    }

    pub fn map_window(&self) {
        unsafe {
            let _ = xlib::XMapWindow(self.display, self.window);
            let _ = xlib::XSync(self.display, 0);
        }
    }

    /// Binds the renderer to the matching X11 window and prepares its GL context.
    pub fn attach(&mut self, window: u32) -> Result<(), OverlayError> {
        unsafe {
            if glx::glXMakeCurrent(self.display, window as c_ulong, self.context) == 0 {
                return Err(runtime_error("failed to make GLX context current"));
            }
        }

        let gl = unsafe { glow::Context::from_loader_function(load_gl_proc) };
        let version = gl.version();
        if version.major < 3 || (version.major == 3 && version.minor < 3) {
            return Err(runtime_error(format!(
                "OpenGL 3.3 with GLSL 330 is required, found {version:?}"
            )));
        }

        let program = create_program(&gl, VERTEX_SHADER, FRAGMENT_SHADER)?;
        let vao = unsafe { gl.create_vertex_array().map_err(runtime_error)? };
        let vbo = unsafe { gl.create_buffer().map_err(runtime_error)? };
        let texture = unsafe { gl.create_texture().map_err(runtime_error)? };
        let u_screen_size = unsafe {
            gl.get_uniform_location(program, "u_screen_size")
                .ok_or_else(|| runtime_error("missing u_screen_size uniform"))?
        };
        let u_frame_size = unsafe {
            gl.get_uniform_location(program, "u_frame_size")
                .ok_or_else(|| runtime_error("missing u_frame_size uniform"))?
        };
        let u_translate = unsafe {
            gl.get_uniform_location(program, "u_translate")
                .ok_or_else(|| runtime_error("missing u_translate uniform"))?
        };
        let u_scale = unsafe {
            gl.get_uniform_location(program, "u_scale")
                .ok_or_else(|| runtime_error("missing u_scale uniform"))?
        };
        let u_texture = unsafe {
            gl.get_uniform_location(program, "u_texture")
                .ok_or_else(|| runtime_error("missing u_texture uniform"))?
        };

        unsafe {
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));

            let vertices: [f32; 24] = [
                0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                1.0, 1.0, 1.0, 1.0, 0.0, 1.0, 0.0, 1.0,
            ];

            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(&vertices), glow::STATIC_DRAW);

            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 16, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 2, glow::FLOAT, false, 16, 8);

            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );

            gl.use_program(Some(program));
            gl.uniform_1_i32(Some(&u_texture), 0);
            gl.use_program(None);

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.bind_vertex_array(None);
        }

        self.gl = Some(gl);
        self.program = Some(program);
        self.vao = Some(vao);
        self.vbo = Some(vbo);
        self.texture = Some(texture);
        self.u_screen_size = Some(u_screen_size);
        self.u_frame_size = Some(u_frame_size);
        self.u_translate = Some(u_translate);
        self.u_scale = Some(u_scale);
        self.u_texture = Some(u_texture);
        self.window = window as xlib::Window;

        self.set_swap_interval(0);

        Ok(())
    }

    pub fn upload_frame(&mut self, frame: &CapturedFrame) -> Result<(), OverlayError> {
        self.frame_size = (frame.width as f32, frame.height as f32);

        let Some(gl) = self.gl.as_ref() else {
            return Err(runtime_error("GLX renderer has no OpenGL context attached"));
        };
        let Some(texture) = self.texture else {
            return Err(runtime_error("GLX renderer has no texture attached"));
        };

        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                frame.width as i32,
                frame.height as i32,
                0,
                glow::BGRA,
                glow::UNSIGNED_BYTE,
                PixelUnpackData::Slice(Some(&frame.bgra)),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
        }

        Ok(())
    }

    pub fn draw(
        &mut self,
        scale: f64,
        translation: (f64, f64),
        viewport: (u32, u32),
    ) -> Result<(), OverlayError> {
        let Some(gl) = self.gl.as_ref() else {
            return Err(runtime_error("GLX renderer has no OpenGL context attached"));
        };
        let Some(program) = self.program else {
            return Err(runtime_error("GLX renderer has no shader program attached"));
        };
        let Some(vao) = self.vao else {
            return Err(runtime_error("GLX renderer has no vertex array attached"));
        };
        let Some(texture) = self.texture else {
            return Err(runtime_error("GLX renderer has no texture attached"));
        };
        let u_screen_size = self
            .u_screen_size
            .as_ref()
            .ok_or_else(|| runtime_error("GLX renderer has no u_screen_size uniform"))?;
        let u_frame_size = self
            .u_frame_size
            .as_ref()
            .ok_or_else(|| runtime_error("GLX renderer has no u_frame_size uniform"))?;
        let u_translate = self
            .u_translate
            .as_ref()
            .ok_or_else(|| runtime_error("GLX renderer has no u_translate uniform"))?;
        let u_scale = self
            .u_scale
            .as_ref()
            .ok_or_else(|| runtime_error("GLX renderer has no u_scale uniform"))?;

        unsafe {
            gl.viewport(0, 0, viewport.0 as i32, viewport.1 as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);

            gl.use_program(Some(program));
            gl.bind_vertex_array(Some(vao));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));

            gl.uniform_2_f32(Some(u_screen_size), viewport.0 as f32, viewport.1 as f32);
            gl.uniform_2_f32(Some(u_frame_size), self.frame_size.0, self.frame_size.1);
            gl.uniform_2_f32(
                Some(u_translate),
                translation.0 as f32,
                translation.1 as f32,
            );
            gl.uniform_1_f32(Some(u_scale), scale as f32);

            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_vertex_array(None);
            gl.use_program(None);

            glx::glXSwapBuffers(self.display, self.window);
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn wait_complete(&self) {
        let Some(gl) = self.gl.as_ref() else {
            return;
        };
        unsafe {
            gl.finish();
        }
    }

    fn set_swap_interval(&self, interval: i32) {
        unsafe {
            let symbol = load_gl_proc("glXSwapInterval");
            if symbol.is_null() {
                return;
            }
            let swap_interval: unsafe extern "C" fn(*mut xlib::Display, c_int) -> () =
                std::mem::transmute(symbol);
            swap_interval(self.display, interval);
        }
    }
}

impl Drop for GlxRenderer {
    fn drop(&mut self) {
        unsafe {
            if let Some(gl) = self.gl.as_ref() {
                if let Some(program) = self.program {
                    gl.delete_program(program);
                }
                if let Some(vao) = self.vao {
                    gl.delete_vertex_array(vao);
                }
                if let Some(vbo) = self.vbo {
                    gl.delete_buffer(vbo);
                }
                if let Some(texture) = self.texture {
                    gl.delete_texture(texture);
                }
            }
            let _ = glx::glXMakeCurrent(self.display, 0, ptr::null_mut());
            if self.window != 0 {
                let _ = xlib::XDestroyWindow(self.display, self.window);
            }
            if self.colormap != 0 {
                let _ = xlib::XFreeColormap(self.display, self.colormap);
            }
            glx::glXDestroyContext(self.display, self.context);
            let _ = xlib::XCloseDisplay(self.display);
        }
    }
}

const VERTEX_SHADER: &str = r#"#version 330 core
layout (location = 0) in vec2 a_pos;
layout (location = 1) in vec2 a_uv;

uniform vec2 u_screen_size;
uniform vec2 u_frame_size;
uniform vec2 u_translate;
uniform float u_scale;

out vec2 v_uv;

void main() {
    vec2 pixel_pos = a_pos * u_frame_size * u_scale + u_translate;
    vec2 ndc = vec2(
        (pixel_pos.x / u_screen_size.x) * 2.0 - 1.0,
        1.0 - (pixel_pos.y / u_screen_size.y) * 2.0
    );
    gl_Position = vec4(ndc, 0.0, 1.0);
    v_uv = a_pos;
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 330 core
in vec2 v_uv;
uniform sampler2D u_texture;
out vec4 color;

void main() {
    color = texture(u_texture, v_uv);
}
"#;
