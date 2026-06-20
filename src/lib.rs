use std::{ffi::c_void, ptr::NonNull};

use raw_window_handle::{
    DisplayHandle, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle
};
use wayland_client::{
    Connection, Proxy, WEnum, protocol::{
        wl_output::WlOutput,
        wl_seat::{Capability, WlSeat},
        wl_surface::WlSurface,
    }
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use wgpu::Surface as WgpuSurface;
use crate::utils::late::Late;

pub mod state;
pub mod utils;

pub struct Seat {
    pub wl_seat: WlSeat,
    pub capabilities: Option<WEnum<Capability>>,
    pub name: Option<String>,
}

pub struct Output {
    pub wl_output: WlOutput,
    pub surface_info: Late<SurfaceInfo>,
    pub name: u32,
    pub configured: bool,
}

impl Output {
    pub fn new_uninit(wl_output: WlOutput, name: u32) -> Self {
        Self {
            wl_output,
            surface_info: Late::uninit(),
            name,
            configured: false,
        }
    }
}

pub struct SurfaceInfo {
    pub surface: WlSurface,
    pub lock_surface: ExtSessionLockSurfaceV1,
    pub surface_handle: WaylandSurfaceH,
    pub width: Late<u32>,
    pub height: Late<u32>,
    pub wgpu_surface: Late<WgpuSurface<'static>>
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandDisplayH(WaylandDisplayHandle);

impl WaylandDisplayH {
    // this can be meaningfull 'static because the backend of the Connection will be alive for the
    // program's duration
    pub fn new(conn: &Connection) -> Self {
        Self(WaylandDisplayHandle::new(
            NonNull::new(conn.backend().display_ptr() as *mut c_void).unwrap(),
        ))
    }
}

impl HasDisplayHandle for WaylandDisplayH {
    // this is meaningfully 'static because the backend of the connection used
    // lives for the entirety of the program
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'static>, raw_window_handle::HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Wayland(self.0)) })
    }
}

unsafe impl Send for WaylandDisplayH {}
unsafe impl Sync for WaylandDisplayH {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandSurfaceH(WaylandWindowHandle);

impl WaylandSurfaceH {
    pub fn new(wl_surface: &WlSurface) -> Self {
        Self(WaylandWindowHandle::new(
            NonNull::new(wl_surface.id().as_ptr() as *mut c_void).unwrap(),
        ))
    }
}

impl HasWindowHandle for WaylandSurfaceH {
    // this is probably 'static, becuase the program only creates one surface
    // in the duration of the program and never destroys until its end, but i'm
    // sure i'll end up shooting myself in the foot 
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'static>, raw_window_handle::HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Wayland(self.0)) })
    }
}

unsafe impl Send for WaylandSurfaceH {}
unsafe impl Sync for WaylandSurfaceH {}


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
