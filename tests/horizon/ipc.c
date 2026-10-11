// Horizon IPC probe: an NSO built with devkitA64 and libnx.
//
// Talks to the service manager through libnx's own client code: smInitialize
// (svcConnectToNamedPort("sm:"), CMIF RegisterClient with the kernel-supplied process ID),
// smGetServiceOriginal, smRegisterService and smUnregisterService (CMIF over HIPC in the
// thread's message buffer, handles returned as move handles). A second thread opens its own
// sm session with libnx's generic Service API and asks for a service that does not exist
// yet; sm defers the request until the main thread registers it. Finally it checks named-
// port errors, an unknown command (the server closes the session), and smExit's CMIF Close.
#include <switch.h>
#include <stdio.h>
#include <string.h>

void __appInit(void) {}
void __appExit(void) {}

static Result late_result = 1;
static Handle late_handle = 0;

static void say(const char* text) {
    svcOutputDebugString(text, strlen(text));
}

static void late_worker(void* arg) {
    (void)arg;
    Handle h = 0;
    Result rc = svcConnectToNamedPort(&h, "sm:");
    Service s;
    serviceCreate(&s, h);
    u64 pid = 0;
    if (R_SUCCEEDED(rc)) rc = serviceDispatchIn(&s, 0, pid, .in_send_pid = true);
    SmServiceName name = smEncodeName("wl:late");
    if (R_SUCCEEDED(rc))
        rc = serviceDispatchIn(&s, 1, name,
            .out_handle_attrs = { SfOutHandleAttr_HipcMove },
            .out_handles = &late_handle);
    late_result = rc;
    serviceClose(&s);
}

int main(int argc, char** argv) {
    (void)argc;
    (void)argv;
    char line[256];

    Result init = smInitialize();
    Handle stale = smGetServiceSession()->session;

    SmServiceName bad = {0};
    bad.name[0] = 'a';
    bad.name[2] = 'b';
    Handle bad_handle = 0x1234;
    Result bad_r = smGetServiceOriginal(&bad_handle, bad);

    Handle port = 0, port2 = 0, client = 0, client2 = 0;
    Result reg = smRegisterService(&port, smEncodeName("wl:echo"), false, 1);
    Result dup = smRegisterService(&port2, smEncodeName("wl:echo"), false, 1);
    Result get = smGetServiceOriginal(&client, smEncodeName("wl:echo"));
    Result full = smGetServiceOriginal(&client2, smEncodeName("wl:echo"));
    snprintf(line, sizeof line, "sm init=%#x bad=%#x bad_handle=%#x reg=%#x port=%d dup=%#x get=%#x client=%d full=%#x",
             init, bad_r, bad_handle, reg, port != 0, dup, get, client != 0, full);
    say(line);

    Result close_client = svcCloseHandle(client);
    Result unreg = smUnregisterService(smEncodeName("wl:echo"));
    Result unreg2 = smUnregisterService(smEncodeName("wl:echo"));
    Result close_port = svcCloseHandle(port);

    Thread t;
    threadCreate(&t, late_worker, NULL, NULL, 0x10000, 0x2C, 1);
    threadStart(&t);
    svcSleepThread(1000000);  // the worker is now blocked in GetServiceHandle("wl:late")
    Handle late_port = 0;
    Result late_reg = smRegisterService(&late_port, smEncodeName("wl:late"), false, 4);
    threadWaitForExit(&t);
    threadClose(&t);
    snprintf(line, sizeof line, "sm close_client=%#x unreg=%#x unreg2=%#x close_port=%#x late_reg=%#x late=%#x late_handle=%d",
             close_client, unreg, unreg2, close_port, late_reg, late_result, late_handle != 0);
    say(line);

    Handle raw = 0, none = 0;
    Result conn = svcConnectToNamedPort(&raw, "sm:");
    Result notfound = svcConnectToNamedPort(&none, "wl:none");
    Result toolong = svcConnectToNamedPort(&none, "abcdefghijkl");
    u32* tls = (u32*)armGetTls();
    memset(tls, 0, 0x100);
    tls[0] = 4;             // CMIF request
    tls[1] = 10;            // ten data words
    tls[4] = 0x49434653;    // "SFCI"
    tls[6] = 99;            // no such command
    Result unknown = svcSendSyncRequest(raw);
    Result again = svcSendSyncRequest(raw);
    svcCloseHandle(raw);
    smExit();
    Result stale_send = svcSendSyncRequest(stale);
    snprintf(line, sizeof line, "port conn=%#x notfound=%#x toolong=%#x unknown=%#x again=%#x stale=%#x",
             conn, notfound, toolong, unknown, again, stale_send);
    say(line);
    return 0;
}
