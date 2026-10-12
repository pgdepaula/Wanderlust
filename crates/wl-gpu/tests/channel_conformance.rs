//! Adversarial hardware and driver ABI conformance suite for Switch Maxwell channels.
//!
//! Verified against:
//! - NVIDIA open-gpu-doc (MIT) rev 9fdf5c4062007929d9f4e6cbad9c9771fe61b880:
//!   `classes/host/clb06f.h` lines 154-176 (GPFIFO entry bitfields, FETCH, PRIV, LEVEL, SYNC).
//! - switchbrew/libnx (ISC) rev feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973:
//!   `nx/source/nvidia/ioctl/nvchannel.c` (SubmitGpfifo inline vs KickoffPb ioctl2 header),
//!   `nx/source/nvidia/gpu_channel.c` (flags: HW_FORMAT bit 2),
//!   `nx/source/nvidia/ioctl/nvhost-as-gpu.c` (address space mapping, unmap, fixed reservations).
//! - wgpu 26.0.1 (MIT/Apache-2.0) completion lifecycle:
//!   `Queue::on_submitted_work_done` and `Device::poll` for truthful host completion.

use wl_gpu::{
    driver::*,
    gpfifo::{Entry, EntryError, MAX_ENTRIES, MAX_WORDS, Submission, WIRE_MAX_WORDS},
    maxwell::Channel,
    memory::*,
};

#[cfg(feature = "wgpu-backend")]
mod common;

#[cfg(feature = "wgpu-backend")]
use common::{maxwell::*, *};
#[cfg(feature = "wgpu-backend")]
use wl_gpu::{
    driver_backend::*,
    maxwell::{TextureImage, TextureSampler},
};

fn make_submit_request(entries: &[u8], legacy: bool) -> (u32, Vec<u8>) {
    let mut header = vec![0u8; 24];
    header[..8].copy_from_slice(&0xdead_beef_cafe_babe_u64.to_le_bytes()); // ignored pointer
    header[8..12].copy_from_slice(&((entries.len() / 8) as u32).to_le_bytes());
    header[12..16].copy_from_slice(&4u32.to_le_bytes()); // HW_FORMAT = 4
    header[16..20].copy_from_slice(&0u32.to_le_bytes()); // input fence id = 0
    header[20..24].copy_from_slice(&0u32.to_le_bytes()); // input fence val = 0
    if legacy {
        header.extend_from_slice(entries);
        let code = 0xc0004808 | ((header.len() as u32) << 16);
        (code, header)
    } else {
        (KICKOFF_PB, header)
    }
}

// ---------------------------------------------------------------------------
// 1. SubmitRequest wire parsing, flag rejection, and boundary checks
// ---------------------------------------------------------------------------

