//! The controller query uses real device initialization, but performs no draw.
use wl_gpu::memory::{GuestAddress, GuestMemory, GuestMemoryWrite};
use wl_nvdrv::{Driver, Error, Ioctl, IoctlKind, Memory, NvError};
struct NoGuestAccess;
impl GuestMemory for NoGuestAccess {
    fn read(&self, _: GuestAddress, _: usize) -> wl_gpu::Result<&[u8]> {
        panic!("inline query must not dereference a guest pointer")
    }
}
impl GuestMemoryWrite for NoGuestAccess {
    fn validate_write(&self, _: GuestAddress, _: usize) -> wl_gpu::Result<()> {
        panic!("controller query must not publish guest memory")
    }
    fn write(&mut self, _: GuestAddress, _: &[u8]) -> wl_gpu::Result<()> {
        panic!("controller query must not publish guest memory")
    }
}
impl Memory for NoGuestAccess {
    type Pin = ();
    fn pin(&mut self, _: GuestAddress, _: u64) -> wl_gpu::Result<()> {
        panic!("controller query must not pin memory")
    }
    fn unpin(&mut self, _: ()) -> wl_gpu::Result<()> {
        panic!("controller close must not release another device's pin")
    }
}
fn query(
    d: &mut Driver<()>,
    fd: u32,
    kind: IoctlKind,
    request: u32,
    input: &[u8],
    output: &mut [u8],
) -> wl_nvdrv::Result<()> {
    d.ioctl(
        Ioctl {
            fd,
            kind,
            request,
            input,
            output,
            extra_input: &[],
            extra_output: &mut [],
        },
        &mut NoGuestAccess,
    )
}
#[test]
#[ignore = "initializes an actual wgpu device; execute explicitly"]
fn control_gpu_identity_and_fd_isolation_do_not_submit_work() {
    let mut d = Driver::new().unwrap();
    let fd = d.open("/dev/nvhost-ctrl-gpu").unwrap();
    let nvmap = d.open("/dev/nvmap").unwrap();
    let before = d.graphics_counters();
    let mut input = [0x77; 176];
    input[..8].copy_from_slice(&160u64.to_le_bytes());
    input[8..16].copy_from_slice(&1u64.to_le_bytes());
    let mut output = [0xcc; 176];
    for _ in 0..32 {
        query(
            &mut d,
            fd,
            IoctlKind::Ioctl,
            0xc0b04705,
            &input,
            &mut output,
        )
        .unwrap();
        assert_eq!(&output[16..20], &[0x20, 1, 0, 0]);
        assert_eq!(&output[108..112], &[0x97, 0xb1, 0, 0]);
        assert_eq!(&output[96..104], &[0; 8]);
        assert_eq!(&output[160..168], b"gm20b\0\0\0");
    }
    for (wrong_fd, kind, request, expected) in [
        (nvmap, IoctlKind::Ioctl, 0xc0b04705, NvError::NotSupported),
        (
            u32::MAX,
            IoctlKind::Ioctl,
            0xc0b04705,
            NvError::BadParameter,
        ),
        (fd, IoctlKind::Ioctl2, 0xc0b04705, NvError::NotSupported),
        (fd, IoctlKind::Ioctl, 0xc0b04704, NvError::NotSupported),
        (fd, IoctlKind::Ioctl, 0x80b04705, NvError::InvalidSize),
    ] {
        output.fill(0xcc);
        assert!(
            matches!(query(&mut d,wrong_fd,kind,request,&input,&mut output),Err(Error::Driver(e)) if e==expected)
        );
        assert_eq!(output, [0xcc; 176]);
    }
    let mut mask = [0u8; 24];
    mask[..4].copy_from_slice(&4u32.to_le_bytes());
    mask[8] = 1;
    let mut masks = [0xcc; 24];
    query(&mut d, fd, IoctlKind::Ioctl, 0xc0184706, &mask, &mut masks).unwrap();
    assert_eq!(&masks[16..], &[3, 0, 0, 0, 0, 0, 0, 0]);
    // Ioctl3 uses the explicit second output, not the legacy address field.
    mask.fill(0);
    mask[0] = 8;
    let mut inline = [0xcc; 8];
    d.ioctl(
        Ioctl {
            fd,
            request: 0xc0184706,
            kind: IoctlKind::Ioctl3,
            input: &mask,
            output: &mut masks,
            extra_input: &[],
            extra_output: &mut inline,
        },
        &mut NoGuestAccess,
    )
    .unwrap();
    assert_eq!(inline, [3, 0, 0, 0, 0, 0, 0, 0]);
    masks.fill(0xcc);
    inline.fill(0xdd);
    assert!(matches!(
        d.ioctl(
            Ioctl {
                fd: nvmap,
                request: 0xc0184706,
                kind: IoctlKind::Ioctl3,
                input: &mask,
                output: &mut masks,
                extra_input: &[],
                extra_output: &mut inline,
            },
            &mut NoGuestAccess
        ),
        Err(Error::Driver(NvError::NotSupported))
    ));
    assert_eq!(masks, [0xcc; 24]);
    assert_eq!(inline, [0xdd; 8]);
    d.close(fd, &mut NoGuestAccess).unwrap();
    assert!(matches!(
        query(
            &mut d,
            fd,
            IoctlKind::Ioctl,
            0xc0b04705,
            &input,
            &mut output
        ),
        Err(Error::Driver(NvError::BadParameter))
    ));
    let reopened = d.open("/dev/nvhost-ctrl-gpu").unwrap();
    assert_ne!(fd, reopened);
    d.close(reopened, &mut NoGuestAccess).unwrap();
    d.close(nvmap, &mut NoGuestAccess).unwrap();
    let after = d.graphics_counters();
    assert_eq!(after.submissions, before.submissions);
    assert_eq!(after.draw_commands, before.draw_commands);
    assert_eq!(after.upload_bytes, before.upload_bytes);
    assert_eq!(after.readback_operations, before.readback_operations);
}

