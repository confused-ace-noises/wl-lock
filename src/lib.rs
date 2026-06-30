use std::{ffi::c_void, mem, ptr::NonNull};

use egui::Ui;
use raw_window_handle::{
    DisplayHandle, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle
};
use wayland_client::{
    Connection, Proxy, WEnum, protocol::{
        wl_output::WlOutput, wl_seat::{Capability, WlSeat}, wl_surface::WlSurface,
    }
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use wgpu::{CurrentSurfaceTexture, Operations, Surface as WgpuSurface, TextureViewDescriptor};
use crate::{state::{App, PointerEvent}, utils::late::Late};

pub mod state;
pub mod utils;
pub mod widgets;

pub struct Seat {
    pub wl_seat: WlSeat,
    pub capabilities: Option<WEnum<Capability>>,
    pub name: Option<String>,
}

pub struct Output {
    pub egui_context: Late<egui::Context>,
    pub events_to_flush: Vec<egui::Event>,
    pub pointer_events: Vec<PointerEvent>,
    pub last_pointer_axis_event: Option<usize>,
    pub wl_output: WlOutput,
    pub surface_info: Late<SurfaceInfo>,
    pub name: u32,
    pub display_name: Late<String>,
    pub configured: bool,
}

impl Output {
    pub fn new_uninit(wl_output: WlOutput, name: u32) -> Self {
        Self {
            egui_context: Late::uninit(),
            pointer_events: Vec::new(),
            events_to_flush: Vec::new(),
            last_pointer_axis_event: None,
            wl_output,
            surface_info: Late::uninit(),
            name,
            display_name: Late::uninit(),
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

pub enum TryExit {
    None,
    Force, 
    PasswdCheck(String),
}

impl App {
    pub fn ui(&mut self, mut output_fn: impl for<'a> FnMut(&String, &'a mut Ui) -> TryExit) {
        let mut should_break: bool;
        let mut should_auth: Option<String>;

        loop {
            should_break = false;
            should_auth = None;

            self.send_frame_req();

            for (_, output) in self.state.outputs.iter_mut() {
                let mut exit: TryExit = TryExit::None;

                let display_name = &*output.display_name;
                
                let run_ui = coerce_hrtb(|ui| {
                    exit = output_fn(display_name, ui);
                });

                {
                    let device = &self.state.wgpu.device;
                    // let output = output;
                    let wgpu_surface = &output.surface_info.wgpu_surface;
                    let ctx = &output.egui_context;

                    let qh = &self.event_queue.handle();

                    output.surface_info.surface.frame(qh, ());

                    let width = *output.surface_info.width;
                    let height = *output.surface_info.height;

                    if !(self.state.new_events || output.egui_context.has_requested_repaint()) {
                        continue;
                    }

                    let surface_texture = match wgpu_surface.get_current_texture() {
                        CurrentSurfaceTexture::Success(texture) => texture,
                        CurrentSurfaceTexture::Suboptimal(texture) => {
                            // wgpu_surface.configure(&self.state.wgpu.device, &Self::wgpu_surface_config(width, height));
                            texture
                        }
                        _ => continue,
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
                }

                // drop((output, name));

                match exit {
                    TryExit::None => {},
                    TryExit::Force => should_break = true,
                    TryExit::PasswdCheck(pwd) => {
                        should_auth = Some(pwd);
                    },
                }
            }

            if should_break {
                break;
            } else if let Some(pwd) = should_auth && self.pam_auth(&pwd) {
                break;
            }
        }

        self.event_queue.roundtrip(&mut self.state).unwrap();

        self.state.session_lock.unlock_and_destroy();
        self.event_queue.roundtrip(&mut self.state).unwrap();
    }
}

fn coerce_hrtb<F: for<'a> FnMut(&'a mut egui::Ui)>(f: F) -> F { f }