#[test]
fn submit_request_wire_header_and_flag_permutations() {
    let single_entry = (0x10000u64 | (1u64 << 42)).to_le_bytes(); // 1 word from 0x10000

    // Test both KickoffPb (ioctl2) and legacy SubmitGpfifo (inline)
    for legacy in [false, true] {
        let (code, payload) = make_submit_request(&single_entry, legacy);
        let extra = (!legacy).then_some(single_entry.as_slice());

        // Valid decode succeeds
        let decoded = SubmitRequest::decode(code, &payload, extra).unwrap();
        assert_eq!(decoded.entries(), single_entry.as_slice());

        // Truncated headers: any header shorter than 24 bytes must fail
        for len in 0..24 {
            assert!(
                SubmitRequest::decode(code, &payload[..len], extra).is_err(),
                "truncated header len {len} should fail"
            );
        }

        // Exact payload length check for legacy inline buffer:
        // Must match 24 + count * 8 exactly
        if legacy {
            let mut bad_payload = payload.clone();
            bad_payload.push(0); // 1 extra byte
            assert!(SubmitRequest::decode(code, &bad_payload, extra).is_err());
            bad_payload.truncate(payload.len() - 1); // 1 missing byte
            assert!(SubmitRequest::decode(code, &bad_payload, extra).is_err());
        }

        // Pointer field at bytes 0..8 is ignored: verify ANY bit pattern passes
        for ptr_val in [0u64, 1, 0x5555_5555_5555_5555, u64::MAX] {
            let mut modified = payload.clone();
            modified[..8].copy_from_slice(&ptr_val.to_le_bytes());
            assert!(
                SubmitRequest::decode(code, &modified, extra).is_ok(),
                "pointer field must be ignored"
            );
        }

        // Flags field at bytes 12..16: ONLY 4 (HW_FORMAT) is valid.
        // Adversarially test all single-bit flags and combinations
        for bit in 0..32 {
            if bit == 2 {
                continue; // bit 2 is 4 (HW_FORMAT), which is valid
            }
            let bad_flag = 1u32 << bit;
            let mut bad_payload = payload.clone();
            bad_payload[12..16].copy_from_slice(&bad_flag.to_le_bytes());
            assert!(
                SubmitRequest::decode(code, &bad_payload, extra).is_err(),
                "flag bit {bit} (val {bad_flag:#x}) must be rejected"
            );

            // Also test HW_FORMAT combined with extra flag bit
            let combined = 4u32 | bad_flag;
            bad_payload[12..16].copy_from_slice(&combined.to_le_bytes());
            assert!(
                SubmitRequest::decode(code, &bad_payload, extra).is_err(),
                "combined flag {combined:#x} must be rejected"
            );
        }

        // Input fence ID (bytes 16..20) must be 0
        for fence_id in [1u32, 2, 0xff, u32::MAX] {
            let mut bad_payload = payload.clone();
            bad_payload[16..20].copy_from_slice(&fence_id.to_le_bytes());
            assert!(
                SubmitRequest::decode(code, &bad_payload, extra).is_err(),
                "non-zero input fence ID must be rejected"
            );
        }

        // Input fence value (bytes 20..24) must be 0
        for fence_val in [1u32, 2, 0x1000, u32::MAX] {
            let mut bad_payload = payload.clone();
            bad_payload[20..24].copy_from_slice(&fence_val.to_le_bytes());
            assert!(
                SubmitRequest::decode(code, &bad_payload, extra).is_err(),
                "non-zero input fence value must be rejected"
            );
        }
    }

    // Extra buffer mismatch:
    // KickoffPb with missing extra buffer -> Err
    let (code, payload) = make_submit_request(&single_entry, false);
    assert!(SubmitRequest::decode(code, &payload, None).is_err());

    // Legacy with extra buffer supplied -> Err
    let (code, payload) = make_submit_request(&single_entry, true);
    assert!(SubmitRequest::decode(code, &payload, Some(&single_entry)).is_err());

    // Entry count upper bounds:
    // KickoffPb supports up to 2048 entries
    let max_entries = single_entry.repeat(MAX_ENTRIES);
    let (code, payload) = make_submit_request(&max_entries, false);
    assert!(SubmitRequest::decode(code, &payload, Some(&max_entries)).is_ok());

    let too_many_kickoff = single_entry.repeat(MAX_ENTRIES + 1);
    let (code, payload) = make_submit_request(&too_many_kickoff, false);
    assert!(SubmitRequest::decode(code, &payload, Some(&too_many_kickoff)).is_err());

    // Legacy SubmitGpfifo supports up to 512 entries
    let max_legacy = single_entry.repeat(512);
    let (code, payload) = make_submit_request(&max_legacy, true);
    assert!(SubmitRequest::decode(code, &payload, None).is_ok());

    let too_many_legacy = single_entry.repeat(513);
    let (code, payload) = make_submit_request(&too_many_legacy, true);
    assert!(SubmitRequest::decode(code, &payload, None).is_err());
}

