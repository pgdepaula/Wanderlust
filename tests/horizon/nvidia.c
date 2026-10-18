// Original AArch64 homebrew: libnx HIPC -> NVIDIA ioctls -> Maxwell indexed draw.
// This is a synthetic hardware-conformance program, not game code.
#include <switch.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "nvidia_shaders.h"

void __appInit(void) {}
void __appExit(void) {}
NvServiceType __nx_nv_service_type = NvServiceType_System;
u32 __nx_nv_transfermem_size = 0x10000;

static void checked(Result rc, const char* operation) {
    if (R_FAILED(rc)) {
        char text[128];
        snprintf(text, sizeof text, "nvidia failure %s rc=%#x", operation, rc);
        svcOutputDebugString(text, strlen(text));
        svcExitProcess();
    }
}
static void call(u32 fd, u32 request, void* data) {
    checked(nvIoctl(fd, request, data), "ioctl");
}
static const struct { u64 address, offset, size; } ranges[] = {
    {0x10000,0x1000,0x2000}, {0x30000,0,0x1000}, {0x31000,0,0x1000},
    {0x40000,0,0x1000}, {0x50000,0x4000,0x1000}, {0x70000,0x3000,0x1000},
    {0x80000,0x6000,0x1000}, {0x90000,0x8000,0x2000}, {0xb0000,0x10000,0x4000},
};
// Literal NVIDIA B197 methods: source definitions and independently assembled
// shaders are the same conformance surface as tests/common/hardware.rs.
static const u32 setup[] = {
    0x20010000,0xb197, 0x200103c4,1,
    0x20030557,0,0x31020,0, 0x2003055d,0,0x30000,0,
    0x20020582,0,0x50000, 0x20020850,0x51,0x400, 0x20020853,8,4,
    0x200308e0,32,0,0x40040, 0x20010924,1,
    0x20020810,0x11,0x200, 0x20020813,8,0,
    0x20040700,0x1018,0,0x70000,1, 0x200207c0,0,0x70047,
    0x20010620,0, 0x20020458,0x38200000,0x38800800,
    0x20090200,0,0x90000,208,29,0xd5,0x1000,1,0,0,
    0x20060280,0x41bc0000,0xc1680000,0x3f800000,0x41bc0000,0x41680000,0,
    0x200203fd,47<<16,29<<16, 0x20020300,47<<16,29<<16,
    0x20030380,1,47<<16,29<<16,
    0x20010487,1,0x200104b3,0,0x200104ba,0,0x200104e0,0,
    0x200104d8,0,0x200104d7,0,0x200104b9,0,0x20010646,0,
    0x200104eb,0,0x20010649,0,0x2001064b,1,0x2001064f,0x1801,
    0x20010574,0,0x2001054e,0,0x20010680,0x1111,
    0x2001036b,0x1b02,0x2001036c,0x1b02,0x20010671,0,0x2001066f,0,
    0x2001036d,0,0x20010372,0,0x200105a3,0,0x20020302,0,0x3f800000,
    0x2001004e,1,0x200100df,1,0x20010286,0x6420,0x2002037e,0,0,
    0x200104bb,0,0x200103e9,0,0x2001035f,1,
    0x200105f2,0,0x200105f3,0x80000,0x200105f4,0,0x200105f5,0x80005,
    0x200105f6,1,0x2001050d,0,0x2001050e,0,0x20010591,0,
};
static const u32 draw[] = {0x20010586,4,0x200105f7,0,0x200105f8,3,0x20010585,0};
static int pixel(const u8* image, unsigned x, unsigned y, u32 rgba) {
    const u8* p = image + y*208 + x*4;
    return p[0] == (rgba&255) && p[1] == ((rgba>>8)&255)
        && p[2] == ((rgba>>16)&255) && p[3] == (rgba>>24);
}
int main(int argc, char** argv) {
    (void)argc; (void)argv;
    hosversionSet(MAKEHOSVERSION(18,0,0));
    checked(smInitialize(), "smInitialize");
    checked(nvInitialize(), "nvInitialize");
    u32 nvmap=0, space=0, channel=0;
    checked(nvOpen(&nvmap,"/dev/nvmap"),"open nvmap");
    checked(nvOpen(&space,"/dev/nvhost-as-gpu"),"open space");
    checked(nvOpen(&channel,"/dev/nvhost-gpu"),"open channel");
    u8* data = aligned_alloc(4096,0x18000);
    if (!data) svcExitProcess();
    memset(data,0,0x18000);
    const float vertices[] = {-.8f,-.6f,0,1,.1f,.2f, .6f,-.4f,0,1,.9f,.1f, -.2f,.7f,0,1,.25f,.85f};
    const u16 indices[] = {0,1,2};
    memcpy(data+0x3000,vertices,sizeof vertices);
    memcpy(data+0x6000,indices,sizeof indices);
    memcpy(data+0x4400,shader_red,sizeof shader_red);
    memcpy(data+0x4100,shader_quarters,sizeof shader_quarters);
    memcpy(data+0x4200,shader_vertex,sizeof shader_vertex);
    for (unsigned i=0;i<208*29;i+=4) {
        data[0x8000+i]=11; data[0x8001+i]=17; data[0x8002+i]=23; data[0x8003+i]=255;
    }
    struct {u32 size,handle;} create={0x18000,0};
    call(nvmap,0xc0080101,&create);
    struct {u32 handle,heap,flags,align; u64 kind,address;} alloc={create.handle,0,1,4096,0,(u64)data};
    _Static_assert(sizeof alloc==32,"nvmap ABI");
    call(nvmap,0xc0200104,&alloc);
    u8 init[40]={0}; call(space,0x40284109,init);
    call(channel,0x40044801,&nvmap); call(space,0x40044101,&channel);
    for (unsigned i=0;i<sizeof ranges/sizeof *ranges;i++) {
        struct {u32 pages,page_size,flags,pad; u64 address;} reserve={ranges[i].size/4096,4096,1,0,ranges[i].address};
        call(space,0xc0184102,&reserve);
        struct {u32 flags,kind,handle,pad; u64 offset,size,address;} map={1,0,create.handle,0,ranges[i].offset,ranges[i].size,ranges[i].address};
        _Static_assert(sizeof map==40,"GPU mapping ABI");
        call(space,0xc0284106,&map);
    }
    u32 fifo[2]={2048,0}; call(channel,0x40084805,fifo);
    struct {u32 cls,flags;u64 id;} object={0xb197,0,0}; call(channel,0xc0104809,&object);
    for (unsigned frame=0;frame<2;frame++) {
        u32* commands=(u32*)(data+0x10000); unsigned count=0;
        if (!frame) {memcpy(commands,setup,sizeof setup);count=sizeof setup/4;}
        else {commands[count++]=0x20010851;commands[count++]=0x100;}
        memcpy(commands+count,draw,sizeof draw);count+=sizeof draw/4;
        u64 entry=0xb0000ull|((u64)count<<42);
        struct {u64 pointer;u32 count,flags,id,threshold;} submit={0,1,6,0,0};
        _Static_assert(sizeof submit==24,"GPFIFO submission ABI");
        checked(nvIoctl2(channel,0xc018481b,&submit,&entry,8),"submit");
        int inside=pixel(data+0x8000,15,15,frame?0xffbf8040:0xff0000ff);
        int outside=pixel(data+0x8000,0,0,0xff17110b);
        char text[160];
        snprintf(text,sizeof text,"nvidia frame=%u inside=%d outside=%d fence=%u threshold=%u",frame+1,inside,outside,submit.id,submit.threshold);
        svcOutputDebugString(text,strlen(text));
        if (!inside || !outside || submit.threshold!=2*(frame+1)) svcExitProcess();
    }
    for (unsigned i=0;i<sizeof ranges/sizeof *ranges;i++) {
        u64 address=ranges[i].address; call(space,0xc0084105,&address);
        struct {u64 address;u32 pages,page_size;} release={address,ranges[i].size/4096,4096};
        call(space,0xc0104103,&release);
    }
    checked(nvClose(channel),"close channel"); checked(nvClose(space),"close space"); checked(nvClose(nvmap),"close nvmap");
    free(data); nvExit(); smExit();
    const char* done="nvidia completed two guest-issued indexed draws";
    svcOutputDebugString(done,strlen(done));
    return 0;
}
