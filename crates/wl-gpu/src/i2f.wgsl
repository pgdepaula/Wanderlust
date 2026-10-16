// Independently derived bounded integer-to-binary32 RN-even packing.
// All intermediates are u32; there is no numeric float conversion.
fn wl_i2f_rn_bits(raw: u32, signed_mode: bool) -> u32 {
    let negative = signed_mode && (raw & 0x80000000u) != 0u;
    var magnitude = raw;
    if negative {
        // Unsigned magnitude handles signed INT_MIN without signed overflow.
        magnitude = ~raw + 1u;
    }
    if magnitude == 0u {
        return 0u;
    }
    var exponent = firstLeadingBit(magnitude);
    var significand = 0u;
    if exponent <= 23u {
        significand = magnitude << (23u - exponent);
    } else {
        let shift = exponent - 23u; // 1 through 8 for 32-bit source values.
        significand = magnitude >> shift;
        let remainder = magnitude & ((1u << shift) - 1u);
        let halfway = 1u << (shift - 1u);
        if remainder > halfway || (remainder == halfway && (significand & 1u) != 0u) {
            significand += 1u;
        }
        if significand == 0x01000000u {
            significand >>= 1u;
            exponent += 1u;
        }
    }
    let sign = select(0u, 0x80000000u, negative);
    return sign | ((exponent + 127u) << 23u) | (significand & 0x007fffffu);
}
