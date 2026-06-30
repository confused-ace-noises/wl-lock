use std::{collections::HashMap, ffi::CStr, mem, os::raw::c_char, sync::Mutex};

use egui::{Context, Modifiers, Ui};
use egui_wgpu::{Renderer, RendererOptions};
use libc::{getpwuid_r, getuid, passwd};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, protocol::{
        wl_buffer::WlBuffer, wl_callback::WlCallback, wl_compositor::WlCompositor, wl_display::WlDisplay, wl_keyboard::WlKeyboard, wl_output::WlOutput, wl_pointer::{self, WlPointer}, wl_registry::WlRegistry, wl_seat::Capability, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface,
    },
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1, ext_session_lock_v1::ExtSessionLockV1,
};
use wgpu::{
    Adapter, BackendOptions, Backends, CompositeAlphaMode, CurrentSurfaceTexture, Device, Instance, InstanceDescriptor, InstanceFlags, MemoryBudgetThresholds, Operations, PowerPreference, PresentMode, Queue, RequestAdapterOptions, SurfaceTarget, TextureFormat, TextureUsages, TextureViewDescriptor, wgt::{DeviceDescriptor, SurfaceConfiguration, WgpuHasDisplayHandle},
};
use xkbcommon::xkb::{self};

use crate::{
    Output, Seat, WaylandDisplayH, WaylandSurfaceH,
    utils::{global::Global, late::Late},
};

pub mod seat;
pub mod session_lock;
pub mod wl_registry;

pub struct App {
    pub connection: Connection,
    pub event_queue: EventQueue<State>,
    pub display: WlDisplay,
    pub state: State,
}

#[derive(Default)]
pub struct State {
    pub compositor: Late<Global<WlCompositor>>,
    pub shm: Late<Global<WlShm>>,
    pub display_handle: Late<WaylandDisplayH>,

    pub seats: HashMap<u32, Seat>,
    pub input: Late<Input>,

    pub wgpu: Late<WgpuInfo>,
    pub egui_renderer: Late<Mutex<Renderer>>,

    pub lock_manager: Late<Global<ExtSessionLockManagerV1>>,
    pub session_lock: Late<ExtSessionLockV1>,

    pub outputs: HashMap<u32, Output>,
    pub init_done: bool,
    pub exit: Option<u32>,

    pub is_locked: bool,
    pub new_events: bool,
    pub pam: Late<Pam>,
}

impl State {
    pub const DOTS_PER_LINE: f32 = 15.0;
}

pub struct Pam {
    pub uid: u32,
    _buffer: Vec<i8>,
    _passwd: passwd,
    _res: *mut passwd,
    pub username: String,
}

pub struct Input {
    xkb_ctx: xkb::Context,
    pointer: Option<Pointer>,
    keyboard: Option<Kb>,
}

pub struct Kb {
    pub focused_output: Option<u32>,
    pub wl_keyboard: WlKeyboard,
    pub xkb_state: Late<xkb::State>,
    pub key_mods: egui::Modifiers,
}

pub struct Pointer {
    pub focused_output: Option<u32>,
    pub wl_pointer: WlPointer,
    pub last_focused_output_in_events: Option<u32>,
    pub last_pointer_pos: Option<(f32, f32)>,
}

pub enum PointerEvent {
    Event(wl_pointer::Event),
    Axis {
        ordered_ev: Vec<wl_pointer::Event>,
        source: Option<wl_pointer::AxisSource>,
        /// bitfield:
        /// 0b00000001 -> Axis
        /// 0b00000010 -> AxisValue120
        /// 0b00000100 -> AxisDiscrete
        available_modes: u8,
        is_stop: Option<bool>,
    },
}

pub struct WgpuInfo {
    pub instance: Instance,
    pub adapter: Adapter,
    pub device: Device,
    pub queue: Queue,
}

pub struct EguiInfo {
    pub context: Context,
    pub renderer: Mutex<Renderer>,
}

impl App {
    pub fn _init() -> App {
        let conn = Connection::connect_to_env().expect("Couldn't connect to wayland server");

        let mut event_queue = conn.new_event_queue::<State>();
        let qh = event_queue.handle();

        let mut state = State::default();

        let display = conn.display();
        let _registry = display.get_registry(&qh, ());

        event_queue.roundtrip(&mut state).unwrap(); // globals

        assert!(state.compositor.is_init());

        state.init_done = true;

        App {
            connection: conn,
            event_queue,
            state,
            display,
        }
    }

    pub fn init() -> App {
        let mut app = App::_init();
        
        app.create_surfaces();
        app.init_wgpu();
        app.init_egui();
        app.init_input();
        app.init_pam();
        app.image_capabilities();
        
        app
    }