// ---------------------------------------------------------------------------
// 2. GPFIFO entry bitfields, word limits, and 40-bit boundary conformance
// ---------------------------------------------------------------------------

#[test]
fn gpfifo_entry_bitfield_and_40bit_boundary_conformance() {
    let make_raw = |lo: u32, hi: u32| -> [u8; 8] {
        let mut b = [0u8; 8];
        b[..4].copy_from_slice(&lo.to_le_bytes());
        b[4..].copy_from_slice(&hi.to_le_bytes());
        b
    };

    // Valid base entry: address 0x10000, 4 words, main level, user privilege, proceed
    // lo = 0x10000, hi = (4 << 10) = 0x1000
    let valid_raw = make_raw(0x10000, 4 << 10);
    let entry = Entry::decode(valid_raw).unwrap();
    assert_eq!(entry.address(), GpuAddress(0x10000));
    assert_eq!(entry.words(), 4);
    assert_eq!(entry.byte_len(), 16);

    // FETCH bit (lo bit 0): 1 = conditional fetch (unsupported)
    assert_eq!(
        Entry::decode(make_raw(0x10001, 4 << 10)),
        Err(EntryError::ConditionalFetch)
    );

    // Reserved bit (lo bit 1): must be 0
    assert_eq!(
        Entry::decode(make_raw(0x10002, 4 << 10)),
        Err(EntryError::ReservedBit)
    );

    // PRIV bit (hi bit 8): 1 = kernel privilege (unsupported)
    assert_eq!(
        Entry::decode(make_raw(0x10000, (4 << 10) | 0x100)),
        Err(EntryError::Privileged)
    );

    // LEVEL bit (hi bit 9): 1 = subroutine (unsupported)
    assert_eq!(
        Entry::decode(make_raw(0x10000, (4 << 10) | 0x200)),
        Err(EntryError::Subroutine)
    );

    // SYNC bit (hi bit 31): 1 = SYNC_WAIT (unsupported)
    assert_eq!(
        Entry::decode(make_raw(0x10000, (4 << 10) | 0x8000_0000)),
        Err(EntryError::Wait)
    );

    // Words count (hi bits 30:10): 0 = control entry (unsupported)
    assert_eq!(
        Entry::decode(make_raw(0x10000, 0)),
        Err(EntryError::ControlEntry)
    );

    // 40-bit GPU address limit:
    // Exactly at 2^40 (1 << 40):
    // Address = (1 << 40) - 4 = 0xff_ffff_fffc. Words = 1 (4 bytes).
    // End = (1 << 40) exactly.
    let lo_max = 0xffff_fffc_u32;
    let hi_max = 0xff | (1 << 10); // address bits 39:32 = 0xff, words = 1
    let edge_entry = Entry::decode(make_raw(lo_max, hi_max)).unwrap();
    assert_eq!(
        edge_entry.address().0 + edge_entry.byte_len() as u64,
        1u64 << 40
    );

    // Overshoot by 1 word (4 bytes):
    // Words = 2 -> End = (1 << 40) + 4 -> AddressRange error
    let hi_over = 0xff | (2 << 10);
    assert_eq!(
        Entry::decode(make_raw(lo_max, hi_over)),
        Err(EntryError::AddressRange)
    );

    // Wire max words in entry: (1 << 21) - 1 = 2,097,151 words (8,388,604 bytes)
    assert_eq!(WIRE_MAX_WORDS, 2_097_151);

    // Aggregate submission budget in Submission::new:
    // 1 single entry requesting > 1,048,576 words fails
    let huge_entry = make_raw(0x10000, (MAX_WORDS as u32 + 1) << 10);
    assert!(Submission::new(&huge_entry).is_err());
}

// ---------------------------------------------------------------------------
// 3. AddressSpace oracle, disjoint intervals, aliasing, and remapping
// ---------------------------------------------------------------------------

