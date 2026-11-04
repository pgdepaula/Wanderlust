/*
Derived finite/RN arithmetic stages from Berkeley SoftFloat Release 3e,
pinned a0c6494cdc11865811dec815d5c0049fba9d82a8/source/s_mulAddF32.c
and source/s_roundPackToF32.c. WGSL integer port with a signed-zero FTZ wrapper; no floating contraction.


License for Berkeley SoftFloat Release 3e

John R. Hauser
2018 January 20

The following applies to the whole of SoftFloat Release 3e as well as to
each source file individually.

Copyright 2011, 2012, 2013, 2014, 2015, 2016, 2017, 2018 The Regents of the
University of California.  All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

 1. Redistributions of source code must retain the above copyright notice,
    this list of conditions, and the following disclaimer.

 2. Redistributions in binary form must reproduce the above copyright
    notice, this list of conditions, and the following disclaimer in the
    documentation and/or other materials provided with the distribution.

 3. Neither the name of the University nor the names of its contributors
    may be used to endorse or promote products derived from this software
    without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE REGENTS AND CONTRIBUTORS "AS IS", AND ANY
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE, ARE
DISCLAIMED.  IN NO EVENT SHALL THE REGENTS OR CONTRIBUTORS BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF
THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.


*/
// Private integer-only WGSL port. Not yet Naga/GPU validated.
struct WlFmaWide { lo:u32, hi:u32 }
struct WlFmaNormalized { exponent:i32, significand:u32 }
struct WlFmaResult { bits:u32, finite_inputs:bool }

fn wl_fma_add(a:WlFmaWide,b:WlFmaWide)->WlFmaWide {
    let lo=a.lo+b.lo;
    return WlFmaWide(lo,a.hi+b.hi+select(0u,1u,lo<a.lo));
}
fn wl_fma_sub(a:WlFmaWide,b:WlFmaWide)->WlFmaWide {
    return WlFmaWide(a.lo-b.lo,a.hi-b.hi-select(0u,1u,a.lo<b.lo));
}
fn wl_fma_shl(a:WlFmaWide,n:u32)->WlFmaWide {
    if n==0u { return a; }
    if n<32u { return WlFmaWide(a.lo<<n,(a.hi<<n)|(a.lo>>(32u-n))); }
    if n<64u { return WlFmaWide(0u,a.lo<<(n-32u)); }
    return WlFmaWide(0u,0u);
}
fn wl_fma_shr(a:WlFmaWide,n:u32)->WlFmaWide {
    if n==0u { return a; }
    if n<32u { return WlFmaWide((a.lo>>n)|(a.hi<<(32u-n)),a.hi>>n); }
    if n<64u { return WlFmaWide(a.hi>>(n-32u),0u); }
    return WlFmaWide(0u,0u);
}
// Exact integer nonzero; no boolean/select conversion in sticky extraction.
fn wl_fma_sticky(a:u32)->u32 { return (a|(0u-a))>>31u; }
fn wl_fma_jam(a:WlFmaWide,n:u32)->WlFmaWide {
    if n==0u { return a; }
    if n<32u {
        let low=(a.lo>>n)|(a.hi<<(32u-n));
        let sticky=wl_fma_sticky(a.lo&(0xffffffffu>>(32u-n)));
        return WlFmaWide(low|sticky,a.hi>>n);
    }
    if n==32u { return WlFmaWide(a.hi|wl_fma_sticky(a.lo),0u); }
    if n<64u {
        let sticky=wl_fma_sticky(a.lo|(a.hi&(0xffffffffu>>(64u-n))));
        return WlFmaWide((a.hi>>(n-32u))|sticky,0u);
    }
    return WlFmaWide(wl_fma_sticky(a.lo|a.hi),0u);
}
fn wl_fma_clz(a:WlFmaWide)->u32 {
    if a.hi!=0u { return countLeadingZeros(a.hi); }
    return 32u+countLeadingZeros(a.lo);
}
fn wl_fma_mul(a:u32,b:u32)->WlFmaWide {
    let a0=a&0xffffu;
    let a1=a>>16u;
    let b0=b&0xffffu;
    let b1=b>>16u;
    let p0=a0*b0;
    let p1=a0*b1;
    let p2=a1*b0;
    let p3=a1*b1;
    let s1=p0+(p1<<16u);
    let lo=s1+(p2<<16u);
    let hi=p3+(p1>>16u)+(p2>>16u)+select(0u,1u,s1<p0)+select(0u,1u,lo<s1);
    return WlFmaWide(lo,hi);
}
fn wl_fma_jam32(a:u32,n:u32)->u32 {
    if n==0u { return a; }
    if n<32u { return (a>>n)|wl_fma_sticky(a&(0xffffffffu>>(32u-n))); }
    return wl_fma_sticky(a);
}
fn wl_fma_normalize(significand:u32)->WlFmaNormalized {
    let shift=countLeadingZeros(significand)-8u;
    return WlFmaNormalized(1-i32(shift),significand<<shift);
}
fn wl_fma_round(sign:bool,exponent_input:i32,significand_input:u32)->u32 {
    var exponent=exponent_input;
    var significand=significand_input;
    let sign_bit=select(0u,0x80000000u,sign);
    if exponent<0 {
        significand=wl_fma_jam32(significand,u32(-exponent));
        exponent=0;
    } else if exponent>0xfd || (exponent==0xfd && significand>=0x7fffffc0u) {
        return sign_bit|0x7f800000u;
    }
    let remainder=significand&0x7fu;
    var rounded=(significand+0x40u)>>7u;
    if remainder==0x40u { rounded=rounded&~1u; }
    if rounded==0u { exponent=0; }
    return sign_bit+(u32(exponent)<<23u)+rounded;
}

