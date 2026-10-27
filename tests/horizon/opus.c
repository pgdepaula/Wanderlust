// Synthetic libnx hardware-codec client; no game content.
#include <switch.h>
#include <stdio.h>
#include <string.h>
void __appInit(void) {}
void __appExit(void) {}
static void check(const char *name, Result result) {
    if (R_SUCCEEDED(result)) return;
    char text[96];int n=snprintf(text,sizeof text,"OPUS_NATIVE_FAIL %s %x",name,result);
    svcOutputDebugString(text,n);svcBreak(0,0,0);svcExitProcess();
}
int main(void) {
    check("sm",smInitialize());
    HwopusDecoder decoder={0};
    check("open",hwopusDecoderInitialize(&decoder,48000,2));
    // Public Opus DTX silence packet, with the service's big-endian frame header.
    const u8 packet[]={0,0,0,3,0,0,0,0,0xf8,0xff,0xfe};
    s16 pcm[960*2+8];memset(pcm,0x55,sizeof pcm);
    s32 consumed=0,samples=0;
    check("decode",hwopusDecodeInterleaved(&decoder,&consumed,&samples,packet,sizeof packet,pcm,sizeof pcm));
    if (consumed!=11 || samples!=960) check("counts",1);
    for (unsigned i=0;i<960*2;i++) if (pcm[i]!=0) check("silence",1);
    for (unsigned i=960*2;i<960*2+8;i++) if (pcm[i]!=0x5555) check("bounds",1);
    hwopusDecoderExit(&decoder);smExit();
    svcOutputDebugString("OPUS_NATIVE_OK",14);svcExitProcess();
}
