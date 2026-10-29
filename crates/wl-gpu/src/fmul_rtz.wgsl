// Exact finite binary32 product with FTZ and rounding toward zero.
// The 24x24 significand product uses a 48-bit pair of unsigned words.
// Single-precision NaN payload is unspecified; use a quiet canonical NaN.
fn mul_ftz_rtz(a:u32,b:u32)->u32 {
    let sign=(a^b)&0x80000000u;
    let ea=(a>>23u)&255u;let eb=(b>>23u)&255u;
    let fa=a&0x7fffffu;let fb=b&0x7fffffu;
    if (ea==255u && fa!=0u) || (eb==255u && fb!=0u) {return 0x7fc00000u;}
    if ea==255u || eb==255u {
        if ea==0u || eb==0u {return 0x7fc00000u;}
        return sign|0x7f800000u;
    }
    if ea==0u || eb==0u {return sign;}
    let ma=fa|0x800000u;let mb=fb|0x800000u;
    let low=(ma&65535u)*(mb&65535u);
    let cross=(ma>>16u)*(mb&65535u)+(ma&65535u)*(mb>>16u);
    let lo=low+(cross<<16u);
    let hi=(ma>>16u)*(mb>>16u)+(cross>>16u)+select(0u,1u,lo<low);
    let top=(hi&0x8000u)!=0u;
    let exponent=i32(ea)+i32(eb)-127+select(0,1,top);
    if exponent>=255 {return sign|0x7f7fffffu;}
    if exponent<=0 {return sign;}
    let mantissa=select((hi<<9u)|(lo>>23u),(hi<<8u)|(lo>>24u),top);
    return sign|(u32(exponent)<<23u)|(mantissa&0x7fffffu);
}
