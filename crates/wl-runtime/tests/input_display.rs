//! Tests for Switch Npad input mapping, Horizon HID shared memory layout,
//! and display presentation buffer management.

use wl_runtime::{
    display::{DisplayServer, PixelFormat},
    input::{
        GamepadState, LIFO_ENTRY_SIZE, LIFO_FULLKEY_OFFSET, LIFO_HANDHELD_OFFSET,
        LIFO_JOYDUAL_OFFSET, NPAD_ENTRY_SIZE, NPAD_SHM_OFFSET, NpadManager, npad, write_npad_state,
    },
    sdl::SdlHost,
};

#[test]
fn xinput_to_switch_face_button_swap() {
    // Xbox A -> Switch B
    let mut pad = GamepadState {
        a: true,
        ..Default::default()
    };
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::B);

    // Xbox B -> Switch A
    pad = GamepadState {
        b: true,
        ..Default::default()
    };
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::A);

    // Xbox X -> Switch Y
    pad = GamepadState {
        x: true,
        ..Default::default()
    };
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::Y);

    // Xbox Y -> Switch X
    pad = GamepadState {
        y: true,
        ..Default::default()
    };
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::X);
}

#[test]
fn xinput_triggers_and_shoulders() {
    // Bumpers: LB -> Switch L, RB -> Switch R
    let mut pad = GamepadState {
        left_shoulder: true,
        right_shoulder: true,
        ..Default::default()
    };
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::L | npad::R);

    // Triggers below threshold (<= 30) should not activate ZL/ZR
    pad.left_trigger = 25;
    pad.right_trigger = 20;
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::L | npad::R);

    // Triggers above threshold (> 30) activate ZL/ZR
    pad.left_trigger = 128;
    pad.right_trigger = 255;
    let (buttons, _, _) = pad.to_switch_npad();
    assert_eq!(buttons, npad::L | npad::R | npad::ZL | npad::ZR);
}

#[test]
fn xinput_dpad_and_sticks() {
    let pad = GamepadState {
        dpad_up: true,
        dpad_left: true,
        start: true, // Plus
        back: true,  // Minus
        left_thumb: true,
        right_thumb: true,
        thumb_lx: -25000,
        thumb_ly: 20000,
        thumb_rx: 28000,
        thumb_ry: -18000,
        ..Default::default()
    };

    let (buttons, stick_l, stick_r) = pad.to_switch_npad();
    assert_eq!(stick_l, (-25000, 20000));
    assert_eq!(stick_r, (28000, -18000));

    assert!(buttons & npad::D_UP != 0);
    assert!(buttons & npad::D_LEFT != 0);
    assert!(buttons & npad::PLUS != 0);
    assert!(buttons & npad::MINUS != 0);
    assert!(buttons & npad::STICK_L != 0);
    assert!(buttons & npad::STICK_R != 0);
    assert!(buttons & npad::STICK_L_LEFT != 0);
    assert!(buttons & npad::STICK_L_UP != 0);
    assert!(buttons & npad::STICK_R_RIGHT != 0);
    assert!(buttons & npad::STICK_R_DOWN != 0);
}

#[test]
fn hid_shared_memory_ring_buffer_write() {
    let mut shm = vec![0u8; 0x40000];

    // Write sample 1 to Player 1 (controller 0)
    write_npad_state(
        &mut shm,
        0,
        1,
        npad::A | npad::PLUS,
        (1000, -2000),
        (3000, -4000),
    );

    let base = NPAD_SHM_OFFSET;
    // Verify StyleSet
    let style_set = u32::from_le_bytes(shm[base..base + 4].try_into().unwrap());
    assert_eq!(style_set, 7);

    // Verify FullKeyLifo
    let lifo = base + LIFO_FULLKEY_OFFSET;
    let cap = u64::from_le_bytes(shm[lifo + 8..lifo + 16].try_into().unwrap());
    let tail = u64::from_le_bytes(shm[lifo + 16..lifo + 24].try_into().unwrap());
    assert_eq!(cap, 16);
    assert_eq!(tail, 1);

    let entry = lifo + 0x20 + (tail as usize) * LIFO_ENTRY_SIZE;
    let sample = u64::from_le_bytes(shm[entry..entry + 8].try_into().unwrap());
    let buttons = u64::from_le_bytes(shm[entry + 16..entry + 24].try_into().unwrap());
    let lx = i32::from_le_bytes(shm[entry + 24..entry + 28].try_into().unwrap());
    let ly = i32::from_le_bytes(shm[entry + 28..entry + 32].try_into().unwrap());
    let rx = i32::from_le_bytes(shm[entry + 32..entry + 36].try_into().unwrap());
    let ry = i32::from_le_bytes(shm[entry + 36..entry + 40].try_into().unwrap());
    let attr = u64::from_le_bytes(shm[entry + 40..entry + 48].try_into().unwrap());

    assert_eq!(sample, 1);
    assert_eq!(buttons, npad::A | npad::PLUS);
    assert_eq!(lx, 1000);
    assert_eq!(ly, -2000);
    assert_eq!(rx, 3000);
    assert_eq!(ry, -4000);
    assert_eq!(attr, 1); // is_connected

    // Verify HandheldLifo and JoyDualLifo are also updated
    for &lifo_off in &[LIFO_HANDHELD_OFFSET, LIFO_JOYDUAL_OFFSET] {
        let l = base + lifo_off;
        let e = l + 0x20 + (tail as usize) * LIFO_ENTRY_SIZE;
        let b = u64::from_le_bytes(shm[e + 16..e + 24].try_into().unwrap());
        assert_eq!(b, npad::A | npad::PLUS);
    }
}

