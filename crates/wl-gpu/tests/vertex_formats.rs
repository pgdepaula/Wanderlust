use wl_gpu::{
    fifo::MethodWrite,
    maxwell::{Channel, Maxwell3d, MethodEffect, MethodError, PositionFormat, VertexArray},
    memory::{AddressMap, GpuAddress, GuestAddress, SliceMemory},
    shader_program::{FragmentProgram, Operation, VertexProgram},
};

fn set(engine: &mut Maxwell3d, method: u16, value: u32) -> Result<MethodEffect, MethodError> {
    engine.apply(MethodWrite {
        subchannel: 0,
        method,
        value,
    })
}
fn tight_float3() -> Maxwell3d {
    let mut channel = Channel::default();
    // Literal B197 stream zero: stride20, inclusive limit of three vertices,
    // FLOAT3 position at0 and FLOAT2 UV at12. FIFO method addresses are words.
    for word in [
        0x20010000, 0xb197, 0x20040700, 0x1014, 0, 0x70000, 1, 0x200207c0, 0, 0x7003b, 0x20010620,
        0, 0x20020458, 0x38400000, 0x38800600,
    ] {
        channel.push_word(word).unwrap();
    }
    channel.finish().unwrap();
    channel.engine
}
fn layout(stride: u32, position: u32, uv: u32, float4: bool) -> Maxwell3d {
    let mut engine = tight_float3();
    for (method, value) in [
        (0x1c00, 0x1000 | stride),
        (0x1f04, 0x70fff),
        (
            0x1160,
            (if float4 { 0x38200000 } else { 0x38400000 }) | (position << 7),
        ),
        (0x1164, 0x38800000 | (uv << 7)),
    ] {
        set(&mut engine, method, value).unwrap();
    }
    engine
}

#[test]
fn literal_float3_position_and_uv_use_exact_twenty_byte_vertex_storage() {
    let engine = tight_float3();
    let array = engine.vertex_array().unwrap();
    assert_eq!(
        array,
        VertexArray {
            address: GpuAddress(0x70000),
            limit: GpuAddress(0x7003b),
            stride: 20,
            offsets: [0, 12],
            position_format: PositionFormat::Float32x3,
        }
    );
    assert_eq!(PositionFormat::Float32x3.byte_size(), 12);
    assert_eq!(PositionFormat::Float32x4.byte_size(), 16);
    assert_eq!(array.size(0, 3).unwrap(), 60);
    // Indexed resource scanning may request a non-triangle vertex span.
    assert_eq!(array.span(1, 2).unwrap(), 60);
    assert!(array.size(1, 2).is_err());
    assert!(array.span(1, 3).is_err());
}

#[test]
fn padded_reordered_and_last_component_offsets_follow_position_width() {
    for (stride, position, uv, float4, expected) in [
        (20, 0, 12, false, PositionFormat::Float32x3),
        (24, 0, 16, true, PositionFormat::Float32x4),
        (20, 8, 0, false, PositionFormat::Float32x3),
        (24, 8, 0, true, PositionFormat::Float32x4),
        (32, 16, 4, false, PositionFormat::Float32x3),
        (32, 16, 4, true, PositionFormat::Float32x4),
        (4092, 4080, 0, false, PositionFormat::Float32x3),
    ] {
        let engine = layout(stride, position, uv, float4);
        let array = engine.vertex_array().unwrap();
        assert_eq!(array.position_format, expected);
        assert_eq!(array.stride, stride as u16);
        assert_eq!(array.offsets, [position as u16, uv as u16]);
        assert_eq!(array.span(0, 1).unwrap(), stride as usize);
    }
    // The same offset fits12 position bytes exactly, but not16. The descriptor
    // itself is legal; complete-layout resolution must catch the overrun.
    let mut engine = layout(20, 8, 0, false);
    assert!(engine.vertex_array().is_ok());
    set(&mut engine, 0x1160, 0x38200400).unwrap();
    assert!(engine.vertex_array().is_err());
    set(&mut engine, 0x1c00, 0x1018).unwrap();
    assert_eq!(
        engine.vertex_array().unwrap().position_format,
        PositionFormat::Float32x4
    );
}

