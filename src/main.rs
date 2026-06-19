use core::slice;
use std::{
    default,
    os::{
        fd::{AsFd, AsRawFd},
        raw::c_void,
    },
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::NonNull,
    time::Duration,
};

use egui::{Color32, Frame, Label, RawInput, RichText, accesskit::Color};
use egui_wgpu::RendererOptions;
use libc::{mmap, sleep};
use raw_window_handle::{
    DisplayHandle, HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use rustix::fs::{MemfdFlags, ftruncate};
use wayland_client::{
    Dispatch, Proxy, QueueHandle,
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_shm::{self, WlShm},
        wl_shm_pool::{self, WlShmPool},
        wl_surface::WlSurface,
    },
};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_surface_v1::ExtSessionLockSurfaceV1;
use wgpu::{
    BackendOptions, Backends, CompositeAlphaMode, CurrentSurfaceTexture, DeviceDescriptor,
    Instance, InstanceDescriptor, InstanceFlags, MemoryBudgetThresholds, Operations, PresentMode,
    RequestAdapterOptions, SurfaceTarget, TextureFormat, TextureUsages, TextureView,
    wgt::{SurfaceConfiguration, TextureViewDescriptor},
};
use wl_lock::{state::App, utils::late::Late};

pub struct Surface {
    pub wl_surface: WlSurface,
    pub lock_surface: ExtSessionLockSurfaceV1,
    pub name: u32,
    pub handles: WaylandHandles,
    pub height: Late<u32>,
    pub width: Late<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandDisplay {
    pub handle: WaylandDisplayHandle,
}

impl HasDisplayHandle for WaylandDisplay {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(RawDisplayHandle::Wayland(self.handle)) })
    }
}

unsafe impl Send for WaylandDisplay {}
unsafe impl Sync for WaylandDisplay {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WaylandHandles {
    pub display: WaylandDisplay,
    pub surface: NonNull<c_void>,
}

unsafe impl Send for WaylandHandles {}
unsafe impl Sync for WaylandHandles {}

impl HasDisplayHandle for WaylandHandles {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.display.display_handle()
    }
}

impl HasWindowHandle for WaylandHandles {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Wayland(WaylandWindowHandle::new(
                self.surface,
            )))
        })
    }
}

// pub struct Surface2 {
//     surface: Surface,
//     pool: WlShmPool,
//     buffer: WlBuffer,
//     chunked_thing: &'static mut [[u8; 4]],
// }

