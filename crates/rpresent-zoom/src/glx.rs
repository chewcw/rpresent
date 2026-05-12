use std::ffi::CString;
use std::os::raw::{c_int, c_uchar, c_ulong, c_void};
use std::ptr;

use glow::HasContext;
use glow::PixelUnpackData;
use x11::glx;
use x11::xlib;

use crate::capture::CapturedFrame;
use crate::overlay::{OverlayError, ScreenshotPlacement};

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
    unsafe {
        std::slice::from_raw_parts(
            slice.as_ptr() as *const u8,
            std::mem::size_of_val(slice),
        )
    }
}

fn compile_shader(gl: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader, OverlayError> {
    unsafe {
        let shader = gl.create_shader(kind).map_err(runtime_error)?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);

        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            return Err(runtime_error(format!("GL shader compilation failed: {log}")));
        }

        Ok(shader)
    }
}

fn create_program(gl: &glow::Context, vertex_source: &str, fragment_source: &str) -> Result<glow::Program, OverlayError> {
    unsafe {
        let program = gl.create_program().map_err(runtime_error)?;
        let vertex_shader = compile_shader(gl, glow::VERTEX_SHADER, vertex_source)?;
        let fragment_shader = compile_shader(gl, glow::FRAGMENT_SHADER, fragment_source)?;

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
    screen_num: c_int,
    visual_id: u32,
) -> Result<glx::GLXFBConfig, OverlayError> {
    unsafe {
        let mut count = 0;
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
            8,
            0,
        ];

        let configs = glx::glXChooseFBConfig(display, screen_num, attribs.as_ptr(), &mut count);
        if configs.is_null() || count <= 0 {
            return Err(runtime_error("failed to choose a GLX FBConfig"));
        }

        let configs = std::slice::from_raw_parts(configs, count as usize);
        let mut chosen = None;

        for &config in configs {
            let mut value = 0;
            if glx::glXGetFBConfigAttrib(display, config, glx::GLX_VISUAL_ID as c_int, &mut value)
                == 0
                && value as u32 == visual_id
            {
                chosen = Some(config);
                break;
            }
        }

        let _ = xlib::XFree(configs.as_ptr() as *mut c_void);

        chosen.ok_or_else(|| {
            runtime_error(format!(
                "no GLX FBConfig matched visual id {visual_id:#x}"
            ))
        })
    }
}

pub(crate) struct GlxRenderer {
    display: *mut xlib::Display,
    window: xlib::Window,
    context: glx::GLXContext,
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    texture: glow::Texture,
    u_screen_size: glow::UniformLocation,
    u_frame_size: glow::UniformLocation,
    u_translate: glow::UniformLocation,
    u_scale: glow::UniformLocation,
    frame_size: (f32, f32),
}

impl GlxRenderer {
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn new(
        screen_num: c_int,
        window: xlib::Window,
        visual_id: u32,
    ) -> Result<Self, OverlayError> {
        let display = xlib::XOpenDisplay(ptr::null());
        if display.is_null() {
            return Err(runtime_error("failed to open X11 display for GLX renderer"));
        }

        let result = (|| -> Result<Self, OverlayError> {
            let mut major = 0;
            let mut minor = 0;
            if glx::glXQueryVersion(display, &mut major, &mut minor) == 0 {
                return Err(runtime_error("failed to query GLX version"));
            }

            if major < 1 || (major == 1 && minor < 3) {
                return Err(runtime_error(format!(
                    "GLX 1.3 or newer is required, found {major}.{minor}"
                )));
            }

            let fb_config = choose_fbconfig(display, screen_num, visual_id)?;

            let context = glx::glXCreateNewContext(
                display,
                fb_config,
                glx::GLX_RGBA_TYPE as c_int,
                ptr::null_mut(),
                1,
            );
            if context.is_null() {
                return Err(runtime_error("failed to create GLX context"));
            }

            if glx::glXMakeCurrent(display, window as c_ulong, context) == 0 {
                glx::glXDestroyContext(display, context);
                return Err(runtime_error("failed to make GLX context current"));
            }

            let gl = glow::Context::from_loader_function(load_gl_proc);
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
                    0.0, 0.0, 0.0, 0.0,
                    1.0, 0.0, 1.0, 0.0,
                    1.0, 1.0, 1.0, 1.0,
                    0.0, 0.0, 0.0, 0.0,
                    1.0, 1.0, 1.0, 1.0,
                    0.0, 1.0, 0.0, 1.0,
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

            Ok(Self {
                display,
                window,
                context,
                gl,
                program,
                vao,
                vbo,
                texture,
                u_screen_size,
                u_frame_size,
                u_translate,
                u_scale,
                frame_size: (1.0, 1.0),
            })
        })();

        if result.is_err() {
            let _ = xlib::XCloseDisplay(display);
        }

        result
    }

    pub fn upload_frame(&mut self, frame: &CapturedFrame) -> Result<(), OverlayError> {
        self.frame_size = (frame.width as f32, frame.height as f32);

        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA as i32,
                frame.width as i32,
                frame.height as i32,
                0,
                glow::BGRA,
                glow::UNSIGNED_BYTE,
                PixelUnpackData::Slice(Some(&frame.rgba)),
            );
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }

        Ok(())
    }

    pub fn draw(
        &self,
        placement: ScreenshotPlacement,
        screen_size: (i32, i32),
    ) -> Result<(), OverlayError> {
        unsafe {
            self.gl.viewport(0, 0, screen_size.0, screen_size.1);
            self.gl.clear_color(0.0, 0.0, 0.0, 1.0);
            self.gl.clear(glow::COLOR_BUFFER_BIT);

            self.gl.use_program(Some(self.program));
            self.gl.bind_vertex_array(Some(self.vao));
            self.gl.active_texture(glow::TEXTURE0);
            self.gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));

            self.gl.uniform_2_f32(
                Some(&self.u_screen_size),
                screen_size.0 as f32,
                screen_size.1 as f32,
            );
            self.gl.uniform_2_f32(Some(&self.u_frame_size), self.frame_size.0, self.frame_size.1);
            self.gl.uniform_2_f32(
                Some(&self.u_translate),
                placement.translate_x,
                placement.translate_y,
            );
            self.gl.uniform_1_f32(Some(&self.u_scale), placement.scale);

            self.gl.draw_arrays(glow::TRIANGLES, 0, 6);

            self.gl.bind_texture(glow::TEXTURE_2D, None);
            self.gl.bind_vertex_array(None);
            self.gl.use_program(None);

            glx::glXSwapBuffers(self.display, self.window as c_ulong);
        }

        Ok(())
    }
}

impl Drop for GlxRenderer {
    fn drop(&mut self) {
        unsafe {
            let _ = glx::glXMakeCurrent(self.display, 0, ptr::null_mut());
            self.gl.delete_texture(self.texture);
            self.gl.delete_buffer(self.vbo);
            self.gl.delete_vertex_array(self.vao);
            self.gl.delete_program(self.program);
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
    v_uv = a_uv;
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