use std::{collections::HashMap, io::ErrorKind::ConnectionAborted};

use wayland_client::{Connection, EventQueue, globals::registry_queue_init, protocol::wl_output::WlOutput};
use wl_lock::{App, utils::late::Late};

fn main() {
    let mut app = App::init();

    let qh = app.event_queue.handle();

    let session_lock = app.state.lock_manager.global.lock(&qh, ());

    
    for (name, output) in app.state.outputs {
        let wl_surface = app.state.compositor.global.create_surface(&qh, ());
        let lock_surface = session_lock.get_lock_surface(&wl_surface, &output.wl_output, &qh, name);
        
    }

}