fn main() {
    let mut app = App::init();

    let qh = app.event_queue.handle();
    let display_ptr =
        NonNull::new(app.connection.backend().display_ptr() as *mut c_void).expect("Can't be null");
    let display_handle = WaylandDisplayHandle::new(display_ptr);

    let wayland_display = WaylandDisplay {
        handle: display_handle,
    };

    let session_lock = app.state.lock_manager.global.lock(&qh, ());

    let mut surfaces = app
        .state
        .outputs
        .iter()
        .map(|(name, out)| {
            let wl_surface = app.state.compositor.global.create_surface(&qh, ());
            let lock_surface =
                session_lock.get_lock_surface(&wl_surface, &out.wl_output, &qh, *name);

            let surface_ptr =
                NonNull::new(wl_surface.id().as_ptr() as *mut c_void).expect("Can't be null");
            
            Surface {
                wl_surface,
                lock_surface,
                name: *name,
                handles: WaylandHandles {
                    display: wayland_display,
                    surface: surface_ptr,
                },
                height: Late::uninit(),
                width: Late::uninit(),
            }
        })
        .collect::<Vec<_>>();

    app.event_queue.roundtrip(&mut app.state).unwrap(); // get configure for each surface

    surfaces.iter_mut().for_each(|x| {
        let output = app.state.outputs.get(&x.name).unwrap();
        *x.height = output.height;
        *x.width = output.width;
    });
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::all(),
        flags: InstanceFlags::default(),
        memory_budget_thresholds: MemoryBudgetThresholds::default(),
        backend_options: BackendOptions::default(),
        display: Some(Box::new(wayland_display)),
    });

    let wgpu_surfaces = surfaces
        .iter()
        .map(|x| {
            instance
                .create_surface(SurfaceTarget::Window(Box::new(x.handles)))
                .unwrap()
        })
        .collect::<Vec<_>>();

    let adapter = instance.request_adapter(&RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: wgpu_surfaces.first(),
        ..Default::default()
    });

    let adapter = pollster::block_on(adapter).expect("error on adapter request");

    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor::default()))
        .expect("error on device request");

    let ctx = egui::Context::default();

    let mut renderer = egui_wgpu::Renderer::new(
        &device,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        RendererOptions::default(),
    );

    for (surface, wgpu_surface) in surfaces.into_iter().zip(wgpu_surfaces) {
        wgpu_surface.configure(
            &device,
            &SurfaceConfiguration {
                usage: TextureUsages::RENDER_ATTACHMENT,
                format: TextureFormat::Bgra8UnormSrgb,
                width: *surface.width,
                height: *surface.height,
                present_mode: PresentMode::Fifo,
                desired_maximum_frame_latency: 2,
                alpha_mode: CompositeAlphaMode::Auto,
                view_formats: vec![],
            },
        );

        let output = match wgpu_surface.get_current_texture() {
            CurrentSurfaceTexture::Success(texture)
            | CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => continue,
        };

        let mut encoder = device.create_command_encoder(&Default::default());
        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [*surface.width, *surface.height],
            pixels_per_point: ctx.pixels_per_point(),
        };

        let mut raw_input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::Vec2::new(*surface.width as f32, *surface.height as f32),
            )),
            events: vec![],
            ..Default::default()
        };

        let full_output = ctx.run_ui(raw_input.take(), |ctx| {
            egui::CentralPanel::default()
                .frame(Frame {
                    fill: Color32::from_rgba_unmultiplied(224, 176, 255, 255),
                    ..Default::default()
                })
                .show_inside(ctx, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label(
                            RichText::new("Hello World!")
                                .color(Color32::from_rgba_unmultiplied(0, 0, 0, 255))
                                .size(40.),
                        );
                    })
                });
        });

        let primitives = ctx.tessellate(full_output.shapes, ctx.pixels_per_point());

        for (id, delta) in &full_output.textures_delta.set {
            renderer.update_texture(&device, &queue, *id, delta);
        }

        renderer.update_buffers(
            &device,
            &queue,
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

        drop(pass);

        queue.submit([encoder.finish()]);
        output.present();
    }

    app.event_queue.roundtrip(&mut app.state).unwrap();

    std::thread::sleep(Duration::from_secs(2));
    session_lock.unlock_and_destroy();
    app.event_queue.roundtrip(&mut app.state).unwrap();
}

// let surfaces = surfaces.into_iter().map(|surface | {
//     let Surface { wl_surface, name , ..} = &surface;

//     let output = app.state.outputs.get_mut(name).expect("Output changed name somehow?");

//     let (ptr, pool, buffer) = make_buffer("test-buffer", &qh, &app.state.shm, output.height, output.width);
//     let slice = unsafe { slice::from_raw_parts_mut(ptr, (output.height * output.width * 4) as usize) };

//     let (chunked, _) = slice.as_chunks_mut::<4>();

//     wl_surface.attach(Some(&buffer), 0, 0); // TODO: is this output-relative or is it absolute?

//     // mauve
//     chunked.iter_mut().for_each(|[b, g, r, a]| {
//         *b = 255;
//         *g = 176;
//         *r = 224;
//         *a = 255;
//     });

//     surface.wl_surface.damage(0, 0, output.width as i32, output.height as i32);

//     wl_surface.commit();

//     Surface2 {
//         buffer,
//         surface,
//         chunked_thing: chunked,
//         pool,
//     }
// }).collect::<Vec<_>>();