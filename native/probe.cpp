#include <windows.h>
#include <wincrypt.h>
#include <cstdio>
#include <cwchar>
#include <filesystem>
#include <string>
#include <vector>
#include "detours.h"
#include "fixture_identity.h"

struct Handle {
    HANDLE value = nullptr;
    explicit Handle(HANDLE h = nullptr) : value(h) {}
    ~Handle() { if (value && value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
};

static bool fixture_matches(HANDLE file, const wchar_t* expected) {
    HCRYPTPROV provider = 0;
    HCRYPTHASH hash = 0;
    bool ok = false;
    BYTE digest[32] = {};
    DWORD digest_len = sizeof(digest);
    if (CryptAcquireContextW(&provider, nullptr, nullptr, PROV_RSA_AES, CRYPT_VERIFYCONTEXT) &&
        CryptCreateHash(provider, CALG_SHA_256, 0, 0, &hash)) {
        BYTE buffer[16384];
        DWORD count = 0;
        bool read_ok = true;
        while (true) {
            if (!ReadFile(file, buffer, sizeof(buffer), &count, nullptr)) { read_ok = false; break; }
            if (count == 0) break;
            if (!CryptHashData(hash, buffer, count, 0)) { read_ok = false; break; }
        }
        if (read_ok && CryptGetHashParam(hash, HP_HASHVAL, digest, &digest_len, 0) && digest_len == 32) {
            wchar_t actual[65] = {};
            for (size_t i = 0; i < 32; ++i) swprintf_s(actual + i * 2, 3, L"%02x", digest[i]);
            ok = _wcsicmp(actual, expected) == 0;
        }
    }
    if (hash) CryptDestroyHash(hash);
    if (provider) CryptReleaseContext(provider, 0);
    return ok;
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return 2;
    const std::wstring scenario = argv[1];
    if (scenario != L"success" && scenario != L"missing" &&
        scenario != L"timeout" && scenario != L"wrong-version") return 2;
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX);

    wchar_t executable[32768] = {};
    const DWORD len = GetModuleFileNameW(nullptr, executable, 32768);
    if (len == 0 || len >= 32768) return 3;
    const auto directory = std::filesystem::path(executable).parent_path();
    const auto fixture = directory / L"rhd_fixture.exe";
    Handle fixture_file(CreateFileW(fixture.c_str(), GENERIC_READ, FILE_SHARE_READ,
                                   nullptr, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr));
    if (fixture_file.value == INVALID_HANDLE_VALUE) return 3;
    const wchar_t* expected = scenario == L"wrong-version"
        ? L"0000000000000000000000000000000000000000000000000000000000000000"
        : RHD_FIXTURE_SHA256;
    const bool matching = fixture_matches(fixture_file.value, expected);
    if (scenario == L"wrong-version") return matching ? 4 : 0;
    if (!matching) return 4;

    const std::wstring event_name = L"Local\\RHD-Bootstrap-Probe-" +
        std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64());
    Handle event(CreateEventW(nullptr, TRUE, FALSE, event_name.c_str()));
    if (!event.value || GetLastError() == ERROR_ALREADY_EXISTS) return 5;
    if (!SetEnvironmentVariableW(L"RHD_BOOTSTRAP_PROBE_EVENT",
                                 scenario == L"timeout" ? nullptr : event_name.c_str())) return 6;

    std::wstring command = L"\"" + fixture.wstring() + L"\"";
    if (scenario == L"timeout") command += L" --wait";
    std::vector<wchar_t> command_line(command.begin(), command.end());
    command_line.push_back(L'\0');
    STARTUPINFOW startup = {};
    startup.cb = sizeof(startup);
    PROCESS_INFORMATION info = {};
    const char* dll = scenario == L"missing" ? "rhd_missing_bootstrap.dll" : "rhd_bootstrap.dll";
    const BOOL created = DetourCreateProcessWithDllExW(
        fixture.c_str(), command_line.data(), nullptr, nullptr, FALSE, CREATE_SUSPENDED,
        nullptr, directory.c_str(), &startup, &info, dll, nullptr);
    SetEnvironmentVariableW(L"RHD_BOOTSTRAP_PROBE_EVENT", nullptr);
    if (!created) return scenario == L"missing" ? 0 : 7;
    Handle process(info.hProcess);
    Handle thread(info.hThread);
    if (ResumeThread(thread.value) != 1) return 8;

    HANDLE objects[] = {event.value, process.value};
    const DWORD wait = WaitForMultipleObjects(2, objects, FALSE, scenario == L"timeout" ? 50 : 5000);
    if (WaitForSingleObject(process.value, 5000) != WAIT_OBJECT_0) return 9;
    DWORD exit_code = 0;
    if (!GetExitCodeProcess(process.value, &exit_code)) return 10;
    if (scenario == L"success") return wait == WAIT_OBJECT_0 && exit_code == 0 ? 0 : 11;
    if (scenario == L"missing") return wait == WAIT_OBJECT_0 + 1 && exit_code != 0 ? 0 : 12;
    return wait == WAIT_TIMEOUT && exit_code == 0 ? 0 : 13;
}
