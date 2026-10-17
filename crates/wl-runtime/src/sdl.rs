//! Dynamic SDL2 presentation window and controller input backend.
//! Dynamically loads SDL2 at runtime so Wanderlust runs both interactively
//! with display/gamepad and headlessly in automated environments.

use crate::{display::DisplayServer, input::GamepadState};
use std::{
    ffi::{c_char, c_int, c_void, CString},
    ptr,
};

pub const SDL_INIT_VIDEO: u32 = 0x00000020;
pub const SDL_INIT_JOYSTICK: u32 = 0x00000200;
pub const SDL_INIT_GAMECONTROLLER: u32 = 0x00002000;
pub const SDL_INIT_EVENTS: u32 = 0x00004000;

pub const SDL_WINDOWPOS_CENTERED: c_int = 0x2FFF0000;
pub const SDL_WINDOW_SHOWN: u32 = 0x00000004;
pub const SDL_WINDOW_RESIZABLE: u32 = 0x00000020;

pub const SDL_RENDERER_ACCELERATED: u32 = 0x00000002;
pub const SDL_RENDERER_PRESENTVSYNC: u32 = 0x00000004;

pub const SDL_PIXELFORMAT_RGBA32: u32 = 0x16462004;
pub const SDL_TEXTUREACCESS_STREAMING: c_int = 1;

pub const SDL_QUIT: u32 = 0x100;
pub const SDL_KEYDOWN: u32 = 0x300;
pub const SDL_KEYUP: u32 = 0x301;
pub const SDL_CONTROLLERDEVICEADDED: u32 = 0x652;
pub const SDL_CONTROLLERDEVICEREMOVED: u32 = 0x653;

// SDL Controller Buttons
pub const SDL_CONTROLLER_BUTTON_A: c_int = 0;
pub const SDL_CONTROLLER_BUTTON_B: c_int = 1;
pub const SDL_CONTROLLER_BUTTON_X: c_int = 2;
pub const SDL_CONTROLLER_BUTTON_Y: c_int = 3;
pub const SDL_CONTROLLER_BUTTON_BACK: c_int = 4;
pub const SDL_CONTROLLER_BUTTON_GUIDE: c_int = 5;
pub const SDL_CONTROLLER_BUTTON_START: c_int = 6;
pub const SDL_CONTROLLER_BUTTON_LEFTSTICK: c_int = 7;
pub const SDL_CONTROLLER_BUTTON_RIGHTSTICK: c_int = 8;
pub const SDL_CONTROLLER_BUTTON_LEFTSHOULDER: c_int = 9;
pub const SDL_CONTROLLER_BUTTON_RIGHTSHOULDER: c_int = 10;
pub const SDL_CONTROLLER_BUTTON_DPAD_UP: c_int = 11;
pub const SDL_CONTROLLER_BUTTON_DPAD_DOWN: c_int = 12;
pub const SDL_CONTROLLER_BUTTON_DPAD_LEFT: c_int = 13;
pub const SDL_CONTROLLER_BUTTON_DPAD_RIGHT: c_int = 14;

// SDL Controller Axes
pub const SDL_CONTROLLER_AXIS_LEFTX: c_int = 0;
pub const SDL_CONTROLLER_AXIS_LEFTY: c_int = 1;
pub const SDL_CONTROLLER_AXIS_RIGHTX: c_int = 2;
pub const SDL_CONTROLLER_AXIS_RIGHTY: c_int = 3;
pub const SDL_CONTROLLER_AXIS_TRIGGERLEFT: c_int = 4;
pub const SDL_CONTROLLER_AXIS_TRIGGERRIGHT: c_int = 5;

// Scancodes
pub const SDL_SCANCODE_W: u32 = 26;
pub const SDL_SCANCODE_S: u32 = 22;
pub const SDL_SCANCODE_A: u32 = 4;
pub const SDL_SCANCODE_D: u32 = 7;
pub const SDL_SCANCODE_UP: u32 = 82;
pub const SDL_SCANCODE_DOWN: u32 = 81;
pub const SDL_SCANCODE_LEFT: u32 = 80;
pub const SDL_SCANCODE_RIGHT: u32 = 79;
pub const SDL_SCANCODE_RETURN: u32 = 40;
pub const SDL_SCANCODE_BACKSPACE: u32 = 42;
pub const SDL_SCANCODE_ESCAPE: u32 = 41;
pub const SDL_SCANCODE_TAB: u32 = 43;
pub const SDL_SCANCODE_SPACE: u32 = 44;
pub const SDL_SCANCODE_Z: u32 = 29;
pub const SDL_SCANCODE_X: u32 = 27;
pub const SDL_SCANCODE_C: u32 = 6;
pub const SDL_SCANCODE_V: u32 = 25;
pub const SDL_SCANCODE_Q: u32 = 20;
pub const SDL_SCANCODE_E: u32 = 8;
pub const SDL_SCANCODE_1: u32 = 30;
pub const SDL_SCANCODE_3: u32 = 32;