#[test]
fn address_space_model_disjoint_intervals_and_aliasing() {
    let mut mm = MemoryManager::default();

    // Register two distinct canonical backing allocations
    let backing1 = mm.register_backing(GuestAddress(0x1000), 0x4000).unwrap();
    let backing2 = mm.register_backing(GuestAddress(0x8000), 0x2000).unwrap();

    // Overlapping canonical backing registration must fail
    assert!(mm.register_backing(GuestAddress(0x2000), 0x1000).is_err());
    assert!(mm.register_backing(GuestAddress(0x9000), 0x1000).is_err());

    // Create address spaces
    let space1 = mm.create_space().unwrap();
    let space2 = mm.create_space().unwrap();

    // Verify 4 KiB alignment requirement for reservations
    assert!(mm.reserve_fixed(space1, GpuAddress(0x10001), 4096).is_err());
    assert!(mm.reserve_fixed(space1, GpuAddress(0x10000), 4095).is_err());
    assert!(mm.reserve_fixed(space1, GpuAddress(0), 4096).is_err()); // GPU addr 0 disallowed

    // Valid reservations in space1
    let r1 = mm
        .reserve_fixed(space1, GpuAddress(0x10000), 0x2000)
        .unwrap(); // [0x10000, 0x12000)
    let r2 = mm
        .reserve_fixed(space1, GpuAddress(0x12000), 0x2000)
        .unwrap(); // [0x12000, 0x14000)

    // Overlapping reservation in space1 fails
    assert!(
        mm.reserve_fixed(space1, GpuAddress(0x11000), 0x2000)
            .is_err()
    );
    assert!(
        mm.reserve_fixed(space1, GpuAddress(0x10000), 0x1000)
            .is_err()
    );
    assert!(
        mm.reserve_fixed(space1, GpuAddress(0x0f000), 0x2000)
            .is_err()
    );

    // Same GPU VA reservation in space2 (different address space) succeeds
    let r_space2 = mm
        .reserve_fixed(space2, GpuAddress(0x10000), 0x2000)
        .unwrap();

    // Map r1 to backing1 at offset 0
    let m1 = mm.map(space1, r1, backing1, 0).unwrap();
    assert_eq!(mm.mapping_epoch(space1).unwrap(), 1);

    // Mapping already mapped reservation fails
    assert!(mm.map(space1, r1, backing1, 0).is_err());

    // Mapping with unaligned offset fails
    assert!(mm.map(space1, r2, backing1, 1).is_err());

    // Mapping exceeding backing size fails
    assert!(mm.map(space1, r2, backing1, 0x3000).is_err()); // 0x3000 + 0x2000 = 0x5000 > 0x4000

    // Valid alias: map r2 to backing1 at offset 0x2000
    let m2 = mm.map(space1, r2, backing1, 0x2000).unwrap();
    assert_eq!(mm.mapping_epoch(space1).unwrap(), 2);

    // Verify AddressMap resolutions
    let map1 = mm.address_map(space1).unwrap();
    assert_eq!(
        map1.resolve(GpuAddress(0x10000), 0x2000).unwrap(),
        GuestAddress(0x1000)
    );
    assert_eq!(
        map1.resolve(GpuAddress(0x12000), 0x2000).unwrap(),
        GuestAddress(0x3000) // 0x1000 base + 0x2000 offset
    );

    // Query across adjacent mapping boundary must fail (single-contiguous-mapping borrow contract)
    assert!(map1.resolve(GpuAddress(0x11ff0), 0x20).is_err());

    // Cross-space alias: map r_space2 in space2 to backing2
    let m_s2 = mm.map(space2, r_space2, backing2, 0).unwrap();
    let map2 = mm.address_map(space2).unwrap();
    assert_eq!(
        map2.resolve(GpuAddress(0x10000), 0x2000).unwrap(),
        GuestAddress(0x8000)
    );

    // Dependency locks:
    // Backing1 cannot be released while m1 and m2 are mapped
    assert!(mm.release_backing(backing1).is_err());
    // Reservation r1 cannot be freed while m1 is mapped
    assert!(mm.free_reservation(space1, r1).is_err());
    // Space1 cannot be destroyed while reservations exist
    assert!(mm.destroy_space(space1).is_err());

    // Unmap m1: advances epoch, unmaps from AddressMap
    mm.unmap(space1, m1).unwrap();
    assert_eq!(mm.mapping_epoch(space1).unwrap(), 3);
    assert!(
        mm.address_map(space1)
            .unwrap()
            .resolve(GpuAddress(0x10000), 4)
            .is_err()
    );

    // Re-unmapping stale m1 fails
    assert!(mm.unmap(space1, m1).is_err());

    // Remap r1 to backing2: produces fresh MappingId != m1
    let m1_new = mm.map(space1, r1, backing2, 0).unwrap();
    assert_ne!(m1_new, m1);
    assert_eq!(mm.mapping_epoch(space1).unwrap(), 4);
    assert_eq!(
        mm.address_map(space1)
            .unwrap()
            .resolve(GpuAddress(0x10000), 4)
            .unwrap(),
        GuestAddress(0x8000)
    );

    // Clean teardown
    mm.unmap(space1, m1_new).unwrap();
    mm.unmap(space1, m2).unwrap();
    mm.free_reservation(space1, r1).unwrap();
    mm.free_reservation(space1, r2).unwrap();
    mm.destroy_space(space1).unwrap();
    mm.release_backing(backing1).unwrap();

    mm.unmap(space2, m_s2).unwrap();
    mm.free_reservation(space2, r_space2).unwrap();
    mm.destroy_space(space2).unwrap();
    mm.release_backing(backing2).unwrap();
}

