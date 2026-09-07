#include <windows.h>
#include <stdio.h>

static void log_loaded(void) {
    HANDLE handle = CreateFileA("C:\\tera-redirect.log", FILE_APPEND_DATA,
        FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
    if (handle == INVALID_HANDLE_VALUE) return;
    char exe[MAX_PATH];
    exe[0] = 0;
    GetModuleFileNameA(NULL, exe, MAX_PATH);
    char line[MAX_PATH + 64];
    int length = snprintf(line, sizeof line, "tera-hook chargee dans: %s (pid %lu)\r\n",
        exe, (unsigned long)GetCurrentProcessId());
    if (length > 0) {
        DWORD written;
        WriteFile(handle, line, (DWORD)length, &written, NULL);
    }
    CloseHandle(handle);
}

BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved) {
    (void)reserved;
    if (reason == DLL_PROCESS_ATTACH) {
        DisableThreadLibraryCalls(instance);
        log_loaded();
    }
    return TRUE;
}
