// rhd_inproc_fixture —— 自製測試宿主：用自己的私有配置模擬客戶端映像。
//
// 它不連入啟動器，也不代表真實遊戲：只提供「版本指標稍後出現、程式頁 EXECUTE_READ」
// 的最小條件，讓 rhd_inproc.dll 的同一份交易碼在受控環境驗證
// 預檢、冪等、逾時、回滾與保護還原。偵測結果只適用測試程式。

#include <windows.h>
#include "inproc_contract.h"
#include <cstdio>
#include <cstring>
#include <string>

using namespace rhd_inproc;

static constexpr std::uint32_t kVerOld = 0x11111111u;
static constexpr std::uint32_t kVerNew = 0x22222222u;

struct Handles {
    HANDLE mapping = nullptr;
    void* view = nullptr;
    HANDLE done = nullptr;
    HANDLE fail = nullptr;
    HANDLE ready = nullptr;
    ~Handles() {
        if (view) UnmapViewOfFile(view);
        if (mapping) CloseHandle(mapping);
        if (done) CloseHandle(done);
        if (fail) CloseHandle(fail);
        if (ready) CloseHandle(ready);
    }
};

struct VersionThreadArgs {
    volatile std::uint32_t* slot;
    std::uint32_t value;
    unsigned delay_ms;
};