#[repr(C)]
pub struct SdlEvent {
    pub type_: u32,
    pub padding: [u8; 124],
}

type SdlInitFn = unsafe extern "C" fn(u32) -> c_int;
type SdlQuitFn = unsafe extern "C" fn();
type SdlCreateWindowFn = unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, c_int, u32) -> *mut c_void;
type SdlDestroyWindowFn = unsafe extern "C" fn(*mut c_void);
type SdlCreateRendererFn = unsafe extern "C" fn(*mut c_void, c_int, u32) -> *mut c_void;
type SdlDestroyRendererFn = unsafe extern "C" fn(*mut c_void);
type SdlCreateTextureFn = unsafe extern "C" fn(*mut c_void, u32, c_int, c_int, c_int) -> *mut c_void;
type SdlDestroyTextureFn = unsafe extern "C" fn(*mut c_void);
type SdlUpdateTextureFn = unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void, c_int) -> c_int;
type SdlRenderClearFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type SdlRenderCopyFn = unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *const c_void) -> c_int;
type SdlRenderPresentFn = unsafe extern "C" fn(*mut c_void);
type SdlPollEventFn = unsafe extern "C" fn(*mut SdlEvent) -> c_int;
type SdlNumJoysticksFn = unsafe extern "C" fn() -> c_int;
type SdlIsGameControllerFn = unsafe extern "C" fn(c_int) -> c_int;
type SdlGameControllerOpenFn = unsafe extern "C" fn(c_int) -> *mut c_void;
type SdlGameControllerCloseFn = unsafe extern "C" fn(*mut c_void);
type SdlGameControllerGetButtonFn = unsafe extern "C" fn(*mut c_void, c_int) -> u8;
type SdlGameControllerGetAxisFn = unsafe extern "C" fn(*mut c_void, c_int) -> i16;

#[cfg(unix)]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

/// Dynamic function table for SDL2.
pub struct SdlApi {
    _handle: *mut c_void,
    pub init: SdlInitFn,
    pub quit: SdlQuitFn,
    pub create_window: SdlCreateWindowFn,
    pub destroy_window: SdlDestroyWindowFn,
    pub create_renderer: SdlCreateRendererFn,
    pub destroy_renderer: SdlDestroyRendererFn,
    pub create_texture: SdlCreateTextureFn,
    pub destroy_texture: SdlDestroyTextureFn,
    pub update_texture: SdlUpdateTextureFn,
    pub render_clear: SdlRenderClearFn,
    pub render_copy: SdlRenderCopyFn,
    pub render_present: SdlRenderPresentFn,
    pub poll_event: SdlPollEventFn,
    pub num_joysticks: SdlNumJoysticksFn,
    pub is_game_controller: SdlIsGameControllerFn,
    pub game_controller_open: SdlGameControllerOpenFn,
    pub game_controller_close: SdlGameControllerCloseFn,
    pub game_controller_get_button: SdlGameControllerGetButtonFn,
    pub game_controller_get_axis: SdlGameControllerGetAxisFn,
}