// ---------------------------------------------------------------------------
// 4. Channel isolation, packet continuation, and independent latching
// ---------------------------------------------------------------------------

#[test]
fn channel_isolation_and_split_packet_continuation() {
    let mut channel_a = Channel::default();
    let mut channel_b = Channel::default();

    // Bind Maxwell B197 on Channel A only:
    // packet: 0x20010000 (inc, count 1, method 0 = bind class) + 0xb197
    channel_a.push_word(0x20010000).unwrap();
    channel_a.push_word(0xb197).unwrap();

    // Channel A now accepts an engine method (e.g. 0x1538 = 0, line 26 of FIXED_RASTER_STATE)
    // 0x2001054e (method 0x1538 / 4 = 0x54e, count 1)
    let effect_a = channel_a.push_word(0x2001054e).unwrap();
    assert_eq!(effect_a, wl_gpu::maxwell::MethodEffect::State); // header consumed
    let write_a = channel_a.push_word(0).unwrap();
    assert_eq!(write_a, wl_gpu::maxwell::MethodEffect::State);

    // Channel B was never bound to B197: sending the same engine method must fail on Channel B
    channel_b.push_word(0x2001054e).unwrap();
    let write_b = channel_b.push_word(0);
    assert!(
        write_b.is_err(),
        "engine write on unbound channel must fail"
    );
    assert!(channel_b.is_faulted());

    // Channel A is completely isolated and unaffected by Channel B's fault
    assert!(!channel_a.is_faulted());
    assert!(channel_b.is_faulted());

    // Packet split across calls on Channel A:
    // Non-incrementing packet with count 2: 0x6002054e (method 0x1538, count 2)
    // Push header and first value in chunk 1
    channel_a.push_word(0x6002054e).unwrap();
    channel_a.push_word(0).unwrap();
    // Interleave independent NOP on a third healthy channel
    let mut channel_c = Channel::default();
    channel_c.push_word(0).unwrap(); // NOP
    assert!(!channel_c.is_faulted());

    // Push second value on Channel A in chunk 2
    let write_a2 = channel_a.push_word(0).unwrap();
    assert_eq!(write_a2, wl_gpu::maxwell::MethodEffect::State);
    channel_a.finish().unwrap();
}

