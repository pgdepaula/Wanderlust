fn permute_bytes(a:u32,b:u32,selector:u32)->u32 {
    var result=0u;
    for (var lane=0u;lane<4u;lane=lane+1u) {
        let nibble=(selector>>(lane*4u))&15u;
        let index=nibble&7u;
        let source=select(a,b,index>=4u);
        let octet=(source>>((index&3u)*8u))&255u;
        let sign=select(0u,255u,(octet&128u)!=0u);
        let value=select(octet,sign,(nibble&8u)!=0u);
        result=result|(value<<(lane*8u));
    }
    return result;
}