static DWORD WINAPI write_version(LPVOID param) {
    const auto* args = static_cast<const VersionThreadArgs*>(param);
    Sleep(args->delay_ms);
    *args->slot = args->value;
    return 0;
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 2) return 2;
    const std::wstring scenario = argv[1];
    if (scenario != L"success" && scenario != L"preimage-mismatch" &&
        scenario != L"version-timeout" && scenario != L"already-patched") return 2;

    SYSTEM_INFO info = {};
    GetSystemInfo(&info);
    const std::size_t page = info.dwPageSize;
    auto* memory = static_cast<unsigned char*>(VirtualAlloc(nullptr, page * 3,
        MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE));
    if (!memory) return 3;

    // 版本頁（RW）在 page0+0x100；兩個程式站點分落 page1/page2（之後保護為 EXECUTE_READ）
    static constexpr std::uint32_t kVerRva = 0x100;
    volatile std::uint32_t* slot =
        reinterpret_cast<volatile std::uint32_t*>(memory + kVerRva);
    const std::size_t site_a_off = page + 0x40;
    const std::size_t site_b_off = page * 2 + 0x80;
    static const std::uint8_t kOrigA[4] = {0x0F, 0x84, 0xC8, 0x03};
    static const std::uint8_t kReplA[4] = {0xE9, 0xC9, 0x03, 0x00};
    static const std::uint8_t kOrigB[6] = {0x48, 0x83, 0xEC, 0x28, 0xE8, 0x27};
    static const std::uint8_t kReplB[6] = {0xC3, 0x90, 0x90, 0x90, 0xC3, 0x90};
    std::memcpy(memory + site_a_off, kOrigA, sizeof(kOrigA));
    std::memcpy(memory + site_b_off, kOrigB, sizeof(kOrigB));

    // already-patched：先佈好「已補丁」狀態；preimage-mismatch：先破壞站點 B 的原值。
    // 兩者都必須在 VirtualProtect 之前完成——EXECUTE_READ 頁不能直接寫（那正是要驗的紀律）。
    if (scenario == L"already-patched") {
        *slot = kVerNew;
        std::memcpy(memory + site_a_off, kReplA, sizeof(kReplA));
        std::memcpy(memory + site_b_off, kReplB, sizeof(kReplB));
    } else if (scenario == L"preimage-mismatch") {
        memory[site_b_off] ^= 0xFF;
    }

    DWORD previous = 0;
    if (!VirtualProtect(memory + page, page * 2, PAGE_EXECUTE_READ, &previous) ||
        previous != PAGE_READWRITE) {
        VirtualFree(memory, 0, MEM_RELEASE);
        return 3;
    }

    Handles h;
    const std::wstring tag = std::to_wstring(GetCurrentProcessId()) + L"-" +
                             std::to_wstring(GetTickCount64());
    const std::wstring done_name = L"Local\\RHDInprocFixtureDone-" + tag;
    const std::wstring fail_name = L"Local\\RHDInprocFixtureFail-" + tag;
    const std::wstring map_name = L"Local\\RHDInprocFixtureCfg-" + tag;
    h.done = CreateEventW(nullptr, TRUE, FALSE, done_name.c_str());
    h.fail = CreateEventW(nullptr, TRUE, FALSE, fail_name.c_str());
    if (!h.done || !h.fail) return 4;

    // 就緒事件名稱來自 probe 的環境變數（繼承）；DLL 用的三個名稱設在自己行程裡
    wchar_t ready_name[160] = {};
    if (!GetEnvironmentVariableW(L"RHD_INPROC_READY", ready_name, 160)) return 4;
    h.ready = OpenEventW(EVENT_MODIFY_STATE, FALSE, ready_name);
    if (!h.ready) return 4;

    if (!SetEnvironmentVariableW(L"RHD_INPROC_EVENT_DONE", done_name.c_str())) return 4;
    if (!SetEnvironmentVariableW(L"RHD_INPROC_EVENT_FAIL", fail_name.c_str())) return 4;

    const std::uint32_t timeout_ms = scenario == L"version-timeout" ? 500u : 4000u;
    const std::uint32_t n = 2;
    h.mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0,
                                   sizeof(TestCfg) + sizeof(Site) * n, map_name.c_str());
    if (!h.mapping) return 4;
    h.view = MapViewOfFile(h.mapping, FILE_MAP_WRITE, 0, 0, 0);
    if (!h.view) return 4;
    auto* cfg = static_cast<TestCfg*>(h.view);
    cfg->magic = kCfgMagic;
    cfg->size = sizeof(TestCfg) + sizeof(Site) * n;
    cfg->n = n;
    cfg->timeout_ms = timeout_ms;
    cfg->base = reinterpret_cast<std::uint64_t>(memory);
    cfg->version_rva = kVerRva;
    cfg->version_old = kVerOld;
    cfg->version_new = kVerNew;
    auto* sites = reinterpret_cast<Site*>(cfg + 1);
    sites[0].rva = static_cast<std::uint32_t>(site_a_off);
    sites[0].len = sizeof(kOrigA);
    std::memcpy(sites[0].original, kOrigA, sizeof(kOrigA));
    std::memcpy(sites[0].replacement, kReplA, sizeof(kReplA));
    sites[1].rva = static_cast<std::uint32_t>(site_b_off);
    sites[1].len = sizeof(kOrigB);
    std::memcpy(sites[1].original, kOrigB, sizeof(kOrigB));
    std::memcpy(sites[1].replacement, kReplB, sizeof(kReplB));

    if (!SetEnvironmentVariableW(L"RHD_INPROC_TEST_MAPPING", map_name.c_str())) return 4;

    // 版本指標稍後出現（模擬脫殼完成）；逾時情境永不出現。
    // preimage-mismatch 也要出現版本：否則 DLL 在原值預檢之前就先逾時，驗不到預檢本身。
    VersionThreadArgs vargs{slot, kVerOld, 200};
    HANDLE version_thread = nullptr;
    if (scenario == L"success" || scenario == L"preimage-mismatch") {
        version_thread = CreateThread(nullptr, 0, write_version, &vargs, 0, nullptr);
        if (!version_thread) return 4;
    }

    if (!SetEvent(h.ready)) return 4;

    // 等 DLL 的結論（失敗事件也要等到——含逾時情境）
    HANDLE both[2] = {h.done, h.fail};
    const DWORD waited = WaitForMultipleObjects(2, both, FALSE, timeout_ms + 8000);
    if (waited == WAIT_TIMEOUT) {
        std::fprintf(stderr, "fixture: DLL 未回報結論\n");
        return 5;
    }
    const bool done_signaled = waited == WAIT_OBJECT_0;

    // 結果驗證
    bool ok = true;
    const bool expect_done = scenario == L"success" || scenario == L"already-patched";
    ok = ok && done_signaled == expect_done;

    auto site_is = [&](std::size_t off, const std::uint8_t* want, std::size_t len) {
        return std::memcmp(memory + off, want, len) == 0;
    };
    if (scenario == L"success" || scenario == L"already-patched") {
        ok = ok && site_is(site_a_off, kReplA, sizeof(kReplA));
        ok = ok && site_is(site_b_off, kReplB, sizeof(kReplB));
    } else if (scenario == L"preimage-mismatch") {
        // 零寫入：A 維持原值、B 維持被我們破壞後的值；版本槽已被版本執行緒寫入
        ok = ok && site_is(site_a_off, kOrigA, sizeof(kOrigA));
        ok = ok && memory[site_b_off] == (kOrigB[0] ^ 0xFF);
        ok = ok && *slot == kVerOld;
    } else { // version-timeout
        ok = ok && site_is(site_a_off, kOrigA, sizeof(kOrigA));
        ok = ok && site_is(site_b_off, kOrigB, sizeof(kOrigB));
        ok = ok && *slot == 0;
    }

    // 頁保護必須還原（失敗/回滾路徑同樣不可留 RWX）
    MEMORY_BASIC_INFORMATION mbi = {};
    ok = ok && VirtualQuery(memory, &mbi, sizeof(mbi)) == sizeof(mbi) &&
         mbi.Protect == PAGE_READWRITE;
    ok = ok && VirtualQuery(memory + page, &mbi, sizeof(mbi)) == sizeof(mbi) &&
         mbi.Protect == PAGE_EXECUTE_READ;
    ok = ok && VirtualQuery(memory + page * 2, &mbi, sizeof(mbi)) == sizeof(mbi) &&
         mbi.Protect == PAGE_EXECUTE_READ;

    if (version_thread) {
        WaitForSingleObject(version_thread, 2000);
        CloseHandle(version_thread);
    }
    if (!ok) std::fprintf(stderr, "fixture failed: scenario=%ls done=%d\n",
                          scenario.c_str(), done_signaled ? 1 : 0);
    VirtualFree(memory, 0, MEM_RELEASE);
    return ok ? 0 : 1;
}
