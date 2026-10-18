use wl_runtime::display::{Framebuffer, MAX_RGBA_BYTES, PixelFormat};

#[test]
fn rgba_rows_ignore_padding_and_rgbx_alpha_is_opaque() {
    let bytes = vec![
        1, 2, 3, 4, 5, 6, 7, 8, 91, 92, 93, 94, 9, 10, 11, 12, 13, 14, 15, 16,
    ];
    let frame = Framebuffer::new(2, 2, 3, PixelFormat::Rgba8888, bytes.clone());
    assert_eq!(
        frame.to_rgba8888().unwrap(),
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
    );
    assert_eq!(frame.pixels, bytes);
    let rgbx = Framebuffer::new(2, 2, 3, PixelFormat::Rgbx8888, bytes);
    assert_eq!(
        rgbx.to_rgba8888().unwrap(),
        [1, 2, 3, 255, 5, 6, 7, 255, 9, 10, 11, 255, 13, 14, 15, 255]
    );
}

#[test]
fn bgra_and_565_colors_have_independent_byte_expectations() {
    let bgra = Framebuffer::new(
        3,
        1,
        3,
        PixelFormat::Bgra8888,
        vec![200, 20, 3, 40, 9, 8, 7, 6, 0, 255, 17, 128],
    );
    assert_eq!(
        bgra.to_rgba8888().unwrap(),
        [3, 20, 200, 40, 7, 8, 9, 6, 17, 255, 0, 128]
    );
    let rgb565 = Framebuffer::new(
        3,
        2,
        4,
        PixelFormat::Rgb565,
        vec![
            0x00, 0xf8, 0xe0, 0x07, 0x1f, 0x00, 0xaa, 0xaa, 0xff, 0xff, 0, 0, 0x10, 0x84,
        ],
    );
    assert_eq!(
        rgb565.to_rgba8888().unwrap(),
        [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255, 131,
            129, 131, 255
        ]
    );
    let ppm = bgra.to_ppm().unwrap();
    assert_eq!(ppm, b"P6\n3 1\n255\n\x03\x14\xc8\x07\x08\x09\x11\xff\x00");
}

#[test]
fn malformed_geometry_and_truncated_storage_are_errors() {
    for (width, height, stride) in [
        (0, 1, 1),
        (1, 0, 1),
        (2, 2, 1),
        (u32::MAX, u32::MAX, u32::MAX),
        (4097, 4097, 4097),
    ] {
        let frame = Framebuffer::new(width, height, stride, PixelFormat::Rgba8888, vec![0; 16]);
        assert!(frame.to_rgba8888().is_err());
        assert!(frame.to_ppm().is_err());
    }
    assert_eq!(MAX_RGBA_BYTES, 64 * 1024 * 1024);
    for format in [
        PixelFormat::Rgba8888,
        PixelFormat::Rgbx8888,
        PixelFormat::Bgra8888,
        PixelFormat::Rgb565,
    ] {
        let bpp = if format == PixelFormat::Rgb565 { 2 } else { 4 };
        let required = (3 + 2) * bpp; // Two visible pixels, stride three, two rows.
        let short = Framebuffer::new(2, 2, 3, format, vec![0; required - 1]);
        assert!(short.to_rgba8888().is_err());
        let exact = Framebuffer::new(2, 2, 3, format, vec![0; required]);
        assert_eq!(exact.to_rgba8888().unwrap().len(), 16);
    }
    assert!(
        Framebuffer::new(1, 1, 1, PixelFormat::Unknown, vec![1, 2, 3, 4])
            .to_rgba8888()
            .is_err()
    );
}
