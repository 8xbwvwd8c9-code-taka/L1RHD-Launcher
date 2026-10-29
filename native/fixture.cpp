#include <windows.h>
#include <cwchar>

int wmain(int argc, wchar_t** argv) {
    if (argc == 2 && std::wcscmp(argv[1], L"--wait") == 0) Sleep(250);
    return GetModuleHandleW(L"rhd_bootstrap.dll") ? 0 : 3;
}