#[test]
fn valid_width_transitions_preserve_other_stream_fields() {
    let mut engine = layout(24, 0, 16, false);
    let original = engine.vertex_array().unwrap();
    for (word, format) in [
        (0x38200000, PositionFormat::Float32x4),
        (0x38400000, PositionFormat::Float32x3),
        (0x38200000, PositionFormat::Float32x4),
    ] {
        set(&mut engine, 0x1160, word).unwrap();
        assert_eq!(
            engine.vertex_array().unwrap(),
            VertexArray {
                position_format: format,
                ..original
            }
        );
    }
}

#[test]
fn encodable_but_invalid_stride_offsets_and_limits_fail_resolution() {
    for stride in [0, 1, 2, 3, 16, 18] {
        let mut engine = tight_float3();
        set(&mut engine, 0x1c00, 0x1000 | stride).unwrap();
        assert!(engine.vertex_array().is_err(), "stride{stride}");
    }
    for (method, value) in [
        (0x1c00, 20),         // Disabled stream bit.
        (0x1160, 0x38400080), // Byte offset1.
        (0x1164, 0x38800680), // Byte offset13.
        (0x1160, 0x385fff80), // Largest14-bit offset, beyond stride.
        (0x1c08, 0x70001),    // Misaligned stream address.
        (0x1f04, 0x6ffff),    // Inclusive limit precedes stream start.
    ] {
        let mut engine = tight_float3();
        set(&mut engine, method, value).unwrap();
        assert!(engine.vertex_array().is_err(), "{method:#x}={value:#x}");
    }
    let array = tight_float3().vertex_array().unwrap();
    for (first, count) in [(0, 0), (1, 3), (u32::MAX, 1), (u32::MAX - 2, 3)] {
        assert!(array.span(first, count).is_err());
    }
    let overflowing = VertexArray {
        address: GpuAddress(u64::MAX - 3),
        limit: GpuAddress(u64::MAX),
        ..array
    };
    assert!(overflowing.span(0, 1).is_err());
    // B197 upper address fields stay40-bit; a valid near-end descriptor still
    // cannot fit a vertex beyond its inclusive maximum address.
    let mut engine = tight_float3();
    for (method, value) in [
        (0x1c04, 255),
        (0x1c08, 0xfffffff0),
        (0x1f00, 255),
        (0x1f04, u32::MAX),
    ] {
        set(&mut engine, method, value).unwrap();
    }
    assert!(engine.vertex_array().unwrap().span(0, 1).is_err());
}

#[test]
fn unsupported_attribute_modifiers_and_direct_writes_preserve_valid_state() {
    let mut engine = layout(32, 16, 4, false);
    let before = engine.vertex_array().unwrap();
    for (method, value) in [
        (0x1160, 0x38400801), // Another stream.
        (0x1160, 0x38400840), // Inactive source.
        (0x1160, 0x20400800), // UINT rather than FLOAT.
        (0x1160, 0xb8400800), // R/B swap.
        (0x1160, 0x38800800), // Position FLOAT2, unsupported.
        (0x1160, 0x38600800), // Position half-float4, unsupported.
        (0x1160, 0x38400820), // Reserved bit5.
        (0x1164, 0x38400200), // UV FLOAT3, unsupported.
        (0x1164, 0x38800201),
        (0x1164, 0x38800240),
        (0x1164, 0xb8800200),
        (0x1c00, 0x3020), // Unknown stream-format bit.
        (0x1c04, 256),
        (0x1f00, 256),
        (0x1c0c, 2), // Frequency other than per-vertex1.
        (0x1880, 1), // Instancing unsupported.
    ] {
        assert_eq!(
            set(&mut engine, method, value),
            Err(MethodError::UnsupportedValue { method, value })
        );
        assert_eq!(engine.vertex_array().unwrap(), before);
    }
    for method in [0x1161, 0x1165, 0x1c01] {
        assert_eq!(
            set(&mut engine, method, 0),
            Err(MethodError::InvalidMethod(method))
        );
        assert_eq!(engine.vertex_array().unwrap(), before);
    }
}

