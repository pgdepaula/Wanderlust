//! Controller input mapping: Switch Npad, XInput, and SDL gamepad mappings
//! backed by the Horizon HID shared memory format.

pub mod npad {
    pub const A: u64 = 1 << 0;
    pub const B: u64 = 1 << 1;
    pub const X: u64 = 1 << 2;
    pub const Y: u64 = 1 << 3;
    pub const STICK_L: u64 = 1 << 4;
    pub const STICK_R: u64 = 1 << 5;
    pub const L: u64 = 1 << 6;
    pub const R: u64 = 1 << 7;
    pub const ZL: u64 = 1 << 8;
    pub const ZR: u64 = 1 << 9;
    pub const PLUS: u64 = 1 << 10;
    pub const MINUS: u64 = 1 << 11;
    pub const D_LEFT: u64 = 1 << 12;
    pub const D_UP: u64 = 1 << 13;
    pub const D_RIGHT: u64 = 1 << 14;
    pub const D_DOWN: u64 = 1 << 15;
    pub const STICK_L_LEFT: u64 = 1 << 16;
    pub const STICK_L_UP: u64 = 1 << 17;
    pub const STICK_L_RIGHT: u64 = 1 << 18;
    pub const STICK_L_DOWN: u64 = 1 << 19;
    pub const STICK_R_LEFT: u64 = 1 << 20;
    pub const STICK_R_UP: u64 = 1 << 21;
    pub const STICK_R_RIGHT: u64 = 1 << 22;
    pub const STICK_R_DOWN: u64 = 1 << 23;
    pub const SL_LEFT: u64 = 1 << 24;
    pub const SR_LEFT: u64 = 1 << 25;
    pub const SL_RIGHT: u64 = 1 << 26;
    pub const SR_RIGHT: u64 = 1 << 27;
}

/// Normalized controller state matching XInput / standard SDL gamepads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GamepadState {
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub start: bool,
    pub back: bool,
    pub guide: bool,
    pub left_thumb: bool,
    pub right_thumb: bool,
    pub left_shoulder: bool,
    pub right_shoulder: bool,
    /// Xbox A (bottom button)
    pub a: bool,
    /// Xbox B (right button)
    pub b: bool,
    /// Xbox X (left button)
    pub x: bool,
    /// Xbox Y (top button)
    pub y: bool,
    /// Left trigger (0..255)
    pub left_trigger: u8,
    /// Right trigger (0..255)
    pub right_trigger: u8,
    /// Left thumbstick X (-32768..32767)
    pub thumb_lx: i16,
    /// Left thumbstick Y (-32768..32767, positive is up)
    pub thumb_ly: i16,
    /// Right thumbstick X (-32768..32767)
    pub thumb_rx: i16,
    /// Right thumbstick Y (-32768..32767, positive is up)
    pub thumb_ry: i16,
}

impl GamepadState {
    /// Maps XInput / Xbox / SDL controller state to Nintendo Switch Npad format:
    /// - Xbox A (bottom) -> Switch B
    /// - Xbox B (right)  -> Switch A
    /// - Xbox X (left)   -> Switch Y
    /// - Xbox Y (top)    -> Switch X
    /// - LB / RB         -> Switch L / R
    /// - LT / RT         -> Switch ZL / ZR (threshold > 30)
    /// - Start / Back    -> Switch Plus / Minus
    /// - LS / RS (click) -> Switch StickL / StickR
    /// - Thumbsticks     -> Switch analog sticks (-32767..32767)
    pub fn to_switch_npad(&self) -> (u64, (i32, i32), (i32, i32)) {
        let mut buttons = 0u64;
        if self.b {
            buttons |= npad::A;
        }
        if self.a {
            buttons |= npad::B;
        }
        if self.y {
            buttons |= npad::X;
        }
        if self.x {
            buttons |= npad::Y;
        }
        if self.left_thumb {
            buttons |= npad::STICK_L;
        }
        if self.right_thumb {
            buttons |= npad::STICK_R;
        }
        if self.left_shoulder {
            buttons |= npad::L;
        }
        if self.right_shoulder {
            buttons |= npad::R;
        }
        if self.left_trigger > 30 {
            buttons |= npad::ZL;
        }
        if self.right_trigger > 30 {
            buttons |= npad::ZR;
        }
        if self.start {
            buttons |= npad::PLUS;
        }
        if self.back {
            buttons |= npad::MINUS;
        }
        if self.dpad_left {
            buttons |= npad::D_LEFT;
        }
        if self.dpad_up {
            buttons |= npad::D_UP;
        }
        if self.dpad_right {
            buttons |= npad::D_RIGHT;
        }
        if self.dpad_down {
            buttons |= npad::D_DOWN;
        }

        let lx = self.thumb_lx as i32;
        let ly = self.thumb_ly as i32;
        let rx = self.thumb_rx as i32;
        let ry = self.thumb_ry as i32;

        if lx < -16000 {
            buttons |= npad::STICK_L_LEFT;
        }
        if lx > 16000 {
            buttons |= npad::STICK_L_RIGHT;
        }
        if ly > 16000 {
            buttons |= npad::STICK_L_UP;
        }
        if ly < -16000 {
            buttons |= npad::STICK_L_DOWN;
        }

        if rx < -16000 {
            buttons |= npad::STICK_R_LEFT;
        }
        if rx > 16000 {
            buttons |= npad::STICK_R_RIGHT;
        }
        if ry > 16000 {
            buttons |= npad::STICK_R_UP;
        }
        if ry < -16000 {
            buttons |= npad::STICK_R_DOWN;
        }

        (buttons, (lx, ly), (rx, ry))
    }
}