// ---------------------------------------------------------------------------
// 5. WGPU Driver integration, completion tracking, wait-without-readback,
//    and partial-fault draw preservation
// ---------------------------------------------------------------------------

#[cfg(feature = "wgpu-backend")]
fn make_driver_entry(address: u64, words: usize) -> [u8; 8] {
    (address | ((words as u64) << 42)).to_le_bytes()
}

#[cfg(feature = "wgpu-backend")]
fn make_driver_header(count: u32) -> [u8; 24] {
    let mut h = [0u8; 24];
    h[8..12].copy_from_slice(&count.to_le_bytes());
    h[12..16].copy_from_slice(&4u32.to_le_bytes()); // HW_FORMAT
    h
}

#[cfg(feature = "wgpu-backend")]
fn mem(data: &mut [u8]) -> SliceMemoryMut<'_> {
    SliceMemoryMut {
        base: GuestAddress(0),
        bytes: data,
    }
}

#[cfg(feature = "wgpu-backend")]
struct InjectedFailMemory<'a>(&'a mut [u8]);

#[cfg(feature = "wgpu-backend")]
impl GuestMemory for InjectedFailMemory<'_> {
    fn read(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<&[u8]> {
        let start = usize::try_from(a.0).map_err(|_| wl_gpu::Error("offset".into()))?;
        let end = start
            .checked_add(n)
            .ok_or_else(|| wl_gpu::Error("overflow".into()))?;
        self.0
            .get(start..end)
            .ok_or_else(|| wl_gpu::Error("unmapped read".into()))
    }
}

#[cfg(feature = "wgpu-backend")]
impl GuestMemoryWrite for InjectedFailMemory<'_> {
    fn validate_write(&self, a: GuestAddress, n: usize) -> wl_gpu::Result<()> {
        self.read(a, n).map(|_| ())
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        Err(wl_gpu::Error("injected guest write failure".into()))
    }
}

#[test]
#[cfg(feature = "wgpu-backend")]
#[ignore = "requires actual wgpu adapter; run explicitly"]
fn driver_completion_token_lifecycle_and_wait_without_readback() {
    let mut driver = GpuDriver::new().unwrap();
    let mut guest_data = vec![0u8; 0x10000];

    let backing = driver.register_backing(GuestAddress(0), 0x10000).unwrap();
    let space = driver.create_space().unwrap();
    let channel = driver.create_channel(space).unwrap();

    let r_cmd = driver
        .reserve_fixed(space, GpuAddress(0x10000), 0x4000)
        .unwrap();
    driver.map(space, r_cmd, backing, 0).unwrap();

    // 1. Submit NOP entry: 1 word of 0 (NOP) at 0x10000
    guest_data[0..4].copy_from_slice(&0u32.to_le_bytes());
    let entry_bytes = make_driver_entry(0x10000, 1);
    let hdr = make_driver_header(1);

    let report = driver
        .submit(
            channel,
            KICKOFF_PB,
            &hdr,
            Some(&entry_bytes),
            &mut mem(&mut guest_data),
        )
        .unwrap();
    assert!(report.fault.is_none());
    assert_eq!(report.progress.entries_completed, 1);
    assert_eq!(report.progress.draws_submitted, 0);

    // Query on NOP: already host_completed and guest_visible because no GPU target was written
    let query_status = driver.query(report.token).unwrap();
    assert!(query_status.host_completed);
    assert!(query_status.guest_visible);

    // Wait on NOP: must not add readbacks or queue submissions
    let before_counters = driver.graphics_counters();
    let wait_status = driver.wait(report.token).unwrap();
    assert!(wait_status.host_completed);
    assert_eq!(
        driver.graphics_counters().readback_operations,
        before_counters.readback_operations
    );
    assert_eq!(
        driver.graphics_counters().submissions,
        before_counters.submissions
    );

    // Retire completed token
    driver.retire(report.token).unwrap();

    // Query on retired token must fail
    assert!(driver.query(report.token).is_err());
    // Second retire on same token must fail
    assert!(driver.retire(report.token).is_err());

    // 2. Capacity limit check: fill records up to MAX_COMPLETIONS (64)
    let mut tokens = Vec::new();
    for _ in 0..MAX_COMPLETIONS {
        let rep = driver
            .submit(
                channel,
                KICKOFF_PB,
                &hdr,
                Some(&entry_bytes),
                &mut mem(&mut guest_data),
            )
            .unwrap();
        tokens.push(rep.token);
    }
    // 65th submission must be rejected with capacity error
    assert!(
        driver
            .submit(
                channel,
                KICKOFF_PB,
                &hdr,
                Some(&entry_bytes),
                &mut mem(&mut guest_data),
            )
            .is_err(),
        "65th submission must exceed MAX_COMPLETIONS"
    );

    // Retire one token: allows next submission to succeed
    driver.retire(tokens[0]).unwrap();
    let replacement = driver
        .submit(
            channel,
            KICKOFF_PB,
            &hdr,
            Some(&entry_bytes),
            &mut mem(&mut guest_data),
        )
        .unwrap();
    driver.retire(replacement.token).unwrap();

    for t in &tokens[1..] {
        driver.retire(*t).unwrap();
    }

    driver.close(channel, &mut mem(&mut guest_data)).unwrap();
}

