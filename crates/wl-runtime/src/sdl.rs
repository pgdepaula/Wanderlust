//! Dynamic SDL2 presentation window and controller input backend.
//! Dynamically loads SDL2 at runtime so Wanderlust runs both interactively
//! with display/gamepad and headlessly in automated environments.

use crate::{display::DisplayServer, input::GamepadState};
use std::{
    ffi::{CString, c_char, c_int, c_void},
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

// SDL2 release-2.30.11 SDL_pixels.h: byte-array RGBA32 aliases ABGR8888
// on little-endian hosts and RGBA8888 on big-endian hosts.
#[cfg(target_endian = "little")]
pub const SDL_PIXELFORMAT_RGBA32: u32 = 0x16762004;
#[cfg(target_endian = "big")]
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
type SdlCreateWindowFn =
    unsafe extern "C" fn(*const c_char, c_int, c_int, c_int, c_int, u32) -> *mut c_void;
type SdlSetWindowTitleFn = unsafe extern "C" fn(*mut c_void, *const c_char);
type SdlDestroyWindowFn = unsafe extern "C" fn(*mut c_void);
type SdlCreateRendererFn = unsafe extern "C" fn(*mut c_void, c_int, u32) -> *mut c_void;
type SdlDestroyRendererFn = unsafe extern "C" fn(*mut c_void);
type SdlCreateTextureFn =
    unsafe extern "C" fn(*mut c_void, u32, c_int, c_int, c_int) -> *mut c_void;
type SdlDestroyTextureFn = unsafe extern "C" fn(*mut c_void);
type SdlUpdateTextureFn =
    unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void, c_int) -> c_int;
type SdlRenderClearFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type SdlRenderCopyFn =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void, *const c_void) -> c_int;
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

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryExW(path: *const u16, file: *mut c_void, flags: u32) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn FreeLibrary(module: *mut c_void) -> c_int;
}

/// Dynamic function table for SDL2.
pub struct SdlApi {
    _handle: *mut c_void,
    pub init: SdlInitFn,
    pub quit: SdlQuitFn,
    pub create_window: SdlCreateWindowFn,
    pub destroy_window: SdlDestroyWindowFn,
    pub set_window_title: SdlSetWindowTitleFn,
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
            let sdl3_names: [&[u8]; 2] =
                [b"/home/pedro/.local/lib/libSDL3.so.0\0", b"libSDL3.so.0\0"];
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
                set_window_title: sym!(SDL_SetWindowTitle, SdlSetWindowTitleFn),
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
                game_controller_get_button: sym!(
                    SDL_GameControllerGetButton,
                    SdlGameControllerGetButtonFn
                ),
                game_controller_get_axis: sym!(
                    SDL_GameControllerGetAxis,
                    SdlGameControllerGetAxisFn
                ),
            })
        }
    }

    #[cfg(windows)]
    pub fn load() -> Option<Self> {
        use std::{os::windows::ffi::OsStrExt, path::PathBuf};
        let path = std::env::var_os("WL_SDL2_LIBRARY")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()
                    .map(|p| p.join("SDL2.dll"))
            })?
            .canonicalize()
            .ok()?;
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return None;
        }
        wide.push(0);
        // Absolute path plus DLL_LOAD_DIR/DEFAULT_DIRS: do not search an
        // arbitrary guest-input working directory or PATH for SDL2.dll.
        let handle = unsafe { LoadLibraryExW(wide.as_ptr(), ptr::null_mut(), 0x100 | 0x1000) };
        if handle.is_null() {
            return None;
        }
        unsafe {
            macro_rules! sym {
                ($name:ident, $type:ty) => {{
                    let name = CString::new(stringify!($name)).ok()?;
                    let pointer = GetProcAddress(handle, name.as_ptr());
                    if pointer.is_null() {
                        FreeLibrary(handle);
                        return None;
                    }
                    std::mem::transmute::<*mut c_void, $type>(pointer)
                }};
            }
            Some(Self {
                _handle: handle,
                init: sym!(SDL_Init, SdlInitFn),
                quit: sym!(SDL_Quit, SdlQuitFn),
                create_window: sym!(SDL_CreateWindow, SdlCreateWindowFn),
                destroy_window: sym!(SDL_DestroyWindow, SdlDestroyWindowFn),
                set_window_title: sym!(SDL_SetWindowTitle, SdlSetWindowTitleFn),
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
                game_controller_get_button: sym!(
                    SDL_GameControllerGetButton,
                    SdlGameControllerGetButtonFn
                ),
                game_controller_get_axis: sym!(
                    SDL_GameControllerGetAxis,
                    SdlGameControllerGetAxisFn
                ),
            })
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn load() -> Option<Self> {
        None
    }
}

