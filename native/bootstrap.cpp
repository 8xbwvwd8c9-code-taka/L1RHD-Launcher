#include <windows.h>
#include "detours.h"

extern "C" void WINAPI RhdBootstrapOrdinal() {}

BOOL WINAPI DllMain(HINSTANCE, DWORD reason, LPVOID) {
    if (DetourIsHelperProcess()) return TRUE;
    if (reason != DLL_PROCESS_ATTACH) return TRUE;
    if (!DetourRestoreAfterWith()) return FALSE;

    wchar_t event_name[160] = {};
    const DWORD len = GetEnvironmentVariableW(L"RHD_BOOTSTRAP_PROBE_EVENT", event_name, 160);
    if (len > 0 && len < 160) {
        HANDLE event = OpenEventW(EVENT_MODIFY_STATE, FALSE, event_name);
        if (event) {
            SetEvent(event);
            CloseHandle(event);
        }
    }
    return TRUE;
}