    pub fn create_surfaces(&mut self) {
        let qh = self.event_queue.handle();
        let display_handle = WaylandDisplayH::new(&self.connection);

        self.state.display_handle.init(display_handle);

        let compositor = &self.state.compositor;
        let session_lock_manager = &self.state.lock_manager;
        let lock = session_lock_manager.lock(&qh, ());

        for (name, output) in &mut self.state.outputs {
            let wl_surface = compositor.create_surface(&qh, ());
            let role = lock.get_lock_surface(&wl_surface, &output.wl_output, &qh, *name);

            let handle = WaylandSurfaceH::new(&wl_surface);

            output.surface_info.init(crate::SurfaceInfo {
                surface: wl_surface,
                lock_surface: role,
                surface_handle: handle,
                width: Late::uninit(),
                height: Late::uninit(),
                wgpu_surface: Late::uninit(),
            });
        }

        self.state.session_lock.init(lock);

        // get configure events
        self.event_queue.roundtrip(&mut self.state).unwrap();
    }

    pub fn init_input(&mut self) {
        self.state.input.init(Input {
            xkb_ctx: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            pointer: None,
            keyboard: None,
        });

        let qh = self.event_queue.handle();

        for (_, seat) in self.state.seats.iter() {
            if let Some(caps) = seat.capabilities {
                match caps {
                    wayland_client::WEnum::Value(cap) => {
                        if cap.contains(Capability::Keyboard) {
                            let wl_keyboard = seat.wl_seat.get_keyboard(&qh, ());
                            self.state.input.keyboard = Some(Kb {
                                focused_output: None,
                                wl_keyboard,
                                xkb_state: Late::uninit(),
                                key_mods: Modifiers::NONE,
                            })
                        }

                        if cap.contains(Capability::Pointer) {
                            let wl_pointer = seat.wl_seat.get_pointer(&qh, ());
                            self.state.input.pointer = Some(Pointer {
                                focused_output: None,
                                wl_pointer,
                                last_focused_output_in_events: None,
                                last_pointer_pos: None,
                            })
                        }
                    }
                    wayland_client::WEnum::Unknown(_) => unimplemented!(),
                }
            }
        }

        self.event_queue.roundtrip(&mut self.state).unwrap();
    }