#[test]
#[ignore = "initializes an actual wgpu device; execute explicitly"]
fn explicit_address_space_configuration_is_checked_before_state_changes() {
    let mut d = Driver::new().unwrap();
    let fd = d.open("/dev/nvhost-as-gpu").unwrap();
    let mut input = [0; 40];
    input[0] = 1;
    input[8..12].copy_from_slice(&0x10000u32.to_le_bytes());
    for (offset, value) in [(16, 0x4000000u64), (24, 0x2000000000), (32, 0x400000000)] {
        input[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    let mut bad = input;
    bad[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        query(&mut d, fd, IoctlKind::Ioctl, 0x40284109, &bad, &mut []),
        Err(Error::Driver(NvError::NotSupported))
    ));
    query(&mut d, fd, IoctlKind::Ioctl, 0x40284109, &input, &mut []).unwrap();
    assert!(matches!(
        query(&mut d, fd, IoctlKind::Ioctl, 0x40284109, &input, &mut []),
        Err(Error::Driver(NvError::AlreadyAllocated))
    ));
    let before = d.graphics_counters();
    let mut region_input = [0xa5; 64];
    region_input[..8].fill(0);
    region_input[8..12].copy_from_slice(&48u32.to_le_bytes());
    region_input[12..16].fill(0);
    let mut region_output = [0xcc; 64];
    query(
        &mut d,
        fd,
        IoctlKind::Ioctl,
        0xc0404108,
        &region_input,
        &mut region_output,
    )
    .unwrap();
    assert_eq!(&region_output[16..24], &0x4000000u64.to_le_bytes());
    assert_eq!(&region_output[40..48], &0x400000000u64.to_le_bytes());
    let expected = region_output;
    for length in [47, 48] {
        region_output.fill(0xcc);
        let mut inline = [0xdd; 48];
        let result = d.ioctl(
            Ioctl {
                fd,
                request: 0xc0404108,
                kind: IoctlKind::Ioctl3,
                input: &region_input,
                output: &mut region_output,
                extra_input: &[],
                extra_output: &mut inline[..length],
            },
            &mut NoGuestAccess,
        );
        if length == 47 {
            assert!(matches!(result, Err(Error::Driver(NvError::InvalidSize))));
            assert_eq!(region_output, [0xcc; 64]);
            assert_eq!(inline, [0xdd; 48]);
        } else {
            result.unwrap();
            assert_eq!(region_output, expected);
            assert_eq!(inline, expected[16..]);
        }
    }
    let mut reserve = [0; 24];
    reserve[..4].copy_from_slice(&1u32.to_le_bytes());
    reserve[4..8].copy_from_slice(&65536u32.to_le_bytes());
    reserve[8] = 1;
    reserve[16..].copy_from_slice(&0x400000000u64.to_le_bytes());
    let mut output = [0xcc; 24];
    query(
        &mut d,
        fd,
        IoctlKind::Ioctl,
        0xc0184102,
        &reserve,
        &mut output,
    )
    .unwrap();
    assert_eq!(reserve, output);
    let mut invalid = reserve;
    invalid[16..].copy_from_slice(&0x2000000000u64.to_le_bytes());
    output.fill(0xcc);
    assert!(matches!(
        query(
            &mut d,
            fd,
            IoctlKind::Ioctl,
            0xc0184102,
            &invalid,
            &mut output
        ),
        Err(Error::Driver(NvError::InvalidAddress))
    ));
    assert_eq!(output, [0xcc; 24]);
    let mut free = [0; 16];
    free[..8].copy_from_slice(&0x400000000u64.to_le_bytes());
    free[8..12].copy_from_slice(&1u32.to_le_bytes());
    free[12..].copy_from_slice(&65536u32.to_le_bytes());
    query(
        &mut d,
        fd,
        IoctlKind::Ioctl,
        0xc0104103,
        &free,
        &mut [0; 16],
    )
    .unwrap();
    d.close(fd, &mut NoGuestAccess).unwrap();
    assert_eq!(d.graphics_counters().submissions, before.submissions);
    assert_eq!(d.graphics_counters().draw_commands, before.draw_commands);
}