impl SdlApi {
    #[cfg(unix)]
    pub fn load() -> Option<Self> {
        unsafe {
            let sdl3_names: [&[u8]; 2] = [
                b"/home/pedro/.local/lib/libSDL3.so.0\0",
                b"libSDL3.so.0\0",
            ];
            for name in sdl3_names {
                let h = dlopen(name.as_ptr() as *const c_char, 2 | 0x100);
                if !h.is_null() {
                    break;
                }
            }
        }
        let names: [&[u8]; 6] = [
            b"/home/pedro/.local/lib/libSDL2-2.0.so.0\0",
            b"/home/pedro/.local/lib/libSDL2.so\0",
            b"libSDL2-2.0.so.0\0",
            b"libSDL2-2.0.so\0",
            b"libSDL2.so\0",
            b"libSDL2.dylib\0",
        ];
        let mut handle = ptr::null_mut();
        for name in names {
            let h = unsafe { dlopen(name.as_ptr() as *const c_char, 2) }; // RTLD_NOW = 2
            if !h.is_null() {
                handle = h;
                break;
            }
        }
        if handle.is_null() {
            eprintln!("[sdl] Could not locate SDL2 shared library");
            return None;
        }

        unsafe {
            macro_rules! sym {
                ($name:ident, $type:ty) => {{
                    let sym_name = CString::new(stringify!($name)).ok()?;
                    let ptr = dlsym(handle, sym_name.as_ptr());
                    if ptr.is_null() {
                        dlclose(handle);
                        return None;
                    }
                    std::mem::transmute::<*mut c_void, $type>(ptr)
                }};
            }

            Some(Self {
                _handle: handle,
                init: sym!(SDL_Init, SdlInitFn),
                quit: sym!(SDL_Quit, SdlQuitFn),
                create_window: sym!(SDL_CreateWindow, SdlCreateWindowFn),
                destroy_window: sym!(SDL_DestroyWindow, SdlDestroyWindowFn),
                create_renderer: sym!(SDL_CreateRenderer, SdlCreateRendererFn),
                destroy_renderer: sym!(SDL_DestroyRenderer, SdlDestroyRendererFn),
                create_texture: sym!(SDL_CreateTexture, SdlCreateTextureFn),
                destroy_texture: sym!(SDL_DestroyTexture, SdlDestroyTextureFn),
                update_texture: sym!(SDL_UpdateTexture, SdlUpdateTextureFn),
                render_clear: sym!(SDL_RenderClear, SdlRenderClearFn),
                render_copy: sym!(SDL_RenderCopy, SdlRenderCopyFn),
                render_present: sym!(SDL_RenderPresent, SdlRenderPresentFn),
                poll_event: sym!(SDL_PollEvent, SdlPollEventFn),
                num_joysticks: sym!(SDL_NumJoysticks, SdlNumJoysticksFn),
                is_game_controller: sym!(SDL_IsGameController, SdlIsGameControllerFn),
                game_controller_open: sym!(SDL_GameControllerOpen, SdlGameControllerOpenFn),
                game_controller_close: sym!(SDL_GameControllerClose, SdlGameControllerCloseFn),
                game_controller_get_button: sym!(SDL_GameControllerGetButton, SdlGameControllerGetButtonFn),
                game_controller_get_axis: sym!(SDL_GameControllerGetAxis, SdlGameControllerGetAxisFn),
            })
        }
    }

    #[cfg(not(unix))]
    pub fn load() -> Option<Self> {
        None
    }
}

/// Active SDL Presentation window and GameController polling host.
pub struct SdlHost {
    pub api: SdlApi,
    pub window: *mut c_void,
    pub renderer: *mut c_void,
    pub texture: *mut c_void,
    pub controller: *mut c_void,
    pub width: u32,
    pub height: u32,
    pub gamepad: GamepadState,
    pub should_close: bool,
}

impl SdlHost {
    /// Attempts to open an SDL2 presentation window (default 1280x720).
    pub fn open(title: &str, width: u32, height: u32) -> Option<Self> {
        let api = SdlApi::load()?;
        unsafe {
            if (api.init)(SDL_INIT_VIDEO | SDL_INIT_GAMECONTROLLER | SDL_INIT_EVENTS) != 0 {
                eprintln!("[sdl] SDL_Init failed");
                return None;
            }
            let c_title = CString::new(title).unwrap_or_default();
            let window = (api.create_window)(
                c_title.as_ptr(),
                SDL_WINDOWPOS_CENTERED,
                SDL_WINDOWPOS_CENTERED,
                width as c_int,
                height as c_int,
                SDL_WINDOW_SHOWN | SDL_WINDOW_RESIZABLE,
            );
            if window.is_null() {
                eprintln!("[sdl] SDL_CreateWindow failed");
                (api.quit)();
                return None;
            }
            let mut renderer = (api.create_renderer)(window, -1, SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC);
            if renderer.is_null() {
                renderer = (api.create_renderer)(window, -1, 0);
            }
            if renderer.is_null() {
                eprintln!("[sdl] SDL_CreateRenderer failed");
                (api.destroy_window)(window);
                (api.quit)();
                return None;
            }
            let texture = (api.create_texture)(
                renderer,
                SDL_PIXELFORMAT_RGBA32,
                SDL_TEXTUREACCESS_STREAMING,
                width as c_int,
                height as c_int,
            );
            if texture.is_null() {
                eprintln!("[sdl] SDL_CreateTexture failed");
                (api.destroy_renderer)(renderer);
                (api.destroy_window)(window);
                (api.quit)();
                return None;
            }
            eprintln!("[sdl] Presentation window opened: '{title}' ({width}x{height})");

            // Open first available GameController
            let mut controller = ptr::null_mut();
            let num = (api.num_joysticks)();
            for i in 0..num {
                if (api.is_game_controller)(i) != 0 {
                    let c = (api.game_controller_open)(i);
                    if !c.is_null() {
                        controller = c;
                        break;
                    }
                }
            }

            Some(Self {
                api,
                window,
                renderer,
                texture,
                controller,
                width,
                height,
                gamepad: GamepadState::default(),
                should_close: false,
            })
        }
    }

