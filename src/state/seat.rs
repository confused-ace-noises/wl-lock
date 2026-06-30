use core::panic;
use egui::{Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2};
use std::{os::fd::AsRawFd, ptr};
use wayland_client::{
    Connection, Dispatch, Proxy, WEnum,
    protocol::{
        wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard},
        wl_pointer::{self, Axis, AxisSource, ButtonState, WlPointer},
        wl_seat::{self, WlSeat},
        wl_surface::WlSurface,
    },
};
use xkbcommon::xkb::{self, Keycode};

use crate::{
    Output,
    state::{Kb, PointerEvent, State},
};

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

            wl_keyboard::Event::Enter { surface, .. } => {
                let output = surface_to_output(state, &surface);

                output
                    .events_to_flush
                    .push(egui::Event::WindowFocused(true));
                state.input.keyboard.as_mut().unwrap().focused_output = Some(output.name);
            }

            wl_keyboard::Event::Leave { surface, .. } => {
                let output = surface_to_output(state, &surface);

                output
                    .events_to_flush
                    .push(egui::Event::WindowFocused(false));

                state.input.keyboard.as_mut().unwrap().focused_output = None;
            }

            wl_keyboard::Event::Key {
                key,
                state: keystate,
                ..
            } => {
                match state.input.keyboard {
                    Some(_) => {}
                    None => return,
                }

                // tmp early debug exit
                // state.session_lock.unlock_and_destroy();
                // state.exit = Some(0);
                // println!("exiting...");
                // return;

                let Some(output_name) = state.input.keyboard.as_ref().unwrap().focused_output
                else {
                    return;
                };

                let output = state.outputs.get_mut(&output_name).unwrap();

                let keycode = key + 8;
                let is_pressed = keystate == WEnum::Value(KeyState::Pressed);
                let sym = state
                    .input
                    .keyboard
                    .as_ref()
                    .unwrap()
                    .xkb_state
                    .key_get_one_sym(keycode.into());

                let mods = state.input.keyboard.as_ref().unwrap().key_mods;

                let egui_key = match sym.raw() {
                    xkb::keysyms::KEY_BackSpace => Some(egui::Key::Backspace),
                    xkb::keysyms::KEY_Return | xkb::keysyms::KEY_KP_Enter => Some(egui::Key::Enter),
                    xkb::keysyms::KEY_Escape => Some(egui::Key::Escape),
                    _ => None,
                };

                if let Some(key) = egui_key {
                    output.events_to_flush.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: is_pressed,
                        repeat: false,
                        modifiers: mods,
                    });
                }

                if is_pressed {
                    let text = state
                        .input
                        .keyboard
                        .as_ref()
                        .unwrap()
                        .xkb_state
                        .key_get_utf8(Keycode::new(keycode));
                    if !text.is_empty() && !text.chars().all(|c| c.is_control()) {
                        output.events_to_flush.push(egui::Event::Text(text));
                    }
                }
            }

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
            }

            wl_keyboard::Event::RepeatInfo { .. } => {} // TODO: do repeating
            _ => {}
        }

        state.new_events = true;
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _proxy: &WlPointer,
        event: <WlPointer as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &wayland_client::QueueHandle<Self>,
    ) {
        fn output(state: &mut State) -> &mut Output {
            state
                .outputs
                .get_mut(
                    &state
                        .input
                        .pointer
                        .as_ref()
                        .unwrap()
                        .last_focused_output_in_events
                        .unwrap(),
                )
                .unwrap()
        }

        if !matches!(event, wl_pointer::Event::Frame) {
            match event {
                wl_pointer::Event::Enter { ref surface, .. } => {
                    let output = surface_to_output(state, surface);
                    output.pointer_events.push(PointerEvent::Event(event));
                    state
                        .input
                        .pointer
                        .as_mut()
                        .unwrap()
                        .last_focused_output_in_events = Some(output.name);
                }

                wl_pointer::Event::Leave { .. } => {
                    let last_focus = &mut state
                        .input
                        .pointer
                        .as_mut()
                        .unwrap()
                        .last_focused_output_in_events;
                    let output = state
                        .outputs
                        .get_mut(&last_focus.expect("server sent two leave events?"))
                        .unwrap();
                    output.pointer_events.push(PointerEvent::Event(event));
                    *last_focus = None;
                }

                wl_pointer::Event::Axis { .. }
                | wl_pointer::Event::AxisDiscrete { .. }
                | wl_pointer::Event::AxisValue120 { .. }
                | wl_pointer::Event::AxisStop { .. }
                | wl_pointer::Event::AxisSource { .. } 
                | wl_pointer::Event::AxisRelativeDirection { .. } => {
                    let output = output(state);

                    let PointerEvent::Axis {
                        mut ordered_ev,
                        mut source,
                        mut available_modes,
                        mut is_stop
                    } = output
                        .last_pointer_axis_event
                        .and_then(|x| {
                            if output.pointer_events.len() > x {
                                Some(output.pointer_events.remove(x))
                            } else {
                                eprintln!("fuck");
                                None
                            }
                        })
                        .unwrap_or(PointerEvent::Axis {
                            ordered_ev: Vec::new(),
                            source: None,
                            available_modes: 0,
                            is_stop: None,
                        })
                    else {
                        unreachable!()
                    };

                    let new_index = output.pointer_events.len();
                    println!("ev: {event:?}");
                    match event {
                        wl_pointer::Event::Axis { .. } => available_modes |= 0b00000001,
                        wl_pointer::Event::AxisValue120 { .. } => available_modes |= 0b00000010,
                        wl_pointer::Event::AxisDiscrete { .. } => available_modes |= 0b00000100,
                        wl_pointer::Event::AxisSource {
                            axis_source: WEnum::Value(src),
                        } => source = Some(src),
                        wl_pointer::Event::AxisStop { .. } => {
                            if is_stop.is_none() {
                                is_stop = Some(false);
                            } else if !is_stop.unwrap() {
                                is_stop = Some(true);
                            }
                        } 
                        _ => {}
                    }
                    ordered_ev.push(event);
                    output.pointer_events.push(PointerEvent::Axis {
                        ordered_ev,
                        source,
                        available_modes,
                        is_stop
                    });
                    output.last_pointer_axis_event = Some(new_index);
                }

                wl_pointer::Event::Frame => unreachable!("can't be here"),

                event => {
                    let output = output(state);
                    output.pointer_events.push(PointerEvent::Event(event));
                }
            }
            return;
        }

        for (_, output) in state.outputs.iter_mut() {
            for event in output.pointer_events.drain(..) {
                match event {
                    PointerEvent::Event(event) => match event {
                        wl_pointer::Event::Enter { .. } => {
                            output
                                .events_to_flush
                                .push(egui::Event::WindowFocused(true));
                            state.input.pointer.as_mut().unwrap().focused_output =
                                Some(output.name);
                        }

                        wl_pointer::Event::Leave { .. } => {
                            output
                                .events_to_flush
                                .push(egui::Event::WindowFocused(false));
                            state.input.pointer.as_mut().unwrap().focused_output = None;
                        }

                        wl_pointer::Event::Motion {
                            time: _,
                            surface_x,
                            surface_y,
                        } => {
                            state.input.pointer.as_mut().unwrap().last_pointer_pos = Some((surface_x as f32, surface_y as f32));
                            output.events_to_flush.push(egui::Event::PointerMoved(Pos2 {
                                x: surface_x as f32,
                                y: surface_y as f32,
                            }));
                        }

                        wl_pointer::Event::Button {
                            serial: _,
                            time: _,
                            button,
                            state: button_state,
                        } => {
                            let modifiers = state
                                .input
                                .keyboard
                                .as_ref()
                                .map(|x| x.key_mods)
                                .unwrap_or(Modifiers::NONE);

                            let pointer = state.input.pointer.as_mut().unwrap();

                            let pressed =
                                matches!(button_state, WEnum::Value(ButtonState::Pressed));

                            let button = match button {
                                272 => PointerButton::Primary,
                                273 => PointerButton::Secondary,
                                274 => PointerButton::Middle,
                                275 => PointerButton::Extra1,
                                276 => PointerButton::Extra2,
                                _ => return, // unimplemented
                            };

                            if let Some(pos) = pointer.last_pointer_pos {
                                output.events_to_flush.push(egui::Event::PointerButton {
                                    pos: pos.into(),
                                    button,
                                    pressed,
                                    modifiers,
                                });
                            }

                        }
                        a => unimplemented!("{a:?}"),
                    },

                    PointerEvent::Axis {
                        ordered_ev,
                        source,
                        available_modes,
                        is_stop
                    } => {
                        let is_axis120 = || available_modes & 0b0000010 == 0b0000010;
                        let is_axis_discrete = || available_modes & 0b0000100 == 0b0000100;
                        let is_axis = || available_modes & 0b0000001 == 0b0000001;

                        println!("{available_modes:b}");

                        match source {
                            Some(AxisSource::Wheel) => {
                                // maybe fix this?
                                let delta: Vec2 = if is_axis120() {
                                    calculate_delta(ordered_ev, |ev| {
                                        if let wl_pointer::Event::AxisValue120 {
                                            axis: WEnum::Value(axis),
                                            value120,
                                        } = ev
                                        {
                                            Some((axis, (*value120 as f32 / 120.)))
                                        } else {
                                            None
                                        }
                                    })
                                } else if is_axis_discrete() {
                                    calculate_delta(ordered_ev, |ev| {
                                        if let wl_pointer::Event::AxisDiscrete {
                                            axis: WEnum::Value(axis),
                                            discrete,
                                        } = ev
                                        {
                                            Some((axis, *discrete as f32))
                                        } else {
                                            None
                                        }
                                    })
                                } else if is_axis() {
                                    calculate_delta(ordered_ev, |ev| {
                                        if let wl_pointer::Event::Axis {
                                            axis: WEnum::Value(axis),
                                            value,
                                            ..
                                        } = ev
                                        {
                                            Some((axis, (*value as f32 / 15.)))
                                        } else {
                                            None
                                        }
                                    })
                                } else {
                                    eprintln!("what should i do then???");
                                    Vec2 { x: 0., y: 0. }
                                };

                                let mut should_be_stop = false;

                                if (delta.x == 0. || delta.y == 0.) && is_stop.is_some() {
                                    should_be_stop = true
                                } else if is_stop.is_some() {
                                    should_be_stop = is_stop.unwrap();
                                }

                                output.events_to_flush.push(egui::Event::MouseWheel {
                                    unit: MouseWheelUnit::Line,
                                    delta,
                                    phase: if should_be_stop { TouchPhase::End } else { TouchPhase::Move },
                                    modifiers: state.input.keyboard.as_ref().unwrap().key_mods,
                                });
                                output.last_pointer_axis_event = None;
                            }
                            Some(AxisSource::Continuous) | Some(AxisSource::Finger) | None => {
                                let delta = if is_axis() {
                                    calculate_delta(ordered_ev, |ev| {
                                        if let wl_pointer::Event::Axis {
                                            axis: WEnum::Value(axis),
                                            value,
                                            ..
                                        } = ev
                                        {
                                            Some((axis, *value as f32))
                                        } else {
                                            None
                                        }
                                    })
                                } else {
                                    Vec2::ZERO // TODO
                                };

                                let mut should_be_stop = false;

                                if (delta.x == 0. || delta.y == 0.) && is_stop.is_some() {
                                    should_be_stop = true
                                } else if is_stop.is_some() {
                                    should_be_stop = is_stop.unwrap();
                                }

                                output.events_to_flush.push(egui::Event::MouseWheel {
                                    unit: MouseWheelUnit::Point,
                                    delta,
                                    phase: if should_be_stop { TouchPhase::End } else { TouchPhase::Move },
                                    modifiers: state.input.keyboard.as_ref().unwrap().key_mods,
                                });
                            },

                            Some(AxisSource::WheelTilt) => {
                                eprintln!("no idea how to handle this");
                            },

                            Some(_) => todo!(),
                        }
                    }
                }
            }
        }

        state.new_events = true;
    }
}

fn calculate_delta(
    events: Vec<wl_pointer::Event>,
    f: impl for<'a> FnMut(&'a wl_pointer::Event) -> Option<(&'a Axis, f32)>,
) -> Vec2 {
    events
        .iter()
        .filter_map(f)
        .fold(Vec2::ZERO, |mut init, (axis, amount)| {
            match axis {
                wl_pointer::Axis::VerticalScroll => init.y += amount,
                wl_pointer::Axis::HorizontalScroll => init.x += amount,
                _ => unimplemented!(),
            }

            init
        })
}

fn surface_to_output<'a>(state: &'a mut State, surface: &WlSurface) -> &'a mut Output {
    state
        .outputs
        .values_mut()
        .find(|output| output.surface_info.surface == *surface)
        .expect("wl_keyboard::Event passed a surface the client doesn't own")
}
