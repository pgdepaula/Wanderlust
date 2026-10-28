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

static void put32(u8 *p, u32 value) { memcpy(p, &value, 4); }
static void put64(u8 *p, u64 value) { memcpy(p, &value, 8); }

static void auxiliary_probe(void) {
    // Explicit REV11 wire request, independent of libnx's older default revision.
    typedef struct { u32 words[13]; } AuxParameters;
    const AuxParameters param = {{48000,240,1,0,1,1,1,0,0,0,0,0,0x3b564552u}};
    _Static_assert(sizeof param == 52, "AudioRendererParameter wire size");
    Service manager, renderer;
    check("aux manager", smGetService(&manager, "audren:u"));
    u64 work_size=0;
    check("aux work size", serviceDispatchInOut(&manager, 1, param, work_size));
    TransferMemory work;
    check("aux transfer memory", tmemCreate(&work, (size_t)work_size, Perm_None));
    const struct { AuxParameters param; u32 pad; u64 size, aruid; }
        request = {param, 0, work_size, 0};
    _Static_assert(sizeof request == 72, "OpenAudioRenderer wire size");
    check("aux open", serviceDispatchIn(&manager, 0, request,
        .in_send_pid=true, .in_num_handles=2,
        .in_handles={work.handle, CUR_PROCESS_HANDLE},
        .out_num_objects=1, .out_objects=&renderer));
    u8 *rings=memalign(0x1000,0x4000), *input=memalign(0x1000,0x2000);
    u8 *output=memalign(0x10,0x1000);
    if (!rings || !input || !output) fail("aux allocation",1);
    memset(rings,0,0x4000);memset(input,0,0x2000);
    memset(rings,0xa5,64);memset(rings+0x2000,0x5a,64);
    memset(rings+128,0x77,2048);
    const u32 sizes[10]={16,160,368,0,192,2352,320,16,0,0};
    u32 total=64;for (unsigned i=0;i<10;i++){put32(input+4+4*i,sizes[i]);total+=sizes[i];}
    put32(input,0x3b564552u);put32(input+60,total);put32(input+64,0x3b564552u);
    for (unsigned i=0;i<5;i++) put32(input+80+32*i+16,1);
    put64(input+80,(u64)(uintptr_t)rings);put64(input+88,0x4000);put32(input+96,4);
    u8 *effect=input+608,*specific=effect+32;
    effect[0]=2;effect[1]=1;effect[2]=1;
    put32(specific+48,1);put32(specific+52,48000);put32(specific+56,512);
    put32(specific+60,1);put32(specific+104,240);
    put64(specific+64,(u64)(uintptr_t)rings);put64(specific+72,(u64)(uintptr_t)(rings+128));
    put64(specific+80,(u64)(uintptr_t)(rings+0x2000));put64(specific+88,(u64)(uintptr_t)(rings+0x2080));
    u8 *mix=input+800;put32(mix,0x3f800000);put32(mix+4,48000);put32(mix+8,1);
    mix[12]=1;put32(mix+20,1);put32(mix+0x924,0x7fffffff);put32(mix+0x928,0xffffffff);
    u8 *sink=mix+2352;sink[0]=1;sink[1]=1;put32(sink+0x120,1);
    check("aux update", serviceDispatch(&renderer, 4,
        .buffer_attrs={SfBufferAttr_HipcMapAlias|SfBufferAttr_Out,SfBufferAttr_HipcMapAlias|SfBufferAttr_Out,SfBufferAttr_HipcMapAlias|SfBufferAttr_In},
        .buffers={{output,0x1000},{NULL,0},{input,total}}));
    if (output[160]!=3) fail("aux effect state",output[160]);
    Handle handle=INVALID_HANDLE;
    check("aux event",serviceDispatch(&renderer,7,
        .out_handle_attrs={SfOutHandleAttr_HipcCopy},.out_handles=&handle));
    Event event;eventLoadRemote(&event,handle,true);
    check("aux start",serviceDispatch(&renderer,5));
    check("aux wait",eventWait(&event,UINT64_MAX));
    u32 written=0,read=0;memcpy(&written,rings+68,4);memcpy(&read,rings+0x2040,4);
    if (written!=240 || read!=240) fail("aux cursors",written);
    for (unsigned i=0;i<64;i++) if(rings[i]!=0xa5 || rings[0x2000+i]!=0x5a) fail("aux CPU ownership",i);
    for (unsigned i=0;i<960;i++) if(rings[128+i]!=0) fail("aux samples",i);
    if(rings[128+960]!=0x77) fail("aux ring padding",1);
    check("aux stop",serviceDispatch(&renderer,6));
    eventClose(&event);serviceClose(&renderer);serviceClose(&manager);tmemClose(&work);
    free(rings);free(input);free(output);
    svcOutputDebugString("AUDIO_AUX_NATIVE_OK",19);
}

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
    audrenExit(); free(input); free(output);
    auxiliary_probe();
    auddevExit(); smExit();
    svcOutputDebugString("AUDIO_NATIVE_OK", 15);
    svcExitProcess();
}