#[test]
fn npad_manager_updates_both_player1_and_handheld() {
    let mut shm = vec![0u8; 0x40000];
    let mut manager = NpadManager::new();
    manager.state.b = true; // Switch A
    manager.state.thumb_lx = 15000;

    manager.update(&mut shm);
    assert_eq!(manager.sampling_number, 1);

    // Check Player 1 (entry 0)
    let p1_entry = NPAD_SHM_OFFSET + LIFO_FULLKEY_OFFSET + 0x20 + LIFO_ENTRY_SIZE;
    let p1_buttons = u64::from_le_bytes(shm[p1_entry + 16..p1_entry + 24].try_into().unwrap());
    let p1_lx = i32::from_le_bytes(shm[p1_entry + 24..p1_entry + 28].try_into().unwrap());
    assert_eq!(p1_buttons, npad::A);
    assert_eq!(p1_lx, 15000);

    // Check Handheld (entry 8)
    let h_base = NPAD_SHM_OFFSET + 8 * NPAD_ENTRY_SIZE;
    let h_entry = h_base + LIFO_HANDHELD_OFFSET + 0x20 + LIFO_ENTRY_SIZE;
    let h_buttons = u64::from_le_bytes(shm[h_entry + 16..h_entry + 24].try_into().unwrap());
    let h_lx = i32::from_le_bytes(shm[h_entry + 24..h_entry + 28].try_into().unwrap());
    assert_eq!(h_buttons, npad::A);
    assert_eq!(h_lx, 15000);
}

#[test]
fn display_server_and_framebuffer_conversions() {
    let mut display = DisplayServer::new(2, 2);

    // Queue 2x2 BGRA8888 buffer
    // Blue pixel, Red pixel, Green pixel, White pixel
    let bgra = vec![
        255, 0, 0, 255, // Blue (B=255, G=0, R=0)
        0, 0, 255, 255, // Red  (B=0, G=0, R=255)
        0, 255, 0, 255, // Green(B=0, G=255, R=0)
        255, 255, 255, 255, // White
    ];

    display.queue_buffer(2, 2, 2, PixelFormat::Bgra8888 as u32, bgra);
    assert_eq!(display.frame_count, 1);

    let frame = display.current_frame.as_ref().unwrap();
    let rgba = frame.to_rgba8888().unwrap();
    assert_eq!(rgba.len(), 16);

    // Pixel 0: Red=0, Green=0, Blue=255, Alpha=255
    assert_eq!(&rgba[0..4], &[0, 0, 255, 255]);
    // Pixel 1: Red=255, Green=0, Blue=0, Alpha=255
    assert_eq!(&rgba[4..8], &[255, 0, 0, 255]);
    // Pixel 2: Red=0, Green=255, Blue=0, Alpha=255
    assert_eq!(&rgba[8..12], &[0, 255, 0, 255]);
    // Pixel 3: White
    assert_eq!(&rgba[12..16], &[255, 255, 255, 255]);

    // Test PPM generation
    let ppm = frame.to_ppm().unwrap();
    assert!(ppm.starts_with(b"P6\n2 2\n255\n"));
    assert_eq!(ppm.len(), b"P6\n2 2\n255\n".len() + 2 * 2 * 3);
}

#[test]
fn sdl_headless_fallback() {
    // In headless or environments without SDL2 installed, SdlHost::open returns None safely
    // without crashing or aborting.
    let host = SdlHost::open("Test Window", 1280, 720);
    // Either opens if SDL2/X11 available, or returns None gracefully
    if let Some(mut h) = host {
        assert_eq!(h.width, 1280);
        assert_eq!(h.height, 720);
        let _ = h.poll_input();
    }
}