#[test]
#[cfg(feature = "wgpu-backend")]
#[ignore = "requires actual wgpu adapter; run explicitly"]
fn driver_partial_fault_draw_preservation_and_publication_retry() {
    let desc = TextureImage::decode(TIC_RGBA)
        .unwrap()
        .with_sampler(TextureSampler::decode(TSC_NEAREST).unwrap());
    let (surface, _images) = rgba_surface(desc);

    let mut data = vec![0u8; 0x18000];
    data[..64].copy_from_slice(&descriptor_bytes(TIC_RGBA, TSC_NEAREST));
    data[0x1000..0x1000 + surface.len()].copy_from_slice(&surface);

    for (off, bin) in [
        (
            0x4000,
            include_bytes!("fixtures/maxwell/tex.bin").as_slice(),
        ),
        (
            0x4200,
            include_bytes!("fixtures/maxwell/vertex.bin").as_slice(),
        ),
    ] {
        data[off..off + bin.len()].copy_from_slice(bin);
    }

    // Triangle vertices (x, y, z, w, u, v)
    let pos = [[-0.8f32, -0.6], [0.6, -0.4], [-0.2, 0.7]];
    let uv = [[0.1f32, 0.2], [0.9, 0.1], [0.25, 0.85]];
    for n in 0..3 {
        for (c, v) in [pos[n][0], pos[n][1], 0., 1., uv[n][0], uv[n][1]]
            .into_iter()
            .enumerate()
        {
            data[0x3000 + n * 24 + c * 4..0x3004 + n * 24 + c * 4]
                .copy_from_slice(&v.to_le_bytes());
        }
    }
    for (n, i) in [0u16, 1, 2].into_iter().enumerate() {
        data[0x6000 + n * 2..0x6002 + n * 2].copy_from_slice(&i.to_le_bytes());
    }
    let rt_offset = 0x8000;
    let rt_size = 208 * 29;
    let bg = [11u8, 17, 23, 255];
    for p in data[rt_offset..rt_offset + rt_size].chunks_exact_mut(4) {
        p.copy_from_slice(&bg);
    }

    // Build valid B197 draw sequence
    let mut words = common::hardware::setup();
    for (m, v) in [
        (0x17c8, 0),
        (0x17cc, 0x80000),
        (0x17d0, 0),
        (0x17d4, 0x80005),
        (0x17d8, 1),
        (0x1434, 0),
        (0x1438, 0),
        (0x1644, 0),
    ] {
        words.extend([0x20010000 | (m / 4), v]);
    }
    words.extend([0x20010586, 4, 0x200105f7, 0, 0x200105f8, 3, 0x20010585, 0]);
    for word in &mut words {
        if *word == 0x31000 {
            *word = 0x31020;
        } else if *word == 0x40000 {
            *word = 0x40040;
        }
    }

    // Append an invalid method opcode (0xe0000000) at the end to trigger a fault after the draw
    words.push(0xe0000000);

    let cmd_len = words.len();
    for (b, w) in data[0x10000..0x10000 + cmd_len * 4]
        .chunks_exact_mut(4)
        .zip(&words)
    {
        b.copy_from_slice(&w.to_le_bytes());
    }

    let mut driver = GpuDriver::new().unwrap();
    let backing = driver.register_backing(GuestAddress(0), 0x18000).unwrap();
    let space = driver.create_space().unwrap();
    let channel = driver.create_channel(space).unwrap();

    for (gpu, offset, size) in [
        (0x10000, 0x1000, 0x2000),
        (0x30000, 0, 0x1000),
        (0x31000, 0, 0x1000),
        (0x40000, 0, 0x1000),
        (0x50000, 0x4000, 0x1000),
        (0x70000, 0x3000, 0x1000),
        (0x80000, 0x6000, 0x1000),
        (0x90000, rt_offset as u64, 0x2000),
        (0xb0000, 0x10000, 0x4000),
    ] {
        let r = driver.reserve_fixed(space, GpuAddress(gpu), size).unwrap();
        driver.map(space, r, backing, offset).unwrap();
    }

    let entry_bytes = make_driver_entry(0xb0000, cmd_len);
    let hdr = make_driver_header(1);

    // Submit: must report partial fault after 1 submitted draw
    let report = driver
        .submit(
            channel,
            KICKOFF_PB,
            &hdr,
            Some(&entry_bytes),
            &mut mem(&mut data),
        )
        .unwrap();
    assert!(
        report.fault.is_some(),
        "expected execution fault from 0xe0000000"
    );
    assert_eq!(report.progress.draws_submitted, 1);
    assert!(driver.has_pending_output());

    // Channel is now latched faulted: replay must fail
    assert!(
        driver
            .submit(
                channel,
                KICKOFF_PB,
                &hdr,
                Some(&entry_bytes),
                &mut mem(&mut data),
            )
            .is_err()
    );

    // Wait for the submitted draw: queue work is complete
    let wait_status = driver.wait(report.token).unwrap();
    assert!(wait_status.host_completed);
    assert!(!wait_status.guest_visible);

    // Injected memory write failure: publication fails, but pending output survives
    let draws_before = driver.graphics_counters().gpu_target_writes;
    assert!(
        driver
            .publish(channel, &mut InjectedFailMemory(&mut data))
            .is_err()
    );
    assert!(
        driver.has_pending_output(),
        "pending output must survive write failure"
    );

    // Second publication with valid memory succeeds without replaying any draw
    let frame = driver
        .publish(channel, &mut mem(&mut data))
        .unwrap()
        .unwrap();
    assert_eq!(
        driver.graphics_counters().gpu_target_writes,
        draws_before,
        "publication retry must not issue extra draws"
    );

    // Verify rendered pixels: non-background texels exist inside the triangle
    let mut inside_count = 0;
    for y in 0..29 {
        for x in 0..52 {
            let pixel = &frame.rgba[(y * 52 + x) * 4..(y * 52 + x + 1) * 4];
            if pixel != bg {
                inside_count += 1;
            }
        }
    }
    assert!(
        inside_count > 100,
        "rendered triangle must have colored pixels"
    );

    // Clean close
    driver.close(channel, &mut mem(&mut data)).unwrap();
}
