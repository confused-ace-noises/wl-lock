use wayland_client::{Connection, Dispatch, protocol::{wl_output::WlOutput, wl_registry::{self, WlRegistry}}};

use crate::{Output, Seat, state::State};

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
                        let output: WlOutput = proxy.bind(name, version.min(4), qhandle, name);
                        state.outputs.insert(name, Output::new_uninit(output, name));
                    }

                    //tmp
                    "wl_shm" => Self::bind(&mut state.shm, proxy, name, qhandle, Self::MIN_WL_SHM_VER),

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