// All raw finite inputs, including subnormals and signed zero. The caller must
// inspect finite_inputs; false is a rejected domain, never a successful zero.
fn wl_fma_finite(a:u32,b:u32,c:u32)->WlFmaResult {
    var ea=i32((a>>23u)&255u);
    var eb=i32((b>>23u)&255u);
    var ec=i32((c>>23u)&255u);
    if ea==255 || eb==255 || ec==255 { return WlFmaResult(0u,false); }
    var sa=a&0x7fffffu;
    var sb=b&0x7fffffu;
    var sc=c&0x7fffffu;
    let product_sign=((a^b)>>31u)!=0u;
    let c_sign=(c>>31u)!=0u;
    if (ea==0 && sa==0u) || (eb==0 && sb==0u) {
        if ec==0 && sc==0u && product_sign!=c_sign { return WlFmaResult(0u,true); }
        return WlFmaResult(c,true);
    }
    if ea==0 {
        let n=wl_fma_normalize(sa);
        ea=n.exponent;
        sa=n.significand;
    }
    if eb==0 {
        let n=wl_fma_normalize(sb);
        eb=n.exponent;
        sb=n.significand;
    }
    var ep=ea+eb-0x7e;
    sa=(sa|0x800000u)<<7u;
    sb=(sb|0x800000u)<<7u;
    var product=wl_fma_mul(sa,sb);
    if product.hi<0x20000000u {
        ep=ep-1;
        product=wl_fma_shl(product,1u);
    }
    if ec==0 {
        if sc==0u {
            return WlFmaResult(wl_fma_round(product_sign,ep-1,wl_fma_jam(product,31u).lo),true);
        }
        let n=wl_fma_normalize(sc);
        ec=n.exponent;
        sc=n.significand;
    }
    sc=(sc|0x800000u)<<6u;
    let difference=ep-ec;
    var result_sign=product_sign;
    var exponent:i32;
    var significand:u32;
    if product_sign==c_sign {
        var sum:u32;
        if difference<=0 {
            exponent=ec;
            sum=sc+wl_fma_jam(product,u32(32-difference)).lo;
        } else {
            exponent=ep;
            let wide_sum=wl_fma_add(product,wl_fma_jam(WlFmaWide(0u,sc),u32(difference)));
            sum=wl_fma_jam(wide_sum,32u).lo;
        }
        if sum<0x40000000u {
            exponent=exponent-1;
            sum=sum<<1u;
        }
        significand=sum;
    } else {
        let wide_c=WlFmaWide(0u,sc);
        var difference_significand:WlFmaWide;
        if difference<0 {
            result_sign=c_sign;
            exponent=ec;
            difference_significand=wl_fma_sub(wide_c,wl_fma_jam(product,u32(-difference)));
        } else if difference==0 {
            exponent=ep;
            difference_significand=wl_fma_sub(product,wide_c);
            if (difference_significand.lo|difference_significand.hi)==0u { return WlFmaResult(0u,true); }
            if (difference_significand.hi>>31u)!=0u {
                result_sign=!result_sign;
                difference_significand=wl_fma_sub(WlFmaWide(0u,0u),difference_significand);
            }
        } else {
            exponent=ep;
            difference_significand=wl_fma_sub(product,wl_fma_jam(wide_c,u32(difference)));
        }
        var shift=i32(wl_fma_clz(difference_significand))-1;
        exponent=exponent-shift;
        shift=shift-32;
        if shift<0 { significand=wl_fma_jam(difference_significand,u32(-shift)).lo; }
        else { significand=difference_significand.lo<<u32(shift); }
    }
    return WlFmaResult(wl_fma_round(result_sign,exponent,significand),true);
}

// FTZ/RN wrapper: NaN payload is unspecified; use canonical quiet NaN.
fn wl_ftz_bits(v:u32)->u32 {return select(v,v&0x80000000u,(v&0x7f800000u)==0u);}
fn wl_fma_ftz(a_in:u32,b_in:u32,c_in:u32)->u32 {
    let a=wl_ftz_bits(a_in);let b=wl_ftz_bits(b_in);let c=wl_ftz_bits(c_in);
    let ea=(a>>23u)&255u;let eb=(b>>23u)&255u;let ec=(c>>23u)&255u;
    if (ea==255u && (a&0x7fffffu)!=0u) || (eb==255u && (b&0x7fffffu)!=0u) || (ec==255u && (c&0x7fffffu)!=0u) {return 0x7fc00000u;}
    if ea==255u || eb==255u {
        if (a&0x7fffffffu)==0u || (b&0x7fffffffu)==0u {return 0x7fc00000u;}
        let sign=(a^b)&0x80000000u;
        if ec==255u && (c&0x80000000u)!=sign {return 0x7fc00000u;}
        return sign|0x7f800000u;
    }
    if ec==255u {return c;}
    return wl_ftz_bits(wl_fma_finite(a,b,c).bits);
}
