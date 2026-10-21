use wl_gpu::{maxwell::*, mme::*};
fn fixture() -> Vec<u32> {
    include_bytes!("fixtures/mme/state-mask.bin")
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect()
}
fn loaded() -> MacroEngine {
    let mut mme = MacroEngine::default();
    mme.upload(0x11c, 3).unwrap();
    mme.upload(0x120, 5).unwrap();
    mme.upload(0x114, 5).unwrap();
    for word in fixture() {
        mme.upload(0x118, word).unwrap();
    }
    mme
}
#[test]
fn independent_assembler_macro_uses_parameter_and_executes_exit_delay_slot() {
    assert_eq!(fixture(), [0x01a00021, 0x000008c0, 0x00000011]);
    let mut mme = loaded();
    // Both RAM pointers auto-increment. Bind the next entry without a selector.
    mme.upload(0x120, 5).unwrap();
    assert_eq!(mme.execute(4, 0x1111), Ok((0x1a00, 0x1111)));
    for value in [0, 1, 0x1111, u32::MAX] {
        assert_eq!(mme.execute(3, value), Ok((0x1a00, value)));
    }
    assert_eq!(mme.execute(0, 0), Err(MacroError::UnboundEntry(0)));
    // Exit does not suppress its delay slot; an unsupported delay-slot read fails.
    mme.upload(0x114, 7).unwrap();
    mme.upload(0x118, 0x15).unwrap();
    assert_eq!(mme.execute(3, 1), Err(MacroError::UnsupportedRead(0)));
}
#[test]
fn uploads_are_bounded_and_invalid_updates_do_not_replace_a_valid_entry() {
    let mut mme = MacroEngine::default();
    assert_eq!(mme.upload(0x118, 0), Err(MacroError::MissingUploadPointer));
    assert_eq!(mme.upload(0x120, 0), Err(MacroError::MissingUploadPointer));
    assert!(mme.upload(0x11c, ENTRIES as u32).is_err());
    assert!(mme.upload(0x114, CODE_WORDS as u32).is_err());
    let mut mme = loaded();
    assert!(mme.upload(0x120, CODE_WORDS as u32).is_err());
    assert_eq!(mme.execute(3, 0x101), Ok((0x1a00, 0x101)));
    mme.upload(0x114, CODE_WORDS as u32 - 1).unwrap();
    mme.upload(0x118, 0x11).unwrap();
    assert!(mme.upload(0x118, 0).is_err());
    assert_eq!(mme.execute(3, 1), Ok((0x1a00, 1)));
    assert_eq!(mme.upload(0x124, 2), Ok(false));
    mme.upload(0x11c, ENTRIES as u32 - 1).unwrap();
    mme.upload(0x120, 5).unwrap();
    assert!(mme.upload(0x120, 5).is_err());
}
#[test]
fn incomplete_uninitialized_unbounded_and_multi_emit_programs_fail_explicitly() {
    let mut mme = loaded();
    mme.upload(0x114, 5).unwrap();
    mme.upload(0x118, 0x00001011).unwrap(); // ADDI reads uninitialized R2.
    assert_eq!(mme.execute(3, 1), Err(MacroError::UninitializedRegister(2)));
    mme.upload(0x114, 5).unwrap();
    for _ in 0..64 {
        mme.upload(0x118, 0x11).unwrap();
    }
    assert_eq!(mme.execute(3, 1), Err(MacroError::MissingExit));
    mme.upload(0x114, 5).unwrap();
    for word in [0x01a00021, 0x840, 0x8c0, 0x11] {
        mme.upload(0x118, word).unwrap();
    }
    assert_eq!(mme.execute(3, 1), Err(MacroError::UnsupportedEmissionCount));
}
#[test]
fn macro_dispatch_updates_state_only_and_preserves_failure_latching() {
    let mut channel = Channel::default();
    let mut write = |method, value| {
        channel
            .push_word(0x20010000 | (u32::from(method) / 4))
            .unwrap();
        channel.push_word(value)
    };
    write(0u16, 0xb197).unwrap();
    write(0x11c, 0).unwrap();
    write(0x120, 0).unwrap();
    write(0x114, 0).unwrap();
    for word in fixture() {
        write(0x118, word).unwrap();
    }
    assert_eq!(write(0x3800, 0x1111).unwrap(), MethodEffect::State);
    // Invalid CT mask is rejected by the existing raster validator.
    assert!(write(0x3800, 0xffff).is_err());
    assert!(channel.is_faulted());
    assert!(channel.push_word(0).is_err());
}

