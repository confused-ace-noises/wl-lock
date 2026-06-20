use core::panic;
use libc::mmap;
use std::{os::fd::AsRawFd, ptr};
use wayland_client::{
    Connection, Dispatch, Proxy, WEnum,
    protocol::{
        wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard},
        wl_seat::{self, WlSeat},
    },
};
use xkbcommon::xkb::{self, Keycode};

use crate::state::{Kb, State};

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

impl Dispatch<WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlKeyboard,
        event: <WlKeyboard as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Keymap { format, fd, size } => {
                match format {
                    WEnum::Value(KeymapFormat::XkbV1) => {}
                    _ => unimplemented!(),
                }

                let keymap_string: String = unsafe {
                    let ptr = libc::mmap(
                        ptr::null_mut(),
                        size as usize,
                        libc::PROT_READ,
                        libc::MAP_PRIVATE,
                        fd.as_raw_fd(),
                        0,
                    ) as *mut u8;

                    if ptr == libc::MAP_FAILED as *mut u8 {
                        panic!("mmap failed: {}", std::io::Error::last_os_error());
                    }

                    let data = std::slice::from_raw_parts(ptr, size as usize);
                    let kmap_str = str::from_utf8(&data[..size as usize - 1])
                        .expect("server sent garbage keymap data")
                        .to_owned();

                    libc::munmap(ptr as *mut libc::c_void, size as usize);
                    kmap_str
                };

                let keymap = xkb::Keymap::new_from_string(
                    &state.input.xkb_ctx,
                    keymap_string,
                    xkb::KEYMAP_FORMAT_TEXT_V1,
                    xkb::KEYMAP_COMPILE_NO_FLAGS,
                )
                .expect("Failed to parse keymap");

                if let Some(Kb { xkb_state, .. }) = &mut state.input.keyboard {
                    xkb_state.init(xkb::State::new(&keymap));
                } else {
                    panic!("kb should already be set at this point")
                }
            }

            wl_keyboard::Event::Enter {
                ..
            } => {

            },

            wl_keyboard::Event::Leave { .. } => {
                
            },

            wl_keyboard::Event::Key {
                key,
                state: keystate,
                ..
            } => {
                match state.input.keyboard {
                    Some(_) => {},
                    None => return,
                }

                // tmp early debug exit
                state.session_lock.unlock_and_destroy();
                state.exit = Some(0);
                println!("exiting...");
                return;

                let keycode = key + 8;
                let is_pressed = keystate == WEnum::Value(KeyState::Pressed);
                let sym = state.input.keyboard.as_ref().unwrap().xkb_state.key_get_one_sym(keycode.into());

                let mods = state.input.keyboard.as_ref().unwrap().key_mods;
                
                

                let egui_key = match sym.raw() {
                    xkb::keysyms::KEY_BackSpace => Some(egui::Key::Backspace),
                    xkb::keysyms::KEY_Return | xkb::keysyms::KEY_KP_Enter => Some(egui::Key::Enter),
                    xkb::keysyms::KEY_Escape => Some(egui::Key::Escape),
                    _ => None,
                };

                if let Some(key) = egui_key {
                    state.input.events.push(egui::Event::Key { key, physical_key: None, pressed: is_pressed, repeat: false, modifiers: mods });
                }

                if is_pressed {
                    let text = state.input.keyboard.as_ref().unwrap().xkb_state.key_get_utf8(Keycode::new(keycode));
                    if !text.is_empty() && !text.chars().all(|c| c.is_control()) {
                        state.input.events.push(egui::Event::Text(text));
                    }
                }

            },

            wl_keyboard::Event::Modifiers {                
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(Kb { xkb_state, .. }) = &mut state.input.keyboard {
                    xkb_state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            },

            wl_keyboard::Event::RepeatInfo { .. } => {},
            _ => {},
        }
    }
}
