// rhd_inproc_probe —— 注入管線測試：對自製 fixture 執行與啟動器完全相同的鏈路。
//
//   CreateProcess(CREATE_SUSPENDED) → VirtualAllocEx/WriteProcessMemory（僅 DLL 路徑字串）
//   → CreateRemoteThread(LoadLibraryW) → ResumeThread → 等宿主就緒
//   → CreateRemoteThread(RhdPatchStart) → 結束碼即 DLL 補丁結果碼。
//
// 這是生產啟動器（src/inproc.rs）的同構驗證：鏈路通了，才談真實客戶端。

#include <windows.h>
#include <psapi.h>
#include <cstdio>
#include <filesystem>
#include <string>
#include "inproc_contract.h"

using namespace rhd_inproc;

struct Handles {
    HANDLE process = nullptr;
    HANDLE thread = nullptr;
    HANDLE ready = nullptr;
    ~Handles() {
        // 失敗路徑必須終止掛起中的 fixture：否則它握住輸出管線，ctest 會等到逾時。
        if (process) {
            DWORD code = 0;
            if (GetExitCodeProcess(process, &code) && code == STILL_ACTIVE) {
                TerminateProcess(process, 2);
            }
            CloseHandle(process);
        }
        if (thread) CloseHandle(thread);
        if (ready) CloseHandle(ready);
    }
};