#[test]
fn assembler_signed_immediate_wraps_and_macro_draw_is_rejected_before_execution() {
    let mut mme = loaded();
    mme.upload(0x114, 5).unwrap();
    // envyas: maddr 0x680; mov $r2 -1; exit send (add $r2 2); nop;
    for word in [0x01a00021, 0xffffc211, 0x000090c1, 0x11] {
        mme.upload(0x118, word).unwrap();
    }
    assert_eq!(mme.execute(3, 0), Ok((0x1a00, 1)));
    let mut engine = Maxwell3d::default();
    for (method, value) in [
        (0, 0xb197),
        (0x11c, 0),
        (0x120, 0),
        (0x114, 0),
        // envyas: maddr 0x485; exit send (add $r1 0); nop;
        (0x118, 0x01214021),
        (0x118, 0x8c0),
        (0x118, 0x11),
    ] {
        engine
            .apply(wl_gpu::fifo::MethodWrite {
                subchannel: 0,
                method,
                value,
            })
            .unwrap();
    }
    assert_eq!(
        engine.apply(wl_gpu::fifo::MethodWrite {
            subchannel: 0,
            method: 0x3800,
            value: 3
        }),
        Err(MethodError::Macro(MacroError::UnsupportedOutput(0x1214)))
    );
}

#[test]
fn scratch_read_uses_word_address_and_latest_explicit_state() {
    let mut mme = MacroEngine::default();
    mme.upload(0x11c, 0).unwrap();
    mme.upload(0x120, 0).unwrap();
    mme.upload(0x114, 0).unwrap();
    // Independently assembled: maddr 0x680; read $r2 0xd1a;
    // exit send (add $r2 0); nop;
    for word in [0x01a00021, 0x03468215, 0x10c0, 0x11] {
        mme.upload(0x118, word).unwrap();
    }
    assert_eq!(
        mme.execute(0, 7),
        Err(MacroError::UninitializedScratch(0x3468))
    );
    for value in [0, 0x1111, u32::MAX] {
        assert_eq!(mme.upload(0x3468, value), Ok(true));
        assert_eq!(mme.execute(0, 7), Ok((0x1a00, value)));
    }
    // Neighboring locations do not alias; a separate channel has no such data.
    mme.upload(0x3464, 1).unwrap();
    mme.upload(0x346c, 2).unwrap();
    assert_eq!(mme.execute(0, 0), Ok((0x1a00, u32::MAX)));
    assert_eq!(
        MacroEngine::default().execute(0, 0),
        Err(MacroError::UnboundEntry(0))
    );
    // No general method read is silently supplied as zero.
    mme.upload(0x114, 1).unwrap();
    mme.upload(0x118, 0x01a00215).unwrap(); // read $r2 0x680
    assert_eq!(mme.execute(0, 0), Err(MacroError::UnsupportedRead(0x680)));
}

#[test]
fn scratch_range_and_alignment_are_checked_before_channel_mutation() {
    use wl_gpu::fifo::MethodWrite;
    let mut engine = Maxwell3d::default();
    let mut apply = |method, value| {
        engine.apply(MethodWrite {
            subchannel: 0,
            method,
            value,
        })
    };
    apply(0, 0xb197).unwrap();
    for method in [0x3400, 0x3468, 0x37fc] {
        assert_eq!(apply(method, u32::MAX), Ok(MethodEffect::State));
    }
    assert_eq!(apply(0x3469, 0), Err(MethodError::InvalidMethod(0x3469)));
    assert_eq!(
        apply(0x33fc, 0),
        Err(MethodError::UnsupportedMethod(0x33fc))
    );
    assert!(apply(0x3804, 0).is_err());
}

#[test]
fn scratch_reads_cover_all_slots_signed_addressing_and_channel_isolation() {
    let load = || {
        let mut mme = MacroEngine::default();
        for (method, value) in [(0x11c, 0), (0x120, 0), (0x114, 0)] {
            mme.upload(method, value).unwrap();
        }
        // envyas: maddr 0x680; read $r2 (add $r1 -1);
        // exit send (add $r2 0); nop;
        for word in [0x01a00021, 0xffffca15, 0x10c0, 0x11] {
            mme.upload(0x118, word).unwrap();
        }
        mme
    };
    let mut first = load();
    let second = load();
    for i in 0..256u16 {
        let byte_address = 0x3400 + 4 * i;
        let value = u32::from(i).wrapping_mul(0x1020_3041);
        first.upload(byte_address, value).unwrap();
        let argument = u32::from(byte_address / 4) + 1;
        assert_eq!(first.execute(0, argument), Ok((0x1a00, value)));
        assert_eq!(
            second.execute(0, argument),
            Err(MacroError::UninitializedScratch(byte_address))
        );
    }
    for (argument, address) in [(0, u32::MAX), (1, 0), (0xd00, 0xcff), (0xe01, 0xe00)] {
        assert_eq!(
            first.execute(0, argument),
            Err(MacroError::UnsupportedRead(address))
        );
    }
}