/// Nintendo Switch HID Shared Memory Npad constants.
pub const NPAD_SHM_OFFSET: usize = 0x9a00;
pub const NPAD_ENTRY_SIZE: usize = 0x5000;
pub const LIFO_FULLKEY_OFFSET: usize = 0x28;
pub const LIFO_HANDHELD_OFFSET: usize = 0x378;
pub const LIFO_JOYDUAL_OFFSET: usize = 0x6c8;
pub const LIFO_CAPACITY: usize = 16;
pub const LIFO_ENTRY_SIZE: usize = 0x30;

/// Writes an Npad controller sample into the Horizon HID shared memory buffer.
pub fn write_npad_state(
    shm: &mut [u8],
    controller_idx: usize,
    sampling_number: u64,
    buttons: u64,
    left_stick: (i32, i32),
    right_stick: (i32, i32),
) {
    if shm.len() < 0x40000 || controller_idx >= 10 {
        return;
    }
    let base = NPAD_SHM_OFFSET + controller_idx * NPAD_ENTRY_SIZE;

    // StyleSet = 7 (FullKey | Handheld | JoyDual)
    shm[base..base + 4].copy_from_slice(&7u32.to_le_bytes());
    // JoyAssignmentMode = 0 (Dual)
    shm[base + 4..base + 8].copy_from_slice(&0u32.to_le_bytes());

    for &lifo_offset in &[
        LIFO_FULLKEY_OFFSET,
        LIFO_HANDHELD_OFFSET,
        LIFO_JOYDUAL_OFFSET,
    ] {
        let lifo = base + lifo_offset;
        let tail = (sampling_number % LIFO_CAPACITY as u64) as usize;

        // RingLifo header: vtable (8), buffer_count (8), tail (8), count (8)
        shm[lifo + 8..lifo + 16].copy_from_slice(&(LIFO_CAPACITY as u64).to_le_bytes());
        shm[lifo + 16..lifo + 24].copy_from_slice(&(tail as u64).to_le_bytes());
        shm[lifo + 24..lifo + 32].copy_from_slice(&(LIFO_CAPACITY as u64).to_le_bytes());

        // Entry at lifo + 0x20 + tail * 0x30
        let entry = lifo + 0x20 + tail * LIFO_ENTRY_SIZE;
        shm[entry..entry + 8].copy_from_slice(&sampling_number.to_le_bytes());
        shm[entry + 8..entry + 16].copy_from_slice(&sampling_number.to_le_bytes());
        shm[entry + 16..entry + 24].copy_from_slice(&buttons.to_le_bytes());
        shm[entry + 24..entry + 28].copy_from_slice(&left_stick.0.to_le_bytes());
        shm[entry + 28..entry + 32].copy_from_slice(&left_stick.1.to_le_bytes());
        shm[entry + 32..entry + 36].copy_from_slice(&right_stick.0.to_le_bytes());
        shm[entry + 36..entry + 40].copy_from_slice(&right_stick.1.to_le_bytes());
        shm[entry + 40..entry + 48].copy_from_slice(&1u64.to_le_bytes()); // Attributes: connected
    }
}

/// Tracks HID input state across frames.
#[derive(Clone, Debug, Default)]
pub struct NpadManager {
    pub sampling_number: u64,
    pub state: GamepadState,
}

impl NpadManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances the sampling frame and writes the controller state to Player 1 (entry 0)
    /// and Handheld (entry 8).
    pub fn update(&mut self, shm: &mut [u8]) {
        self.sampling_number = self.sampling_number.wrapping_add(1);
        let (buttons, stick_l, stick_r) = self.state.to_switch_npad();

        // Player 1 (entry 0)
        write_npad_state(shm, 0, self.sampling_number, buttons, stick_l, stick_r);
        // Handheld (entry 8)
        write_npad_state(shm, 8, self.sampling_number, buttons, stick_l, stick_r);
    }
}