#[test]
fn exact_vertex_span_reads_reject_missing_tail_and_keep_alias_remaps_checked() {
    let array = tight_float3().vertex_array().unwrap();
    let size = array.size(0, 3).unwrap();
    let words = [
        0xbf400000u32,
        0xbe800000,
        0x3f000000,
        0x3e800000,
        0x3f400000,
        0x3f000000,
        0xbe000000,
        0x3e800000,
        0x3f000000,
        0x3e800000,
        0xbe800000,
        0x3f400000,
        0x3f000000,
        0x3f400000,
        0x3f000000,
    ];
    let bytes: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    assert_eq!(bytes.len(), 60);
    let mut backing = vec![0; 160];
    backing[..60].copy_from_slice(&bytes);
    for (dst, src) in backing[100..160].iter_mut().zip(&bytes) {
        *dst = *src ^ 0x80;
    }
    let memory = SliceMemory {
        base: GuestAddress(100),
        bytes: &backing,
    };
    let mut map = AddressMap::default();
    map.map(array.address, GuestAddress(100), 60).unwrap();
    map.map(GpuAddress(0x80000), GuestAddress(100), 60).unwrap();
    let read = map.read(&memory, array.address, size).unwrap();
    assert_eq!(read, bytes);
    assert!(std::ptr::eq(read.as_ptr(), backing.as_ptr()));
    assert_eq!(&read[56..60], &0x3f000000u32.to_le_bytes());
    assert_eq!(
        map.resolve(GpuAddress(0x80000), size).unwrap(),
        GuestAddress(100)
    );
    let mut short_map = AddressMap::default();
    short_map.map(array.address, GuestAddress(100), 59).unwrap();
    assert!(short_map.read(&memory, array.address, size).is_err());
    let short_memory = SliceMemory {
        base: GuestAddress(100),
        bytes: &backing[..59],
    };
    assert!(map.read(&short_memory, array.address, size).is_err());
    let mut split = AddressMap::default();
    split.map(array.address, GuestAddress(100), 32).unwrap();
    split
        .map(GpuAddress(0x70020), GuestAddress(132), 28)
        .unwrap();
    assert!(split.read(&memory, array.address, size).is_err());
    map.unmap_exact(array.address, 60).unwrap();
    assert!(map.read(&memory, array.address, size).is_err());
    map.map(array.address, GuestAddress(200), 60).unwrap();
    assert_eq!(
        map.read(&memory, array.address, size).unwrap(),
        &backing[100..160]
    );
    assert_eq!(map.read(&memory, GpuAddress(0x80000), size).unwrap(), bytes);
}

#[test]
fn assembled_vertex_program_really_reads_position_w_and_exports_it_as_a_varying() {
    let binary = include_bytes!("fixtures/maxwell/vertex-float3-w.bin");
    let program = VertexProgram::decode(binary).unwrap();
    assert_eq!(program.required_registers(), 6);
    assert!(program.operations().contains(&Operation::AttributeRead {
        dst: 0,
        attribute: 0x80,
        words: 4
    }));
    assert!(
        program
            .operations()
            .contains(&Operation::Move { dst: 4, src: 3 })
    );
    let module = naga::front::wgsl::parse_str(&program.wgsl()).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
    assert!(FragmentProgram::decode(binary).is_err());
    // CPU decoding establishes the W dependency; actual FLOAT3 fetch filling W
    // with1 must be demonstrated by the separate GPU execution test.
}
