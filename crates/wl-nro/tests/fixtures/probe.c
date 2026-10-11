// Horizon process probe: an NSO built with devkitA64 and libnx.
//
// The program starts through libnx's unmodified startup path: crt0, self-relocation
// (__nx_dynamic, DT_RELR), envSetup (NSO entry ABI: X0 = 0, X1 = main thread handle),
// newlib setup, virtmemSetup (svcGetInfo region queries), heap creation (svcSetHeapSize)
// and main-thread/TLS setup (svcQueryMemory on its own stack). It then reports what it
// observes through svcOutputDebugString and exits through svcExitProcess.
#include <switch.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// Start no system services: libnx's default __appInit connects to sm, applet, hid, time
// and fs over IPC, which this probe does not exercise.
void __appInit(void) {}
void __appExit(void) {}

static __thread unsigned tls_value = 0x5a5a;

static unsigned fib(unsigned n) {
    return n < 2 ? n : fib(n - 1) + fib(n - 2);
}

static void say(const char* text) {
    svcOutputDebugString(text, strlen(text));
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    char line[256];
    u64 tid = 0, pid = 0, heap = 0, total = 0, used = 0, stack_region = 0;
    svcGetThreadId(&tid, CUR_THREAD_HANDLE);
    svcGetProcessId(&pid, CUR_PROCESS_HANDLE);
    svcGetInfo(&heap, InfoType_HeapRegionAddress, CUR_PROCESS_HANDLE, 0);
    svcGetInfo(&total, InfoType_TotalMemorySize, CUR_PROCESS_HANDLE, 0);
    svcGetInfo(&used, InfoType_UsedMemorySize, CUR_PROCESS_HANDLE, 0);
    svcGetInfo(&stack_region, InfoType_StackRegionAddress, CUR_PROCESS_HANDLE, 0);
    u64 rejected = 0x1234;
    Result bad = svcGetInfo(&rejected, InfoType_TotalMemorySize, CUR_PROCESS_HANDLE, 1);

    MemoryInfo text, stack;
    u32 page;
    svcQueryMemory(&text, &page, (u64)&main);
    svcQueryMemory(&stack, &page, (u64)line);

    const unsigned count = 4096;
    u32* values = malloc(count * sizeof *values);
    u32 sum = 0;
    for (unsigned i = 0; i < count; i++) values[i] = i * 2654435761u;
    for (unsigned i = 0; i < count; i++) sum ^= values[i] + (sum << 5);
    int heap_ok = values != NULL && (u64)values >= heap;
    free(values);

    tls_value += fib(15);
    double ratio = (double)stack.size / (double)total;

    snprintf(line, sizeof line, "probe tid=%#lx pid=%#lx handle=%#x tls=%#x getinfo_bad_subtype=%#x out=%#lx",
             tid, pid, threadGetCurHandle(), tls_value, bad, rejected);
    say(line);
    snprintf(line, sizeof line, "text state=%u perm=%u size=%#lx; stack state=%u perm=%u size=%#lx in_stack_region=%d",
             text.type, text.perm, text.size, stack.type, stack.perm, stack.size, stack.addr >= stack_region);
    say(line);
    snprintf(line, sizeof line, "heap_ok=%d sum=%08x ratio=%.6f used_below_total=%d",
             heap_ok, sum, ratio, used < total);
    say(line);
    return 0;
}