static std::uint32_t expected_rc(const std::wstring& scenario) {
    if (scenario == L"success") return kOk;
    if (scenario == L"already-patched") return kAlreadyPatched;
    if (scenario == L"preimage-mismatch") return kPreimageMismatch;
    return kTimeout; // version-timeout
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return 2;
    const std::wstring scenario = argv[1];
    if (scenario != L"success" && scenario != L"preimage-mismatch" &&
        scenario != L"version-timeout" && scenario != L"already-patched") return 2;

    wchar_t self[32768] = {};
    GetModuleFileNameW(nullptr, self, 32768);
    const auto directory = std::filesystem::path(self).parent_path();
    const auto fixture = directory / L"rhd_inproc_fixture.exe";
    const auto dll = directory / L"rhd_inproc.dll";
    if (!std::filesystem::exists(fixture)) return 3;
    if (!std::filesystem::exists(dll)) return 3;

    // 就緒事件：名稱經環境變數傳給 fixture（DLL 也在同一行程內讀得到自己的三個名稱）
    const std::wstring ready_name = L"Local\\RHDInprocProbe-" +
        std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64());
    Handles h;
    h.ready = CreateEventW(nullptr, TRUE, FALSE, ready_name.c_str());
    if (!h.ready) return 4;
    if (!SetEnvironmentVariableW(L"RHD_INPROC_READY", ready_name.c_str())) return 4;

    std::wstring command = L"\"" + fixture.wstring() + L"\" " + scenario;
    std::vector<wchar_t> command_line(command.begin(), command.end());
    command_line.push_back(L'\0');
    STARTUPINFOW startup = {};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION info = {};
    if (!CreateProcessW(fixture.c_str(), command_line.data(), nullptr, nullptr, FALSE,
                        CREATE_SUSPENDED, nullptr, directory.c_str(), &startup, &info)) {
        std::fprintf(stderr, "probe: CreateProcessW 失敗 err=%u\n", GetLastError());
        return 5;
    }
    h.process = info.hProcess;
    h.thread = info.hThread;

    // ── 注入：唯一一次跨程序寫入＝DLL 路徑字串 ──
    std::wstring dll_path = dll.wstring();
    std::vector<wchar_t> dll_wide(dll_path.begin(), dll_path.end());
    dll_wide.push_back(L'\0');
    void* remote_path = VirtualAllocEx(h.process, nullptr,
                                       dll_wide.size() * sizeof(wchar_t),
                                       MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!remote_path) return 6;
    SIZE_T written = 0;
    if (!WriteProcessMemory(h.process, remote_path, dll_wide.data(),
                            dll_wide.size() * sizeof(wchar_t), &written)) {
        std::fprintf(stderr, "probe: WriteProcessMemory(路徑) 失敗 err=%u\n", GetLastError());
        return 6;
    }
    HMODULE k32 = GetModuleHandleW(L"kernel32.dll");
    if (!k32) return 6;
    const FARPROC load_library = GetProcAddress(k32, "LoadLibraryW");
    if (!load_library) return 6;
    HANDLE loader_thread = CreateRemoteThread(h.process, nullptr, 0,
        reinterpret_cast<LPTHREAD_START_ROUTINE>(load_library), remote_path, 0, nullptr);
    if (!loader_thread) return 6;
    if (WaitForSingleObject(loader_thread, 15000) != WAIT_OBJECT_0) return 6;
    DWORD loader_exit = 0;
    GetExitCodeThread(loader_thread, &loader_exit);
    CloseHandle(loader_thread);
    if (loader_exit == 0) {
        std::fprintf(stderr, "probe: 遠端 LoadLibraryW 失敗\n");
        return 6;
    }

    if (ResumeThread(h.thread) == static_cast<DWORD>(-1)) return 7;

    // 等宿主就緒（事件、映射、環境變數備妥）
    if (WaitForSingleObject(h.ready, 10000) != WAIT_OBJECT_0) {
        DWORD fx = 0;
        GetExitCodeProcess(h.process, &fx);
        std::fprintf(stderr, "probe: 宿主未就緒（fixture_exit=%lu STILL=%d）\n", fx,
                     fx == STILL_ACTIVE);
        return 8;
    }

    // ── 遠端呼叫 RhdPatchStart ──
    HMODULE local_dll = LoadLibraryW(dll_wide.data());
    if (!local_dll) return 9;
    const FARPROC local_start = GetProcAddress(local_dll, "RhdPatchStart");
    if (!local_start) return 9;
    // 遠端模組基底：EnumProcessModulesEx（測試宿主無殼，模組表可列舉）
    HMODULE mods[256] = {};
    DWORD needed = 0;
    std::uint64_t remote_dll = 0;
    if (K32EnumProcessModulesEx(h.process, mods, sizeof(mods), &needed, LIST_MODULES_ALL) &&
        needed <= sizeof(mods)) {
        wchar_t name[260] = {};
        const DWORD count = needed / sizeof(HMODULE);
        for (DWORD i = 0; i < count; ++i) {
            if (!K32GetModuleBaseNameW(h.process, mods[i], name, 260)) continue;
            if (_wcsicmp(name, L"rhd_inproc.dll") == 0) {
                remote_dll = reinterpret_cast<std::uint64_t>(mods[i]);
                break;
            }
        }
    }
    if (!remote_dll) return 10;
    const auto rva = reinterpret_cast<std::uint64_t>(local_start) -
                     reinterpret_cast<std::uint64_t>(local_dll);
    HANDLE patch_thread = CreateRemoteThread(h.process, nullptr, 0,
        reinterpret_cast<LPTHREAD_START_ROUTINE>(remote_dll + rva), nullptr, 0, nullptr);
    if (!patch_thread) return 11;
    if (WaitForSingleObject(patch_thread, 30000) != WAIT_OBJECT_0) return 11;
    DWORD rc = 0;
    GetExitCodeThread(patch_thread, &rc);
    CloseHandle(patch_thread);

    // 宿主結果
    if (WaitForSingleObject(h.process, 30000) != WAIT_OBJECT_0) return 12;
    DWORD fixture_exit = 0;
    GetExitCodeProcess(h.process, &fixture_exit);

    const std::uint32_t want = expected_rc(scenario);
    const bool ok = rc == want && fixture_exit == 0;
    std::printf("probe: scenario=%ls dll_rc=0x%X (期望 0x%X) fixture_exit=%lu → %s\n",
                scenario.c_str(), rc, want, fixture_exit, ok ? "PASS" : "FAIL");
    FreeLibrary(local_dll);
    return ok ? 0 : 1;
}
