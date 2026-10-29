#![cfg(feature = "wgpu-backend")]
use wl_gpu::{driver::*, driver_backend::*, initial_capture::CommandSnapshot, memory::*};
fn method(w: &mut Vec<u32>, m: u32, v: u32) {
    w.extend([0x20010000 | (m / 4), v]);
}
fn entry(a: u64, n: usize) -> [u8; 8] {
    (a | ((n as u64) << 42)).to_le_bytes()
}
#[test]
#[ignore = "requires real GPU: diagnostic snapshot must not replace guarded command acquisition"]
fn preexecution_snapshot_never_replays_an_overwritten_command_suffix() {
    for capture_fails in [false, true] {
        let directory = std::env::temp_dir().join(format!(
            "wl-initial-gpu-{}-{capture_fails}",
            std::process::id()
        ));
        if capture_fails {
            std::fs::write(&directory, b"not a directory").unwrap();
        }
        let mut driver = GpuDriver::new().unwrap();
        driver.enable_initial_command_capture(&directory).unwrap();
        let mut data = vec![0u8; 8192];
        let backing = driver.register_backing(GuestAddress(0), 8192).unwrap();
        let space = driver.create_space().unwrap();
        let channel = driver.create_channel(space).unwrap();
        for (gpu, offset) in [(0xb0000, 0), (0x90000, 4096)] {
            let reserve = driver.reserve_fixed(space, GpuAddress(gpu), 4096).unwrap();
            driver.map(space, reserve, backing, offset).unwrap();
        }
        // Literal single-layer pitch RGBA8 7x9 target. The second GPFIFO entry
        // initially contains a DMA NOP, but the clear overwrites it with color.
        let mut words = vec![0x20010000, 0xb197];
        for (m, v) in [
            (0x138, 1),
            (0x15d0, 0),
            (0x121c, 1),
            (0x10f8, 0),
            (0xff4, 7 << 16),
            (0xff8, 9 << 16),
        ] {
            method(&mut words, m, v);
        }
        for (n, v) in [0, 0x90000, 28, 9, 0xd5, 0x1000, 1, 0, 0]
            .into_iter()
            .enumerate()
        {
            method(&mut words, 0x800 + n as u32 * 4, v);
        }
        words.extend([0x20040360, 1f32.to_bits(), 0, 0, 1f32.to_bits()]);
        method(&mut words, 0x19d0, 0x3c);
        for (raw, value) in data.chunks_exact_mut(4).zip(&words) {
            raw.copy_from_slice(&value.to_le_bytes());
        }
        let mut entries = entry(0xb0000, words.len()).to_vec();
        entries.extend(entry(0x90000, 1));
        let mut header = [0u8; 24];
        header[8..12].copy_from_slice(&2u32.to_le_bytes());
        header[12..16].copy_from_slice(&4u32.to_le_bytes());
        let report = driver
            .submit(
                channel,
                KICKOFF_PB,
                &header,
                Some(&entries),
                &mut SliceMemoryMut {
                    base: GuestAddress(0),
                    bytes: &mut data,
                },
            )
            .unwrap();
        // The real executor reacquires 0x000000ff after publishing red pixels;
        // interpreting the old NOP snapshot here would incorrectly succeed.
        assert!(report.fault.is_some());
        assert_eq!(report.progress.entries_completed, 1);
        assert_eq!(driver.graphics_counters().clear_commands, 1);
        assert_eq!(driver.graphics_counters().draw_commands, 0);
        for pixel in data[4096..4096 + 7 * 9 * 4].chunks_exact(4) {
            assert_eq!(pixel, [255, 0, 0, 255]);
        }
        let counters = driver.initial_capture_counters().unwrap();
        if capture_fails {
            assert_eq!((counters.files, counters.failures), (0, 1));
            std::fs::remove_file(directory).unwrap();
        } else {
            assert_eq!((counters.files, counters.failures), (1, 0));
            assert_eq!(counters.guest_read_calls, 2);
            let path = std::fs::read_dir(&directory)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let snapshot = CommandSnapshot::read(path).unwrap();
            assert_eq!(snapshot.entries().nth(1).unwrap().1, [0; 4]);
            assert_eq!(counters.guest_borrowed_bytes, (words.len() * 4 + 4) as u64);
            std::fs::remove_dir_all(directory).unwrap();
        }
        assert!(driver.enable_initial_command_capture("unused").is_err());
        driver
            .close(
                channel,
                &mut SliceMemoryMut {
                    base: GuestAddress(0),
                    bytes: &mut data,
                },
            )
            .unwrap();
    }
}