    /// Polls SDL events and game controllers, updating the internal gamepad state.
    pub fn poll_input(&mut self) -> &GamepadState {
        let mut event = SdlEvent {
            type_: 0,
            padding: [0; 124],
        };

        unsafe {
            while (self.api.poll_event)(&mut event) != 0 {
                match event.type_ {
                    SDL_QUIT => {
                        self.should_close = true;
                    }
                    SDL_CONTROLLERDEVICEADDED => {
                        if self.controller.is_null() {
                            let num = (self.api.num_joysticks)();
                            for i in 0..num {
                                if (self.api.is_game_controller)(i) != 0 {
                                    let c = (self.api.game_controller_open)(i);
                                    if !c.is_null() {
                                        self.controller = c;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    SDL_CONTROLLERDEVICEREMOVED => {
                        if !self.controller.is_null() {
                            (self.api.game_controller_close)(self.controller);
                            self.controller = ptr::null_mut();
                        }
                    }
                    SDL_KEYDOWN | SDL_KEYUP => {
                        let pressed = event.type_ == SDL_KEYDOWN;
                        // scancode is at offset 16 in SDL_KeyboardEvent
                        let scancode = u32::from_le_bytes(event.padding[12..16].try_into().unwrap_or_default());
                        match scancode {
                            SDL_SCANCODE_W => self.gamepad.thumb_ly = if pressed { 32767 } else { 0 },
                            SDL_SCANCODE_S => self.gamepad.thumb_ly = if pressed { -32767 } else { 0 },
                            SDL_SCANCODE_A => self.gamepad.thumb_lx = if pressed { -32767 } else { 0 },
                            SDL_SCANCODE_D => self.gamepad.thumb_lx = if pressed { 32767 } else { 0 },
                            SDL_SCANCODE_UP => self.gamepad.dpad_up = pressed,
                            SDL_SCANCODE_DOWN => self.gamepad.dpad_down = pressed,
                            SDL_SCANCODE_LEFT => self.gamepad.dpad_left = pressed,
                            SDL_SCANCODE_RIGHT => self.gamepad.dpad_right = pressed,
                            SDL_SCANCODE_RETURN | SDL_SCANCODE_Z => self.gamepad.b = pressed, // Nintendo A
                            SDL_SCANCODE_BACKSPACE | SDL_SCANCODE_X => self.gamepad.a = pressed, // Nintendo B
                            SDL_SCANCODE_C => self.gamepad.y = pressed, // Nintendo X
                            SDL_SCANCODE_V => self.gamepad.x = pressed, // Nintendo Y
                            SDL_SCANCODE_Q => self.gamepad.left_shoulder = pressed,
                            SDL_SCANCODE_E => self.gamepad.right_shoulder = pressed,
                            SDL_SCANCODE_1 => self.gamepad.left_trigger = if pressed { 255 } else { 0 },
                            SDL_SCANCODE_3 => self.gamepad.right_trigger = if pressed { 255 } else { 0 },
                            SDL_SCANCODE_SPACE => self.gamepad.start = pressed, // Plus
                            SDL_SCANCODE_TAB | SDL_SCANCODE_ESCAPE => self.gamepad.back = pressed, // Minus
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }

            // If game controller is connected, poll controller directly
            if !self.controller.is_null() {
                let btn = |b| (self.api.game_controller_get_button)(self.controller, b) != 0;
                let axis = |a| (self.api.game_controller_get_axis)(self.controller, a);

                self.gamepad.a = btn(SDL_CONTROLLER_BUTTON_A);
                self.gamepad.b = btn(SDL_CONTROLLER_BUTTON_B);
                self.gamepad.x = btn(SDL_CONTROLLER_BUTTON_X);
                self.gamepad.y = btn(SDL_CONTROLLER_BUTTON_Y);
                self.gamepad.start = btn(SDL_CONTROLLER_BUTTON_START);
                self.gamepad.back = btn(SDL_CONTROLLER_BUTTON_BACK);
                self.gamepad.guide = btn(SDL_CONTROLLER_BUTTON_GUIDE);
                self.gamepad.left_shoulder = btn(SDL_CONTROLLER_BUTTON_LEFTSHOULDER);
                self.gamepad.right_shoulder = btn(SDL_CONTROLLER_BUTTON_RIGHTSHOULDER);
                self.gamepad.left_thumb = btn(SDL_CONTROLLER_BUTTON_LEFTSTICK);
                self.gamepad.right_thumb = btn(SDL_CONTROLLER_BUTTON_RIGHTSTICK);
                self.gamepad.dpad_up = btn(SDL_CONTROLLER_BUTTON_DPAD_UP);
                self.gamepad.dpad_down = btn(SDL_CONTROLLER_BUTTON_DPAD_DOWN);
                self.gamepad.dpad_left = btn(SDL_CONTROLLER_BUTTON_DPAD_LEFT);
                self.gamepad.dpad_right = btn(SDL_CONTROLLER_BUTTON_DPAD_RIGHT);

                self.gamepad.thumb_lx = axis(SDL_CONTROLLER_AXIS_LEFTX);
                self.gamepad.thumb_ly = -axis(SDL_CONTROLLER_AXIS_LEFTY); // SDL Y is down, Switch Y is up
                self.gamepad.thumb_rx = axis(SDL_CONTROLLER_AXIS_RIGHTX);
                self.gamepad.thumb_ry = -axis(SDL_CONTROLLER_AXIS_RIGHTY);

                let lt = axis(SDL_CONTROLLER_AXIS_TRIGGERLEFT);
                self.gamepad.left_trigger = (lt.max(0) as u32 * 255 / 32767) as u8;
                let rt = axis(SDL_CONTROLLER_AXIS_TRIGGERRIGHT);
                self.gamepad.right_trigger = (rt.max(0) as u32 * 255 / 32767) as u8;
            }
        }
        &self.gamepad
    }

    /// Presents the latest frame from the display server onto the SDL window.
    pub fn present_frame(&mut self, display: &DisplayServer) {
        unsafe {
            if let Some(frame) = &display.current_frame {
                let rgba = frame.to_rgba8888();
                let pitch = (frame.width * 4) as c_int;
                (self.api.update_texture)(
                    self.texture,
                    ptr::null(),
                    rgba.as_ptr() as *const c_void,
                    pitch,
                );
                (self.api.render_clear)(self.renderer);
                (self.api.render_copy)(self.renderer, self.texture, ptr::null(), ptr::null());
            } else {
                (self.api.render_clear)(self.renderer);
            }
            (self.api.render_present)(self.renderer);
        }
    }

    /// Presents a raw RGBA/BGRA byte slice (width * height * 4).
    pub fn present_raw_rgba(&mut self, rgba: &[u8], width: u32, _height: u32) {
        let pitch = (width * 4) as c_int;
        unsafe {
            (self.api.update_texture)(
                self.texture,
                ptr::null(),
                rgba.as_ptr() as *const c_void,
                pitch,
            );
            (self.api.render_clear)(self.renderer);
            (self.api.render_copy)(self.renderer, self.texture, ptr::null(), ptr::null());
            (self.api.render_present)(self.renderer);
        }
    }
}

impl Drop for SdlHost {
    fn drop(&mut self) {
        unsafe {
            if !self.controller.is_null() {
                (self.api.game_controller_close)(self.controller);
            }
            if !self.texture.is_null() {
                (self.api.destroy_texture)(self.texture);
            }
            if !self.renderer.is_null() {
                (self.api.destroy_renderer)(self.renderer);
            }
            if !self.window.is_null() {
                (self.api.destroy_window)(self.window);
            }
            (self.api.quit)();
        }
    }
}
