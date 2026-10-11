// Horizon threading probe: an NSO built with devkitA64 and libnx.
//
// After libnx's unmodified startup, the main thread creates a worker with libnx's
// threadCreate (stack allocated from the heap and aliased into the stack region with
// svcMapMemory, svcCreateThread on virtual core 1) and starts it. Both threads add
// deterministic values to one shared total inside a libnx Mutex, with a deliberately slow
// read-modify-write so that preemption inside the critical section creates contention
// (svcArbitrateLock / svcArbitrateUnlock). The main thread then waits on a libnx CondVar
// until the worker reports progress (svcWaitProcessWideKeyAtomic / svcSignalProcessWideKey),
// times out waiting for the sleeping worker (svcWaitSynchronization with a timeout), and
// finally blocks in threadWaitForExit until the worker exits, closes it (svcUnmapMemory,
// svcCloseHandle) and reports. Every expected value is derivable without running it.
#include <switch.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

void __appInit(void) {}
void __appExit(void) {}

#define ITERATIONS 3000

typedef struct {
    Mutex lock;
    CondVar progress;
    u64 total;           // protected by lock
    u32 entries[2];      // critical sections entered per thread, protected by lock
    int ready;           // protected by lock
    u64 worker_tid;
    u64 worker_tls;
    u32 worker_tls_value;
} Shared;

static __thread u32 tls_value = 7;

static u64 mix(u64 x) {
    x ^= x >> 31;
    x *= 0x7fb5d329728ea185ULL;
    x ^= x >> 27;
    x *= 0x81dadef4bc2dd44dULL;
    x ^= x >> 33;
    return x;
}

static void say(const char* text) {
    svcOutputDebugString(text, strlen(text));
}

static void work(Shared* s, unsigned who, unsigned from, unsigned to) {
    for (unsigned i = from; i < to; i++) {
        u64 v = mix(((u64)who << 32) | i);
        mutexLock(&s->lock);
        // Read, compute slowly, write back: without mutual exclusion updates are lost.
        volatile u64 t = s->total;
        u64 delta = 0;
        for (unsigned k = 0; k < 24; k++) delta += (v >> k) & 0xff;
        s->total = t + delta;
        s->entries[who]++;
        mutexUnlock(&s->lock);
        tls_value++;
    }
}

static void worker(void* arg) {
    Shared* s = arg;
    svcGetThreadId(&s->worker_tid, CUR_THREAD_HANDLE);
    s->worker_tls = (u64)armGetTls();
    work(s, 1, 0, ITERATIONS / 2);
    mutexLock(&s->lock);
    s->ready = 1;
    condvarWakeAll(&s->progress);
    mutexUnlock(&s->lock);
    svcSleepThread(2000000);  // 2 ms: the main thread's 100 us wait on us times out
    work(s, 1, ITERATIONS / 2, ITERATIONS);
    s->worker_tls_value = tls_value;
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    char line[256];
    Shared* s = calloc(1, sizeof *s);
    mutexInit(&s->lock);
    condvarInit(&s->progress);

    Thread t;
    Result create = threadCreate(&t, worker, s, NULL, 0x10000, 0x2C, 1);
    Result start = R_SUCCEEDED(create) ? threadStart(&t) : create;
    u64 main_tid = 0;
    svcGetThreadId(&main_tid, CUR_THREAD_HANDLE);

    work(s, 0, 0, ITERATIONS / 2);
    mutexLock(&s->lock);
    while (!s->ready) condvarWait(&s->progress, &s->lock);
    mutexUnlock(&s->lock);

    s32 index = 5;
    Result timed = svcWaitSynchronization(&index, &t.handle, 1, 100000);
    work(s, 0, ITERATIONS / 2, ITERATIONS);
    Result joined = threadWaitForExit(&t);
    Result closed = threadClose(&t);

    snprintf(line, sizeof line, "threads create=%#x start=%#x main_tid=%#lx worker_tid=%#lx tls_main=%#lx tls_worker=%#lx",
             create, start, main_tid, s->worker_tid, (u64)armGetTls(), s->worker_tls);
    say(line);
    snprintf(line, sizeof line, "sync total=%016lx entries=%u,%u tls_values=%u,%u ready=%d",
             s->total, s->entries[0], s->entries[1], tls_value, s->worker_tls_value, s->ready);
    say(line);
    snprintf(line, sizeof line, "wait timed=%#x index=%d joined=%#x closed=%#x",
             timed, index, joined, closed);
    say(line);
    free(s);
    return 0;
}