    pub fn init_wgpu(&mut self) {
        let instance = wgpu::Instance::new(Self::wgpu_instance_desc(*self.state.display_handle));

        for (_, output) in self.state.outputs.iter_mut() {
            let wgpu_surface = instance
                .create_surface(SurfaceTarget::Window(Box::new(
                    output.surface_info.surface_handle,
                )))
                .unwrap();

            output.surface_info.wgpu_surface.init(wgpu_surface);
        }

        let adapter = instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            compatible_surface: Some(
                &self
                    .state
                    .outputs
                    .iter()
                    .next()
                    .unwrap()
                    .1
                    .surface_info
                    .wgpu_surface,
            ),
            ..Default::default()
        });

        let adapter = pollster::block_on(adapter).unwrap();

        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default())).unwrap();

        println!("{}", device.limits().max_texture_dimension_3d);

        self.state.outputs.iter_mut().for_each(|(_, output)| {
            output.surface_info.wgpu_surface.configure(
                &device,
                &Self::wgpu_surface_config(*output.surface_info.width, *output.surface_info.height),
            );
        });

        self.state.wgpu.init(WgpuInfo {
            instance,
            adapter,
            device,
            queue,
        });
    }

    fn wgpu_surface_config(width: u32, height: u32) -> SurfaceConfiguration<Vec<TextureFormat>> {
        SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            format: TextureFormat::Bgra8UnormSrgb,
            width,
            height,
            present_mode: PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: CompositeAlphaMode::Auto,
            view_formats: vec![],
        }
    }

    fn wgpu_instance_desc(display: impl WgpuHasDisplayHandle) -> InstanceDescriptor {
        InstanceDescriptor {
            backends: Backends::all(),
            flags: InstanceFlags::default(),
            memory_budget_thresholds: MemoryBudgetThresholds::default(),
            backend_options: BackendOptions::default(),
            display: Some(Box::new(display)),
        }
    }

    pub fn init_egui(&mut self) {
        for (_, output) in self.state.outputs.iter_mut() {
            let ctx = Context::default();
            ctx.input_mut(|x| x.max_texture_side = 8000);
            output.egui_context.init(ctx);
        }

        let renderer = egui_wgpu::Renderer::new(
            &self.state.wgpu.device,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            RendererOptions::default(),
        );

        self.state.egui_renderer.init(Mutex::new(renderer));
    }

    pub fn send_frame_req(&mut self) {
        let qh = self.event_queue.handle();

        for (_, output) in self.state.outputs.iter() {
            output.surface_info.surface.frame(&qh, ());
        }
        self.event_queue.roundtrip(&mut self.state).unwrap();
    }

    pub fn image_capabilities(&mut self) {
        for (_, output) in self.state.outputs.iter() {
            egui_extras::install_image_loaders(&output.egui_context)
            // output.egui_context.options_mut(|x| x);
        }
        self.event_queue.roundtrip(&mut self.state).unwrap();
    }

    pub fn init_pam(&mut self) {
        let uid = unsafe { getuid() };
        let mut pwd: passwd = unsafe { mem::zeroed() };
        let mut buf = vec![0i8; 1024];
        let mut res = std::ptr::null_mut();

        unsafe {
            getpwuid_r(uid, &mut pwd as *mut passwd, buf.as_mut_ptr() as *mut c_char, buf.len(), &mut res);
        }

        let username = unsafe { CStr::from_ptr(pwd.pw_name) }.to_string_lossy().to_string().clone();


        self.state.pam.init(Pam { uid, _buffer: buf, _passwd: pwd, _res: res, username });
    }

    /// passwd argument will get cleared when called
    pub fn pam_auth(&self, passwd: &String) -> bool {
        let mut pam_client = pam::Client::with_password("login").expect("failed to start PAM client");
        pam_client.conversation_mut().set_credentials(&self.state.pam.username, passwd);
        match pam_client.authenticate() {
            Ok(_) => {
                println!("success!");
                true
            },
            Err(e) => {
                println!("failed: {e:?}");
                false
            } 
        }
    }

    pub fn frame_to_output(&mut self, output_name: u32, run_ui: impl for<'a> FnMut(&'a mut Ui)) -> Option<()> {
        let device = &self.state.wgpu.device;
        let output = self.state.outputs.get_mut(&output_name)?;
        let wgpu_surface = &output.surface_info.wgpu_surface;
        let ctx = &output.egui_context;

        let qh = &self.event_queue.handle();

        output.surface_info.surface.frame(qh, ());

        let width = *output.surface_info.width;
        let height = *output.surface_info.height;

        if !(self.state.new_events || output.egui_context.has_requested_repaint()) {
            return Some(());
        }

        let surface_texture = match wgpu_surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture) => texture,
            CurrentSurfaceTexture::Suboptimal(texture) => {
                // wgpu_surface.configure(&self.state.wgpu.device, &Self::wgpu_surface_config(width, height));
                texture
            }
            _ => return None,
        };

        let mut encoder = device.create_command_encoder(&Default::default());

        let view = surface_texture
            .texture
            .create_view(&TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: ctx.pixels_per_point(),
        };

        let raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(width as f32, height as f32),
            )),
            events: mem::take(&mut output.events_to_flush),
            ..Default::default()
        };

        let full_output = ctx.run_ui(raw_input, run_ui);

        let primitives = ctx.tessellate(full_output.shapes, ctx.pixels_per_point());

        let mut renderer = self.state.egui_renderer.lock().unwrap();

        for (id, delta) in &full_output.textures_delta.set {
            renderer.update_texture(device, &self.state.wgpu.queue, *id, delta);
        }

        renderer.update_buffers(
            device,
            &self.state.wgpu.queue,
            &mut encoder,
            &primitives,
            &screen_descriptor,
        );

        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations::default(),
            })],
            ..Default::default()
        });

        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &primitives, &screen_descriptor);

        for id in &full_output.textures_delta.free {
            renderer.free_texture(id);
        }

        drop(renderer);
        drop(pass);

        self.state.wgpu.queue.submit([encoder.finish()]);
        surface_texture.present();
        self.event_queue.flush().unwrap();
        Some(())
    }
}

impl State {
    pub const MIN_WL_COMPOSITOR_VER: u32 = 6;
    pub const MIN_WL_SEAT_VER: u32 = 9;
    pub const MIN_WL_SUBCOMPOSITOR_VER: u32 = 1;
    pub const MIN_ZWLR_LAYER_SHELL_VER: u32 = 4;
    pub const MIN_WL_SHM_VER: u32 = 2;

    pub fn bind<T>(
        bind_to: &mut Late<Global<T>>,
        proxy: &WlRegistry,
        name: u32,
        qh: &QueueHandle<Self>,
        version: u32,
    ) where
        T: Proxy + 'static,
        Self: Dispatch<T, ()>,
    {
        bind_to.init(Global::new(proxy.bind(name, version, qh, ()), name));
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: ExtSessionLockManagerV1);
delegate_noop!(State: WlShmPool);

delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);


impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlOutput,
        event: <WlOutput as Proxy>::Event,
        data: &u32,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_output::Event::Name { name } = event { 
            let output = state.outputs.get_mut(data).unwrap();
            output.display_name.init(name);
        }
    }
}
// impl Dispatch<WlBuffer, ()> for App {
//     fn event(
//         state: &mut Self,
//         proxy: &WlBuffer,
//         event: <WlBuffer as Proxy>::Event,
//         data: &(),
//         conn: &Connection,
//         qhandle: &QueueHandle<Self>,
//     ) {
//         match event {
//             wayland_client::protocol::wl_buffer::Event::Release => {

//             },
//             _ => todo!(),
//         }
//     }
// }
delegate_noop!(State: ignore WlBuffer);

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlCallback,
        event: <WlCallback as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        if let wayland_client::protocol::wl_callback::Event::Done { .. } = event {
            state.new_events = true;
        }
    }
}
