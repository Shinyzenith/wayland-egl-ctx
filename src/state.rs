use crate::error::{Result, WaylandEGLStateError};
use crate::utils::load_shader;

use gl::types::GLuint;
use khronos_egl as egl;
use std::{ffi::c_void, mem::transmute, rc::Rc};
use wayland_client::{
    protocol::{wl_compositor, wl_display::WlDisplay, wl_surface::WlSurface},
    ConnectError, Connection, Proxy,
};
use wayland_egl::WlEglSurface;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

#[derive(Debug)]
pub struct WaylandEGLState {
    pub width: i32,
    pub height: i32,
    pub running: bool,
    pub title: String,

    pub wl_connection: Connection,
    pub wl_display: WlDisplay,
    pub wl_surface: Option<WlSurface>,

    pub egl: egl::Instance<egl::Static>,
    pub egl_window: Option<Rc<WlEglSurface>>,
    pub egl_display: Option<egl::Display>,
    pub egl_surface: Option<egl::Surface>,
    pub egl_context: Option<egl::Context>,

    pub gl_program: GLuint,

    pub xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    pub xdg_surface: Option<xdg_surface::XdgSurface>,
    pub xdg_toplevel: Option<xdg_toplevel::XdgToplevel>,
    pub wl_compositor: Option<wl_compositor::WlCompositor>,
}

impl WaylandEGLState {
    #[tracing::instrument]
    pub fn new() -> Result<Self, ConnectError> {
        let server_connection = Connection::connect_to_env()?;
        Ok(Self {
            width: 320,
            height: 240,
            running: true,
            title: "Nya".into(),

            wl_connection: server_connection.clone(),
            wl_display: server_connection.display(),
            wl_surface: None,

            egl: egl::Instance::new(egl::Static),
            egl_window: None,
            egl_display: None,
            egl_surface: None,
            egl_context: None,

            gl_program: 0,

            xdg_wm_base: None,
            xdg_surface: None,
            xdg_toplevel: None,
            wl_compositor: None,
        })
    }

    pub fn deinit(self: &Self) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            gl::DeleteProgram(self.gl_program);
        }

        self.egl
            .destroy_surface(self.egl_display.unwrap(), self.egl_surface.unwrap())?;
        self.egl
            .destroy_context(self.egl_display.unwrap(), self.egl_context.unwrap())?;

        self.xdg_surface.clone().unwrap().destroy();
        self.wl_surface.clone().unwrap().destroy();

        Ok(())
    }

    pub fn init_egl(self: &mut Self) -> Result<(), Box<dyn std::error::Error>> {
        // Init gl
        gl_loader::init_gl();
        gl::load_with(|s| gl_loader::get_proc_address(s) as *const _);

        self.egl_window = Some(Rc::new(WlEglSurface::new(
            self.wl_surface.clone().unwrap().id(),
            self.width,
            self.height,
        )?));

        self.egl_display = Some(
            unsafe {
                self.egl
                    .get_display(self.wl_display.id().as_ptr() as *mut c_void)
            }
            .unwrap(),
        );

        self.egl.initialize(self.egl_display.unwrap())?;

        let attributes = [
            egl::SURFACE_TYPE,
            egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_ES2_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];

        let config = self
            .egl
            .choose_first_config(self.egl_display.unwrap(), &attributes)?
            .expect("unable to find an appropriate EGL configuration");

        self.egl_surface = Some(unsafe {
            self.egl.create_window_surface(
                self.egl_display.unwrap(),
                config,
                self.egl_window.clone().unwrap().ptr() as egl::NativeWindowType,
                None,
            )?
        });

        let context_attributes = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE, egl::NONE];
        self.egl_context = Some(self.egl.create_context(
            self.egl_display.unwrap(),
            config,
            None,
            &context_attributes,
        )?);

        self.egl.make_current(
            self.egl_display.unwrap(),
            self.egl_surface,
            self.egl_surface,
            self.egl_context,
        )?;

        self.init_program()?;

        Ok(())
    }

    fn init_program(self: &mut Self) -> Result<()> {
        let vert_shader = load_shader(
            gl::VERTEX_SHADER,
            include_str!("./shaders/shader.vert").into(),
        )
        .unwrap();

        let frag_shader = load_shader(
            gl::FRAGMENT_SHADER,
            include_str!("./shaders/shader.frag").into(),
        )
        .unwrap();

        unsafe {
            self.gl_program = gl::CreateProgram();
        }

        if self.gl_program == 0 {
            tracing::event!(tracing::Level::ERROR, "glCreateProgramFailed!");
            return Err(WaylandEGLStateError::GLCreateProgramFailed);
        }

        unsafe {
            gl::AttachShader(self.gl_program, vert_shader);
            gl::AttachShader(self.gl_program, frag_shader);

            gl::LinkProgram(self.gl_program);
        }

        let mut linked: gl::types::GLint = 1;
        unsafe { gl::GetProgramiv(self.gl_program, gl::LINK_STATUS, &mut linked as *mut i32) }

        if linked > 0 {
            tracing::event!(tracing::Level::INFO, "Successfully linked the program!");
        } else {
            return Err(WaylandEGLStateError::GLLinkProgramFailed);
        }

        Ok(())
    }

    pub fn draw(self: &Self) {
        let ptr: [gl::types::GLfloat; 9] = [0.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, -1.0, 0.0];
        unsafe {
            gl::ClearColor(1.0, 1.0, 1.0, 0.0);
            gl::Clear(gl::CLEAR_BUFFER);

            gl::UseProgram(self.gl_program);

            gl::VertexAttribPointer(0, 3, gl::FLOAT, gl::FALSE, 0, transmute(&ptr[0]));

            gl::EnableVertexAttribArray(0);
            gl::DrawArrays(gl::TRIANGLES, 0, 3);
        }
    }

    pub fn validate_globals(self: &Self) -> Result<()> {
        if let None = self.xdg_wm_base {
            return Err(WaylandEGLStateError::XdgWmBaseMissing);
        } else if let None = self.wl_compositor {
            return Err(WaylandEGLStateError::WlCompositorMissing);
        }

        Ok(())
    }
}
