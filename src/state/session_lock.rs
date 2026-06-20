use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::{ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1}, ext_session_lock_v1::{self, ExtSessionLockV1}};

use crate::state::State;

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
                
                assert!(output.surface_info.is_init());

                output.surface_info.height.init(height);
                output.surface_info.width.init(width);
                
                output.configured = true;
                proxy.ack_configure(serial);
            },
            _ => unimplemented!(),
        }
    }
}