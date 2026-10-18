// Horizon IPC probe: an authentic Switch homebrew NRO built with devkitA64 and libnx.
//
// Tests Service Manager (sm:) IPC initialization, deliberately absent service lookup ("wlmiss"),
// handle validation, clean service closure, smExit(), and deterministic debug output.
#include <switch.h>
#include <stdio.h>
#include <string.h>

// Start no default system services; the probe explicitly initializes and tests sm
void __appInit(void) {}
void __appExit(void) {}

static void say(const char* text) {
    svcOutputDebugString(text, strlen(text));
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    char line[256];

    // Stage 1: Connect to Service Manager ("sm:") via svcConnectToNamedPort and CMIF Initialize
    Result rc_sm = smInitialize();
    snprintf(line, sizeof line, "absent_service sm_init rc=%#x", rc_sm);
    say(line);

    if (R_SUCCEEDED(rc_sm)) {
        // Stage 2: Service lookup for standard "wlmiss" user service
        Handle srv_handle = 0;
        Result rc_lookup = smGetServiceOriginal(&srv_handle, smEncodeName("wlmiss"));
        snprintf(line, sizeof line, "absent_service lookup srv=wlmiss rc=%#x handle=%#x", rc_lookup, srv_handle);
        say(line);

        // Stage 3: Validate and close service handle if acquired
        if (R_SUCCEEDED(rc_lookup) && srv_handle != 0) {
            Result rc_close = svcCloseHandle(srv_handle);
            snprintf(line, sizeof line, "absent_service close srv=wlmiss rc=%#x", rc_close);
            say(line);
        }

        // Stage 4: Unregister/exit SM session cleanly
        smExit();
        say("absent_service sm_exit ok");
    } else {
        say("absent_service sm_init failed");
    }

    return 0;
}
