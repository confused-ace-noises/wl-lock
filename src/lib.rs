use std::{collections::HashMap, os::linux::raw::stat, process::exit};

use rustix::fs::Stat;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_dispatch, delegate_noop, globals::GlobalListContents, protocol::{
        wl_compositor::WlCompositor, wl_output::{self, WlOutput}, wl_registry::{self, WlRegistry}, wl_seat::{self, Capability, WlSeat}, wl_subcompositor::WlSubcompositor, wl_surface::WlSurface
    }
};
use wayland_protocols::ext::session_lock::v1::client::{ext_session_lock_manager_v1::ExtSessionLockManagerV1, ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1}, ext_session_lock_v1::{self, ExtSessionLockV1}};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use crate::utils::late::Late;

pub mod utils;

pub struct Global<T> {
    pub global: T,
    pub name: u32,
}

impl<T> Global<T> {
    pub fn new(global: T, name: u32) -> Self {
        Self { global, name }
    }
}

pub struct Seat {
    pub wl_seat: WlSeat,
    pub capabilities: Option<WEnum<Capability>>,
    pub name: Option<String>,
}

pub struct Output {
    pub wl_output: WlOutput,
    pub surface: Late<WlSurface>,
    pub lock_surface: Late<ExtSessionLockSurfaceV1>,
    pub width: u32, 
    pub height: u32,
    pub name: u32,
    pub configured: bool,
}

pub struct App {
    pub connection: Connection,
    pub event_queue: EventQueue<State>,
    pub state: State,
}

#[derive(Default)]
pub struct State {
    pub compositor: Late<Global<WlCompositor>>,
    pub layer_shell: Late<Global<ZwlrLayerShellV1>>,
    pub lock_manager: Late<Global<ExtSessionLockManagerV1>>,
    pub seats: HashMap<u32, Seat>,
    
    pub outputs: HashMap<u32, Output>,
    pub init_done: bool,
    pub exit: Option<u32>,

    pub is_locked: bool,
}

impl App {
    pub fn init() -> App {
        let conn = Connection::connect_to_env().expect("Couldn't connect to wayland server");

        let mut event_queue = conn.new_event_queue::<State>();
        let qh = event_queue.handle();

        let mut state = State::default();

        let display = conn.display();
        let _registry = display.get_registry(&qh, ());

        event_queue.roundtrip(&mut state).unwrap(); // globals

        assert!(state.compositor.is_init() && state.layer_shell.is_init());

        state.init_done = true;

        App { connection: conn, event_queue, state }
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

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &WlRegistry,
        event: <WlRegistry as wayland_client::Proxy>::Event,
        _: &(),
        _: &Connection,
        qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                println!("{name}: {interface} v{version}");

                match interface.as_str() {
                    "wl_compositor" => Self::bind(
                        &mut state.compositor,
                        proxy,
                        name,
                        qhandle,
                        Self::MIN_WL_COMPOSITOR_VER.min(version),
                    ),

                    "zwlr_layer_shell_v1" => Self::bind(
                        &mut state.layer_shell,
                        proxy,
                        name,
                        qhandle,
                        Self::MIN_ZWLR_LAYER_SHELL_VER.min(version),
                    ),

                    "wl_seat" => {
                        let wl_seat =
                            proxy.bind(name, Self::MIN_WL_SEAT_VER.min(version), qhandle, name);
                        state.seats.insert(
                            name,
                            Seat {
                                wl_seat,
                                capabilities: None,
                                name: None,
                            },
                        );
                    },

                    "ext_session_lock_manager_v1" => {
                        Self::bind(&mut state.lock_manager, proxy, name, qhandle, 1)
                    },

                    "wl_output" => {
                        let output: WlOutput = proxy.bind(name, version.min(4), qhandle, ());
                        state.outputs.insert(name, Output {
                            wl_output: output,
                            surface: Late::uninit(),
                            lock_surface: Late::uninit(),
                            width: 0, height: 0,
                            name,
                            configured: false,
                        });
                    }

                    _ => {} // do nothing if it's not recognized
                }
            }

            wl_registry::Event::GlobalRemove { name } => {
                if let Some(seat) = state.seats.remove(&name) {
                    seat.wl_seat.release();
                } else {
                    eprintln!("A core global was removed by the server, shutting down...");
                    state.exit = Some(1);
                }
            }

            _ => unimplemented!(),
        }
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: ZwlrLayerShellV1);
delegate_noop!(State: ExtSessionLockManagerV1);

impl Dispatch<WlSeat, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlSeat,
        event: <WlSeat as Proxy>::Event,
        data: &u32,
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        let seat = state
            .seats
            .get_mut(data)
            .expect("Server sent a wl_seat event before registering said seat.");

        match event {
            wl_seat::Event::Capabilities { capabilities } => seat.capabilities = Some(capabilities),
            wl_seat::Event::Name { name } => seat.name = Some(name),
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for State {
    fn event(
        state: &mut Self,
        proxy: &ExtSessionLockV1,
        event: <ExtSessionLockV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_v1::Event::Locked => {
                state.is_locked = true;
            },
            
            ext_session_lock_v1::Event::Finished if state.is_locked => {
                proxy.unlock_and_destroy();
                state.is_locked = false;
            },

            ext_session_lock_v1::Event::Finished if !state.is_locked => {
                proxy.destroy();
            },

            _ => unimplemented!(),
        }
    }
}

delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlOutput);

// impl Dispatch<WlOutput, ()> for State {
//     fn event(
//         state: &mut Self,
//         proxy: &WlOutput,
//         event: <WlOutput as Proxy>::Event,
//         data: &(),
//         conn: &Connection,
//         qhandle: &QueueHandle<Self>,
//     ) {
//         match event {
//             wl_output::Event::Geometry { x, y, physical_width, physical_height, subpixel, make, model, transform } => todo!(),
//             wl_output::Event::Mode { flags, width, height, refresh } => todo!(),
//             wl_output::Event::Done => todo!(),
//             wl_output::Event::Scale { factor } => todo!(),
//             wl_output::Event::Name { name } => todo!(),
//             wl_output::Event::Description { description } => todo!(),
//             _ => todo!(),
//         }
//     }
// }

impl Dispatch<ExtSessionLockSurfaceV1, u32> for State {
    fn event(
        state: &mut Self,
        proxy: &ExtSessionLockSurfaceV1,
        event: <ExtSessionLockSurfaceV1 as Proxy>::Event,
        data: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_session_lock_surface_v1::Event::Configure { serial, width, height } => {
                let output = state.outputs.get_mut(data).unwrap();
                output.height = height;
                output.width = width;
                output.configured = true;
                proxy.ack_configure(serial);
            },
            _ => unimplemented!(),
        }
    }
}