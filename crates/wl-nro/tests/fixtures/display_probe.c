// Horizon display-service probe: an authentic Switch homebrew NRO built with devkitA64 and libnx.
//
// Tests VI display service initialization (viInitialize as Application, querying vi:u),
// opening the default display ("Default"), querying display resolution, closing the display,
// and exiting the VI client cleanly.
#include <switch.h>
#include <stdio.h>
#include <string.h>

// Start no default system services; the probe explicitly initializes and tests vi
void __appInit(void) {}
void __appExit(void) {}

static void say(const char* text) {
    svcOutputDebugString(text, strlen(text));
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    char line[256];

    // Stage 1: Connect to Service Manager ("sm:")
    Result rc_sm = smInitialize();
    snprintf(line, sizeof line, "display_probe sm_init rc=%#x", rc_sm);
    say(line);

    if (R_SUCCEEDED(rc_sm)) {
        // Stage 2: Initialize VI service client as Application (requests "vi:u" via SM)
        Result rc_vi = viInitialize(ViServiceType_Application);
        snprintf(line, sizeof line, "display_probe vi_init rc=%#x", rc_vi);
        say(line);

        if (R_SUCCEEDED(rc_vi)) {
            // Stage 3: Open default display ("Default")
            ViDisplay display = {0};
            Result rc_open = viOpenDefaultDisplay(&display);
            snprintf(line, sizeof line, "display_probe open_display rc=%#x id=%#llx", rc_open, (unsigned long long)display.display_id);
            say(line);

            if (R_SUCCEEDED(rc_open)) {
                // Stage 4: Request display resolution
                s32 width = 0, height = 0;
                Result rc_res = viGetDisplayResolution(&display, &width, &height);
                snprintf(line, sizeof line, "display_probe get_res rc=%#x width=%d height=%d", rc_res, (int)width, (int)height);
                say(line);

                // Stage 5: Close display
                Result rc_close = viCloseDisplay(&display);
                snprintf(line, sizeof line, "display_probe close_display rc=%#x", rc_close);
                say(line);
            }

            // Stage 6: Exit VI service client
            viExit();
            say("display_probe vi_exit ok");
        } else {
            say("display_probe vi_init failed");
        }

        // Clean teardown of Service Manager connection
        smExit();
        say("display_probe sm_exit ok");
    } else {
        say("display_probe sm_init failed");
    }

    return 0;
}