#[cfg(windows)]
impl Drop for SdlApi {
    fn drop(&mut self) {
        if self._handle.is_null() {
            return;
        }
        unsafe {
            FreeLibrary(self._handle);
        }
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
    /// Updates host diagnostics without creating a guest frame or GPU command.
    pub fn set_title(&mut self, title: &str) -> bool {
        let Ok(title) = CString::new(title) else {
            return false;
        };
        unsafe {
            (self.api.set_window_title)(self.window, title.as_ptr());
        }
        true
    }
    /// Attempts to open an SDL2 presentation window (default 1280x720).
    pub fn open(title: &str, width: u32, height: u32) -> Option<Self> {
        checked_rgba_layout(width, height)?;
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
            let mut renderer = (api.create_renderer)(
                window,
                -1,
                SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC,
            );
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
                        let scancode = u32::from_le_bytes(
                            event.padding[12..16].try_into().unwrap_or_default(),
                        );
                        match scancode {
                            SDL_SCANCODE_W => {
                                self.gamepad.thumb_ly = if pressed { 32767 } else { 0 }
                            }
                            SDL_SCANCODE_S => {
                                self.gamepad.thumb_ly = if pressed { -32767 } else { 0 }
                            }
                            SDL_SCANCODE_A => {
                                self.gamepad.thumb_lx = if pressed { -32767 } else { 0 }
                            }
                            SDL_SCANCODE_D => {
                                self.gamepad.thumb_lx = if pressed { 32767 } else { 0 }
                            }
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
                            SDL_SCANCODE_1 => {
                                self.gamepad.left_trigger = if pressed { 255 } else { 0 }
                            }
                            SDL_SCANCODE_3 => {
                                self.gamepad.right_trigger = if pressed { 255 } else { 0 }
                            }
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

    /// Presents a validated display frame; malformed input never reaches SDL.
    pub fn present_frame(&mut self, display: &DisplayServer) -> bool {
        if let Some(frame) = &display.current_frame {
            let Ok(rgba) = frame.to_rgba8888() else {
                return false;
            };
            self.present_raw_rgba(&rgba, frame.width, frame.height)
        } else {
            unsafe {
                if (self.api.render_clear)(self.renderer) != 0 {
                    return false;
                }
                (self.api.render_present)(self.renderer);
            }
            true
        }
    }

    /// Presents exactly width*height*4 packed RGBA bytes. Textures are resized
    /// before upload so SDL's null rectangle cannot read beyond the source.
    pub fn present_raw_rgba(&mut self, rgba: &[u8], width: u32, height: u32) -> bool {
        let Some((w, h, pitch, size)) = checked_rgba_layout(width, height) else {
            return false;
        };
        if rgba.len() != size {
            return false;
        }
        unsafe {
            let resized = self.width != width || self.height != height;
            let texture = if resized {
                (self.api.create_texture)(
                    self.renderer,
                    SDL_PIXELFORMAT_RGBA32,
                    SDL_TEXTUREACCESS_STREAMING,
                    w,
                    h,
                )
            } else {
                self.texture
            };
            if texture.is_null() {
                return false;
            }
            if (self.api.update_texture)(texture, ptr::null(), rgba.as_ptr().cast(), pitch) != 0 {
                if resized {
                    (self.api.destroy_texture)(texture);
                }
                return false;
            }
            if resized {
                (self.api.destroy_texture)(self.texture);
                self.texture = texture;
                self.width = width;
                self.height = height;
            }
            if (self.api.render_clear)(self.renderer) != 0
                || (self.api.render_copy)(self.renderer, texture, ptr::null(), ptr::null()) != 0
            {
                return false;
            }
            (self.api.render_present)(self.renderer);
        }
        true
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

fn checked_rgba_layout(width: u32, height: u32) -> Option<(c_int, c_int, c_int, usize)> {
    if width == 0 || height == 0 {
        return None;
    }
    let w = c_int::try_from(width).ok()?;
    let h = c_int::try_from(height).ok()?;
    let pitch = w.checked_mul(4)?;
    let size = usize::try_from(pitch)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)?;
    (size <= crate::display::MAX_RGBA_BYTES).then_some((w, h, pitch, size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Framebuffer, PixelFormat};
    use std::cell::{Cell, RefCell};
    #[derive(Debug, PartialEq, Eq)]
    enum Call {
        Create(u32, i32, i32),
        Update(usize, i32),
        Destroy(usize),
        Present,
    }
    thread_local! {
        static CALLS: RefCell<Vec<Call>> = const { RefCell::new(Vec::new()) };
        static FAIL_UPDATE: Cell<bool> = const { Cell::new(false) };
        static FAIL_CREATE: Cell<bool> = const { Cell::new(false) };
    }
    fn log(call: Call) {
        CALLS.with(|c| c.borrow_mut().push(call));
    }
    fn take() -> Vec<Call> {
        CALLS.with(|c| std::mem::take(&mut *c.borrow_mut()))
    }
    unsafe extern "C" fn init(_: u32) -> c_int {
        0
    }
    unsafe extern "C" fn quit() {}
    unsafe extern "C" fn window(
        _: *const c_char,
        _: c_int,
        _: c_int,
        _: c_int,
        _: c_int,
        _: u32,
    ) -> *mut c_void {
        ptr::dangling_mut::<c_void>()
    }
    unsafe extern "C" fn no_handle(_: *mut c_void) {}
    unsafe extern "C" fn title(_: *mut c_void, _: *const c_char) {}
    unsafe extern "C" fn renderer(_: *mut c_void, _: c_int, _: u32) -> *mut c_void {
        ptr::dangling_mut::<c_void>()
    }
    unsafe extern "C" fn texture(
        _: *mut c_void,
        format: u32,
        _: c_int,
        w: c_int,
        h: c_int,
    ) -> *mut c_void {
        log(Call::Create(format, w, h));
        if FAIL_CREATE.get() {
            ptr::null_mut()
        } else {
            ptr::without_provenance_mut::<c_void>(2)
        }
    }
    unsafe extern "C" fn destroy(texture: *mut c_void) {
        log(Call::Destroy(texture as usize));
    }
    unsafe extern "C" fn update(
        texture: *mut c_void,
        _: *const c_void,
        _: *const c_void,
        pitch: c_int,
    ) -> c_int {
        // Never dereference the source: the test observes whether malformed input
        // reaches the native boundary, without creating an out-of-bounds C read.
        log(Call::Update(texture as usize, pitch));
        if FAIL_UPDATE.get() { -1 } else { 0 }
    }
    unsafe extern "C" fn clear(_: *mut c_void) -> c_int {
        0
    }
    unsafe extern "C" fn copy(
        _: *mut c_void,
        _: *mut c_void,
        _: *const c_void,
        _: *const c_void,
    ) -> c_int {
        0
    }
    unsafe extern "C" fn present(_: *mut c_void) {
        log(Call::Present);
    }
    unsafe extern "C" fn event(_: *mut SdlEvent) -> c_int {
        0
    }
    unsafe extern "C" fn num() -> c_int {
        0
    }
    unsafe extern "C" fn is_controller(_: c_int) -> c_int {
        0
    }
    unsafe extern "C" fn controller(_: c_int) -> *mut c_void {
        ptr::null_mut()
    }
    unsafe extern "C" fn button(_: *mut c_void, _: c_int) -> u8 {
        0
    }
    unsafe extern "C" fn axis(_: *mut c_void, _: c_int) -> i16 {
        0
    }
    fn host() -> SdlHost {
        take();
        FAIL_UPDATE.set(false);
        FAIL_CREATE.set(false);
        SdlHost {
            api: SdlApi {
                _handle: ptr::null_mut(),
                init,
                quit,
                create_window: window,
                destroy_window: no_handle,
                set_window_title: title,
                create_renderer: renderer,
                destroy_renderer: no_handle,
                create_texture: texture,
                destroy_texture: destroy,
                update_texture: update,
                render_clear: clear,
                render_copy: copy,
                render_present: present,
                poll_event: event,
                num_joysticks: num,
                is_game_controller: is_controller,
                game_controller_open: controller,
                game_controller_close: no_handle,
                game_controller_get_button: button,
                game_controller_get_axis: axis,
            },
            window: ptr::dangling_mut::<c_void>(),
            renderer: ptr::dangling_mut::<c_void>(),
            texture: ptr::dangling_mut::<c_void>(),
            controller: ptr::null_mut(),
            width: 1280,
            height: 720,
            gamepad: GamepadState::default(),
            should_close: false,
        }
    }
    #[test]
    fn invalid_window_titles_never_reach_the_native_api() {
        let mut host = host();
        assert!(!host.set_title("invalid\0title"));
        assert!(host.set_title("Wanderlust | Vulkan | draws: 0"));
        assert!(take().is_empty());
    }
    #[test]
    fn short_and_overflowing_rgba_never_reach_sdl() {
        let mut host = host();
        for (w, h, n) in [
            (2, 2, 15),
            (2, 2, 17),
            (0, 2, 0),
            (2, 0, 0),
            (u32::MAX, 1, 4),
            (1, u32::MAX, 4),
            (4097, 4097, 4),
        ] {
            assert!(!host.present_raw_rgba(&vec![0; n], w, h));
            assert!(take().is_empty());
            assert_eq!(
                (host.width, host.height, host.texture as usize),
                (1280, 720, 1)
            );
        }
    }
    #[test]
    fn small_frame_resizes_before_upload_and_warm_dimensions_reuse() {
        let mut host = host();
        let mut display = DisplayServer::new(2, 2);
        display.current_frame = Some(Framebuffer::new(
            2,
            2,
            2,
            PixelFormat::Rgba8888,
            vec![0; 16],
        ));
        assert!(host.present_frame(&display));
        #[cfg(target_endian = "little")]
        let expected_format = 0x16762004;
        #[cfg(target_endian = "big")]
        let expected_format = 0x16462004;
        assert_eq!(
            take(),
            [
                Call::Create(expected_format, 2, 2),
                Call::Update(2, 8),
                Call::Destroy(1),
                Call::Present
            ]
        );
        assert_eq!((host.width, host.height, host.texture as usize), (2, 2, 2));
        assert!(host.present_frame(&display));
        assert_eq!(take(), [Call::Update(2, 8), Call::Present]);
        display.current_frame = Some(Framebuffer::new(
            2,
            2,
            2,
            PixelFormat::Rgba8888,
            vec![0; 15],
        ));
        assert!(!host.present_frame(&display));
        assert!(take().is_empty());
    }
    #[test]
    fn failed_resize_or_upload_keeps_old_texture_and_dimensions() {
        let mut host = host();
        FAIL_CREATE.set(true);
        assert!(!host.present_raw_rgba(&[0; 16], 2, 2));
        assert_eq!(take().len(), 1);
        assert_eq!(
            (host.width, host.height, host.texture as usize),
            (1280, 720, 1)
        );
        FAIL_CREATE.set(false);
        FAIL_UPDATE.set(true);
        assert!(!host.present_raw_rgba(&[0; 16], 2, 2));
        let calls = take();
        assert!(matches!(
            calls.as_slice(),
            [Call::Create(_, 2, 2), Call::Update(2, 8), Call::Destroy(2)]
        ));
        assert_eq!(
            (host.width, host.height, host.texture as usize),
            (1280, 720, 1)
        );
    }
}
