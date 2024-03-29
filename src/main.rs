use khronos_egl as egl;
use std::{rc::Rc, result, str::FromStr};
use thiserror::Error;
use tracing_subscriber;
use wayland_client::{
    delegate_noop,
    protocol::{
        wl_compositor,
        wl_display::WlDisplay,
        wl_registry,
        wl_surface::{self, WlSurface},
    },
    ConnectError, Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_egl::WlEglSurface;
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

pub type Result<T, E = WaylandEGLStateError> = result::Result<T, E>;

#[derive(Error, Debug)]
pub enum WaylandEGLStateError {
    #[error("xdg_wm_base global missing")]
    XdgWmBaseMissing,

    #[error("wl_compositor global missing")]
    WlCompositorMissing,
}

#[derive(Debug)]
struct WaylandEGLState {
    width: i32,
    height: i32,
    running: bool,
    title: String,

    wl_connection: Connection,
    wl_display: WlDisplay,
    wl_surface: Option<WlSurface>,

    egl_window: Option<Rc<WlEglSurface>>,
    egl_display: Option<egl::Display>,
    egl_surface: Option<egl::Surface>,
    egl_context: Option<egl::Context>,

    xdg_wm_base: Option<xdg_wm_base::XdgWmBase>,
    xdg_surface: Option<xdg_surface::XdgSurface>,
    xdg_toplevel: Option<xdg_toplevel::XdgToplevel>,
    wl_compositor: Option<wl_compositor::WlCompositor>,
}

impl WaylandEGLState {
    #[tracing::instrument]
    fn new() -> Result<Self, ConnectError> {
        let server_connection = Connection::connect_to_env()?;

        Ok(Self {
            width: 1280,
            height: 720,
            running: true,
            title: "Nya".into(),

            wl_connection: server_connection.clone(),
            wl_display: server_connection.display(),
            wl_surface: None,

            egl_window: None,
            egl_display: None,
            egl_surface: None,
            egl_context: None,

            xdg_wm_base: None,
            xdg_surface: None,
            xdg_toplevel: None,
            wl_compositor: None,
        })
    }

    fn validate_globals(self: &Self) -> Result<()> {
        if let None = self.xdg_wm_base {
            return Err(WaylandEGLStateError::XdgWmBaseMissing);
        } else if let None = self.wl_compositor {
            return Err(WaylandEGLStateError::WlCompositorMissing);
        }

        Ok(())
    }
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::from_str("trace")?)
        .with_writer(std::io::stderr)
        .init();

    let mut state = WaylandEGLState::new()?;

    // Wayland init :)
    let mut event_queue = state.wl_connection.new_event_queue();

    let queue_handle = event_queue.handle();
    let _registry = state.wl_display.get_registry(&queue_handle, ());

    event_queue.roundtrip(&mut state)?;

    state.validate_globals()?;

    state.wl_surface = Some(
        state
            .wl_compositor
            .as_ref()
            .unwrap()
            .create_surface(&queue_handle, ()),
    );

    state.xdg_surface = Some(state.xdg_wm_base.clone().unwrap().get_xdg_surface(
        &state.wl_surface.clone().unwrap(),
        &queue_handle,
        (),
    ));

    state.xdg_toplevel = Some(
        state
            .xdg_surface
            .clone()
            .unwrap()
            .get_toplevel(&queue_handle, ()),
    );
    state
        .xdg_toplevel
        .clone()
        .unwrap()
        .set_title(state.title.clone());
    state.wl_surface.clone().unwrap().commit();

    state.egl_window = Some(Rc::new(WlEglSurface::new(
        state.wl_surface.clone().unwrap().id(),
        state.width,
        state.height,
    )?));

    // We do egl stuff here:

    let egl = egl::Instance::new(egl::Static);

    state.egl_display = Some(
        unsafe { egl.get_display(state.wl_display.id().as_ptr() as *mut std::ffi::c_void) }
            .unwrap(),
    );

    egl.initialize(state.egl_display.unwrap())?;

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
        egl::NONE,
    ];

    let config = egl
        .choose_first_config(state.egl_display.unwrap(), &attributes)?
        .expect("unable to find an appropriate EGL configuration");

    state.egl_surface = Some(unsafe {
        egl.create_window_surface(
            state.egl_display.unwrap(),
            config,
            state.egl_window.clone().unwrap().ptr() as egl::NativeWindowType,
            None,
        )?
    });

    let context_attributes = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE, egl::NONE];
    state.egl_context = Some(egl.create_context(
        state.egl_display.unwrap(),
        config,
        None,
        &context_attributes,
    )?);

    egl.make_current(
        state.egl_display.unwrap(),
        state.egl_surface,
        state.egl_surface,
        state.egl_context,
    )?;

    gl_loader::init_gl();
    gl::load_with(|s| gl_loader::get_proc_address(s) as *const _);

    // We do painting stuff here
    while state.running {
        event_queue.dispatch_pending(&mut state)?;

        tracing::event!(tracing::Level::DEBUG, "Rendering");

        unsafe {
            gl::ClearColor(0.0 / 255 as f32, 79.0 / 255 as f32, 158.0 / 255 as f32, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT);
        }

        egl.swap_buffers(state.egl_display.unwrap(), state.egl_surface.unwrap())?;
    }

    egl.destroy_surface(state.egl_display.unwrap(), state.egl_surface.unwrap())?;
    egl.destroy_context(state.egl_display.unwrap(), state.egl_context.unwrap())?;

    state.xdg_surface.clone().unwrap().destroy();
    state.wl_surface.clone().unwrap().destroy();

    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, ()> for WaylandEGLState {
    #[tracing::instrument(skip(registry, queue_handle, state), ret, level = "trace")]
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue_handle: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "xdg_wm_base" => {
                    state.xdg_wm_base = Some(registry.bind::<xdg_wm_base::XdgWmBase, _, _>(
                        name,
                        version,
                        queue_handle,
                        (),
                    ));
                }
                "wl_compositor" => {
                    state.wl_compositor = Some(registry.bind::<wl_compositor::WlCompositor, _, _>(
                        name,
                        version,
                        queue_handle,
                        (),
                    ));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for WaylandEGLState {
    #[tracing::instrument(skip(xdg_wm_base), ret, level = "trace")]
    fn event(
        _: &mut Self,
        xdg_wm_base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            xdg_wm_base.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for WaylandEGLState {
    #[tracing::instrument(skip(xdg_surface), ret, level = "trace")]
    fn event(
        _: &mut Self,
        xdg_surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for WaylandEGLState {
    #[tracing::instrument(skip(), ret, level = "trace")]
    fn event(
        state: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        event: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width == 0 || height == 0 {
                    return; // We do not respect this configure
                }

                if state.width != width || state.height != height {
                    state.width = width;
                    state.height = height;

                    state
                        .egl_window
                        .clone()
                        .unwrap()
                        .resize(state.width, state.height, 0, 0);
                    state.wl_surface.clone().unwrap().commit();
                }
            }
            xdg_toplevel::Event::Close {} => {
                state.running = false;
            }
            _ => {}
        }
    }
}

delegate_noop!(WaylandEGLState: wl_compositor::WlCompositor);
delegate_noop!(WaylandEGLState: wl_surface::WlSurface);
