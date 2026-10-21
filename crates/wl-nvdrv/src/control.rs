//! Inline Switch GPU identity queries, not host-adapter capabilities or pointers.
//! Wire offsets: libnx feebd026ca0f5dcc2119f46ad8e0d16ad3dd4973,
//! nx/source/nvidia/ioctl/nvhost-ctrl-gpu.c and nx/include/switch/nvidia/ioctl.h.
use crate::{Ioctl, NvError, Result, long, out32, out64, word};

pub(super) fn ioctl(r: Ioctl<'_>) -> Result<()> {
    match r.request {
        0xc0b04705 => characteristics(r.input, r.output),
        0xc0184706 => tpc_masks(r.input, r.output),
        _ => Err(NvError::NotSupported.into()),
    }
}

fn characteristics(input: &[u8], output: &mut [u8]) -> Result<()> {
    if input.len() != 176 || output.len() != 176 || long(input, 0) < 160 {
        return Err(NvError::InvalidSize.into());
    }
    if long(input, 8) == 0 {
        return Err(NvError::BadParameter.into());
    }
    output.fill(0);
    out64(output, 0, 160);
    // Switch returns the structure inline. The nonzero address is an opaque
    // ABI field, including 1 in libnx; never resolve or dereference it.
    out64(output, 8, long(input, 8));
    let gc = &mut output[16..];
    for (offset, value) in [
        (0, 0x120), // GM200 architecture
        (4, 0xb),   // GM20B implementation
        (8, 0xa1),
        (12, 1), // One GPC, two TPCs: the emulated chip, not the host GPU.
        (32, 2),
        (36, 0x20), // AXI bus
        (40, 0x20000),
        (44, 0x20000),
        (48, 27),
        (52, 0x30000),
        (56, 1),
        (60, 0x503),
        (64, 0x503),
        (68, 0x80),
        (72, 40),
        (88, 0x902d),
        (92, 0xb197),
        (96, 0xb1c0),
        (100, 0xb06f),
        (104, 0xa140),
        (108, 0xb0b5),
        (112, 1),
        (120, 2),
        (124, 1),
        (132, 1),
        (136, 0x21d70),
    ] {
        out32(gc, offset, value);
    }
    out64(gc, 16, 0x40000);
    out64(gc, 144, 0x6230326d67); // Little-endian "gm20b".
    // Flags remain zero: do not advertise sparse mappings, cycle statistics,
    // sync-fence FDs or general Host1x syncpoint operations we do not implement.
    // Class identifiers describe the guest ISA, not universal opcode support.
    Ok(())
}

fn tpc_masks(input: &[u8], output: &mut [u8]) -> Result<()> {
    if input.len() != 24 || output.len() != 24 {
        return Err(NvError::InvalidSize.into());
    }
    // Bounded one-GPC query. Other buffer shapes remain unsupported.
    if word(input, 0) != 4 {
        return Err(NvError::NotSupported.into());
    }
    if word(input, 4) != 0 || long(input, 8) == 0 {
        return Err(NvError::BadParameter.into());
    }
    output.copy_from_slice(input);
    output[16..].fill(0);
    out32(output, 16, 3); // TPCs zero and one in GPC zero.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn characteristics_wire_offsets_and_opaque_pointer() {
        let mut input = [0xa5; 176];
        input[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        input[8..16].copy_from_slice(&1u64.to_le_bytes());
        let mut out = [0xcc; 176];
        characteristics(&input, &mut out).unwrap();
        assert_eq!(
            &out[..16],
            &[160, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]
        );
        // Literal offsets independently counted from the published C structure.
        assert_eq!(
            &out[16..32],
            &[0x20, 1, 0, 0, 11, 0, 0, 0, 0xa1, 0, 0, 0, 1, 0, 0, 0]
        );
        assert_eq!(&out[32..40], &[0, 0, 4, 0, 0, 0, 0, 0]);
        assert_eq!(&out[88..92], &[40, 0, 0, 0]);
        assert_eq!(&out[92..104], &[0; 12]); // reserved + flags
        assert_eq!(&out[108..112], &[0x97, 0xb1, 0, 0]);
        assert_eq!(&out[116..120], &[0x6f, 0xb0, 0, 0]);
        assert_eq!(&out[160..168], b"gm20b\0\0\0");
        assert_eq!(&out[168..], &[0; 8]);
        assert_eq!(input[16], 0xa5);
    }
    #[test]
    fn malformed_characteristics_never_publish_partial_output() {
        for (input_len, output_len, size, address) in [
            (0, 176, 160u64, 1u64),
            (175, 176, 160, 1),
            (176, 175, 160, 1),
            (177, 176, 160, 1),
            (176, 176, 0, 1),
            (176, 176, 159, 1),
            (176, 176, 160, 0),
        ] {
            let mut input = vec![0; input_len];
            if input.len() >= 16 {
                input[..8].copy_from_slice(&size.to_le_bytes());
                input[8..16].copy_from_slice(&address.to_le_bytes());
            }
            let mut output = vec![0xcc; output_len];
            assert!(characteristics(&input, &mut output).is_err());
            assert!(output.iter().all(|b| *b == 0xcc));
        }
    }
    #[test]
    fn tpc_mask_is_inline_and_has_no_padding_leak() {
        let mut input = [0xcc; 24];
        input[..8].copy_from_slice(&4u64.to_le_bytes());
        input[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        let mut output = [0xdd; 24];
        tpc_masks(&input, &mut output).unwrap();
        assert_eq!(&output[..16], &input[..16]);
        assert_eq!(&output[16..], &[3, 0, 0, 0, 0, 0, 0, 0]);
        for index in [0, 4, 8] {
            let mut invalid = input;
            invalid[index..index + 4].fill(0);
            if index == 4 {
                invalid[4] = 1;
            }
            if index == 8 {
                invalid[8..16].fill(0);
            }
            let mut output = [0xdd; 24];
            assert!(tpc_masks(&invalid, &mut output).is_err());
            assert_eq!(output, [0xdd; 24]);
        }
    }
}
