// Synthetic libnx client. No Nintendo code or assets: exercise actual ARM64
// service discovery, transfer memory, CMIF buffers and kernel event waits.
#include <switch.h>
#include <malloc.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void __appInit(void) {}
void __appExit(void) {}

static void fail(const char *stage, Result rc) {
    char text[96];
    int n = snprintf(text, sizeof text, "AUDIO_NATIVE_FAIL %s %x", stage, rc);
    svcOutputDebugString(text, (u64)n);
    svcBreak(0, 0, 0);
    svcExitProcess();
}
static void check(const char *stage, Result rc) { if (R_FAILED(rc)) fail(stage, rc); }

int main(void) {
    check("sm", smInitialize());
    check("device", auddevInitialize());
    AudioDeviceName name = {0};
    check("device name", auddevGetActiveAudioDeviceName(&name));
    check("device volume", auddevSetAudioDeviceOutputVolume(&name, 0.5f));
    float volume = -1;
    check("read volume", auddevGetAudioDeviceOutputVolume(&name, &volume));
    if (volume != 0.5f) fail("wrong volume", 3);
    AudioRendererConfig cfg = {
        .output_rate = AudioRendererOutputRate_48kHz,
        .num_voices = 1, .num_effects = 0, .num_sinks = 1,
        .num_mix_objs = 1, .num_mix_buffers = 2,
    };
    check("open", audrenInitialize(&cfg));
    u32 state = 0;
    check("initial state", audrenGetState(&state));
    if (state != 1) fail("not stopped", state);
    size_t n = audrenGetInputParamSize(&cfg);
    u8 *input = memalign(0x1000, (n+0xfff)&~0xfff);
    u8 *output = memalign(0x10, 0x1000);
    if (!input || !output) fail("allocation", 1);
    memset(input, 0, n);
    AudioRendererUpdateDataHeader *h = (void *)input;
    h->revision = audrenGetRevision();
    h->behavior_sz = 0x10; h->mempools_sz = 4*0x20;
    h->voices_sz = 0x170; h->channels_sz = 0x70;
    h->mixes_sz = 0x930; h->sinks_sz = 0x140; h->perfmgr_sz = 0x10;
    h->total_sz = (u32)n;
    memcpy(input+0x40, &h->revision, 4);
    check("silent update", audrenRequestUpdateAudioRenderer(input, n, output, 0x1000, NULL, 0));
    check("start", audrenStartAudioRenderer());
    u64 before = armGetSystemTick();
    audrenWaitFrame();
    audrenWaitFrame();
    u64 after = armGetSystemTick();
    // Two always-ready fake events cannot pass this timing assertion.
    if (after-before < 95000 || after-before > 19200000) fail("event clock", 2);
    check("running update", audrenRequestUpdateAudioRenderer(input, n, output, 0x1000, NULL, 0));
    const AudioRendererUpdateDataHeader *reply = (const void *)output;
    if (audrenGetRevision() >= AUDREN_REVISION_5) {
        u64 frames = 0;
        memcpy(&frames, output+0x40+4*0x10+0x10+0x20+0x10+0xb0, 8);
        if (frames < 2 || frames > 3) fail("frame count", (Result)frames);
    } else if (reply->total_sz != 368) {
        fail("legacy output size", reply->total_sz);
    }
    check("stop", audrenStopAudioRenderer());
    check("stopped state", audrenGetState(&state));
    if (state != 1) fail("still running", state);
    audrenExit(); auddevExit(); smExit(); free(input); free(output);
    svcOutputDebugString("AUDIO_NATIVE_OK", 15);
    svcExitProcess();
}
