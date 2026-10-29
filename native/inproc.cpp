// rhd_inproc.dll —— 進程內八刀補丁（TR-BUG-458 路線 B2：與 LinLogin 同位格）
//
// 與被禁用的舊 B1（跨程序 WriteProcessMemory 逐刀改寫）的差異：
//   1. 本 DLL 由 LoadLibraryW 注入客戶端後，補丁寫入全部發生在客戶端進程內
//      （CreateRemoteThread 呼叫 RhdPatchStart），與原版 LinLogin.dll 的寫入位格相同；
//      啟動器對客戶端唯一的一次跨程序寫入是「DLL 路徑字串」（注入機制本身）。
//   2. 寫入時機照抄原廠契約：輪詢版本欄出現 2002122601（SecureEngine 脫殼完成指標）
//      才動手 —— 即 LinLogin TestFunction 的自旋語意，不另設 grace 秒數。
//   3. 交易紀律沿 rhd_transaction_fixture 驗證過的規格：全部站點原值先驗證
//      （任一不符即零寫入拒絕）→ 提升頁保護 → 寫入 → 覆核 → FlushInstructionCache
//      → 還原保護 → 還原結果複查；任何失敗回滾或回報不安全狀態。
//   4. DllMain 不等待、不建執行緒；真正工作只在 CreateRemoteThread 之後。
//
// 狀態回報：具名事件（環境變數 RHD_INPROC_EVENT_DONE / RHD_INPROC_EVENT_FAIL）
// 與執行緒結束碼（inproc_contract.h 的 Rc）；日誌寫入客戶端工作目錄 rhd_inproc.log。

#include <windows.h>
#include "detours.h"
#include "inproc_contract.h"
#include <cstdarg>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <vector>

using namespace rhd_inproc;

// ───────────────────────────── 編譯期八刀表 ─────────────────────────────

// RVA / 長度 / 原值 / 補丁值。原值已於 2026-10-29 對 dump_Lin.copy.img 驗證；
// 補丁值對 Lin_mem.copy.bin（刀 1-6）與 §17 首手記錄（刀 7-8）驗證。
static const Site kSites[] = {
    // RVA        len  original              replacement           名稱
    {0x5CBB800u, 4, {0x69,0xF7,0x55,0x77}, {0x6D,0xF7,0x55,0x77}},        // 版本欄1
    {0x5CBB808u, 4, {0x69,0xF7,0x55,0x77}, {0x6D,0xF7,0x55,0x77}},        // 版本欄2
    {0xE658B1u,  2, {0x74,0x08},           {0x90,0x90}},                   // NOP條件跳轉
    {0x97FC8Cu,  6, {0x0F,0x84,0xC8,0x03,0x00,0x00}, {0xE9,0xC9,0x03,0x00,0x00,0x00}}, // JMP強制GG成功
    {0x97D020u,  6, {0x48,0x83,0xEC,0x28,0xE8,0x27}, {0xC3,0x90,0x90,0x90,0xC3,0x90}}, // RET閹割檢查
    {0x9E4CA8u,  4, {0xE2,0xD7,0x0F,0x93}, {0xF2,0xC7,0x0E,0x96}},        // 握手imm32
    {0xA7AC57u,  6, {0x0F,0x84,0xAC,0x00,0x00,0x00}, {0xE9,0xAD,0x00,0x00,0x00,0x90}}, // JMP跳過封包加密
    {0xA7A9C5u,  2, {0x75,0x05},           {0xEB,0x1E}},                   // JMP跳過RC4解密
};
static const std::uint32_t kVersionRva = 0x5CBB800u;
static const std::uint32_t kVersionOld = 0x7755F769u; // 2002122601
static const std::uint32_t kVersionNew = 0x6DF75577u; // 2002122605
static const char* const kSiteNames[] = {
    "版本欄1", "版本欄2", "NOP條件跳轉", "JMP強制GG成功",
    "RET閹割檢查", "握手imm32", "JMP跳過封包加密", "JMP跳過RC4解密",
};

// ───────────────────────────── 日誌與事件 ─────────────────────────────

static void logf(const char* fmt, ...) {
    HANDLE file = CreateFileW(L"rhd_inproc.log", FILE_APPEND_DATA,
                              FILE_SHARE_READ | FILE_SHARE_WRITE, nullptr,
                              OPEN_ALWAYS, FILE_ATTRIBUTE_NORMAL, nullptr);
    if (file == INVALID_HANDLE_VALUE) return;
    SYSTEMTIME st = {};
    GetLocalTime(&st);
    char line[512];
    int n = _snprintf_s(line, sizeof(line), _TRUNCATE,
                        "[%04u-%02u-%02u %02u:%02u:%02u.%03u] ",
                        st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
                        st.wMilliseconds);
    va_list args;
    va_start(args, fmt);
    _vsnprintf_s(line + n, sizeof(line) - static_cast<size_t>(n), _TRUNCATE, fmt, args);
    va_end(args);
    n = static_cast<int>(strlen(line));
    line[n++] = '\r'; line[n++] = '\n';
    DWORD written = 0;
    WriteFile(file, line, static_cast<DWORD>(n), &written, nullptr);
    CloseHandle(file);
}

static void signal_env_event(const wchar_t* env_name) {
    wchar_t name[160] = {};
    const DWORD len = GetEnvironmentVariableW(env_name, name, 160);
    if (len == 0 || len >= 160) return;
    HANDLE ev = OpenEventW(EVENT_MODIFY_STATE, FALSE, name);
    if (!ev) {
        logf("開啟事件失敗 err=%u", GetLastError());
        return;
    }
    SetEvent(ev);
    CloseHandle(ev);
}

// 結果回報：測試模式優先用環境變數名稱；真實模式用「pid 衍生」名稱——
// 啟動器在 CreateProcess 取得 pid 後建立同名事件，雙方不需要環境塊。
static void signal_result(const wchar_t* env_name, const wchar_t* kind) {
    wchar_t name[160] = {};
    const DWORD len = GetEnvironmentVariableW(env_name, name, 160);
    if (len > 0 && len < 160) {
        HANDLE ev = OpenEventW(EVENT_MODIFY_STATE, FALSE, name);
        if (ev) {
            SetEvent(ev);
            CloseHandle(ev);
            return;
        }
    }
    swprintf(name, 160, L"Local\\RHDInproc%s-%u", kind, GetCurrentProcessId());
    HANDLE ev = OpenEventW(EVENT_MODIFY_STATE, FALSE, name);
    if (!ev) {
        logf("開啟結果事件失敗 %ls err=%u", kind, GetLastError());
        return;
    }
    SetEvent(ev);
    CloseHandle(ev);
}

// ───────────────────────────── 測試模式配置 ─────────────────────────────

struct Config {
    std::uint64_t base;
    std::uint32_t version_rva;
    std::uint32_t version_old;
    std::uint32_t version_new;
    std::uint32_t timeout_ms;
    const Site* sites;
    std::uint32_t n;
    bool test = false;
};

static bool load_test_config(Config& cfg) {
    wchar_t name[160] = {};
    const DWORD len = GetEnvironmentVariableW(L"RHD_INPROC_TEST_MAPPING", name, 160);
    if (len == 0 || len >= 160) return false;
    HANDLE mapping = OpenFileMappingW(FILE_MAP_READ, FALSE, name);
    if (!mapping) return false;
    const void* view = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0);
    bool ok = false;
    do {
        if (!view) break;
        const TestCfg* head = static_cast<const TestCfg*>(view);
        if (head->magic != kCfgMagic) break;
        if (head->size < sizeof(TestCfg) || head->n == 0 || head->n > 64) break;
        if (head->size != sizeof(TestCfg) + sizeof(Site) * head->n) break;
        // 映射內容在呼叫期間保持存活：fixture 在 DLL 工作期間不關閉映射。
        cfg.base = head->base;
        cfg.version_rva = head->version_rva;
        cfg.version_old = head->version_old;
        cfg.version_new = head->version_new;
        cfg.timeout_ms = head->timeout_ms;
        cfg.sites = reinterpret_cast<const Site*>(static_cast<const unsigned char*>(view) +
                                                  sizeof(TestCfg));
        cfg.n = head->n;
        cfg.test = true;
        ok = true;
    } while (false);
    if (!ok) UnmapViewOfFile(view);
    // 映射控制代碼刻意保留到行程結束：view 的生命週期必須覆蓋整個補丁過程。
    return ok;
}

// ───────────────────────────── 交易式補丁 ─────────────────────────────

static bool site_matches(const unsigned char* addr, const std::uint8_t* want, std::uint32_t len) {
    return std::memcmp(addr, want, len) == 0;
}

static bool writable_protection(DWORD protect) {
    switch (protect & 0xFF) {
        case PAGE_READWRITE:
        case PAGE_WRITECOPY:
        case PAGE_EXECUTE_READWRITE:
        case PAGE_EXECUTE_WRITECOPY:
            return true;
        default:
            return false;
    }
}

// 對每個站點落刀。回傳 kOk 或失敗碼；失敗時已回滾（或回報不安全）。
static std::uint32_t apply_sites(const Config& cfg) {
    unsigned char* base = reinterpret_cast<unsigned char*>(static_cast<std::uintptr_t>(cfg.base));

    // ── 1. 全部站點原值預檢：任一不符即拒絕，零寫入 ──
    for (std::uint32_t i = 0; i < cfg.n; ++i) {
        const Site& s = cfg.sites[i];
        if (s.len == 0 || s.len > 8) {
            logf("站點 %u 長度不合法 %u", i, s.len);
            return kPreimageMismatch;
        }
        unsigned char* addr = base + s.rva;
        if (!site_matches(addr, s.original, s.len)) {
            unsigned char seen[8] = {};
            std::memcpy(seen, addr, s.len);
            logf("原值預檢失敗：站點%u(%s) rva=0x%X 期望=%02X%02X 實得=%02X%02X",
                 i, cfg.test ? "測試" : kSiteNames[i], s.rva,
                 s.original[0], s.original[1], seen[0], seen[1]);
            return kPreimageMismatch;
        }
    }

    // ── 2. 頁保護提升（唯一頁面各一次），記錄原保護 ──
    struct Protected { unsigned char* page; DWORD old; };
    std::vector<Protected> protected_pages;
    auto page_of = [](unsigned char* p) {
        return reinterpret_cast<unsigned char*>(
            reinterpret_cast<std::uintptr_t>(p) & ~static_cast<std::uintptr_t>(0xFFF));
    };
    for (std::uint32_t i = 0; i < cfg.n; ++i) {
        unsigned char* page = page_of(base + cfg.sites[i].rva);
        bool known = false;
        for (const auto& p : protected_pages) known = known || p.page == page;
        if (known) continue;
        MEMORY_BASIC_INFORMATION mbi = {};
        if (VirtualQuery(page, &mbi, sizeof(mbi)) != sizeof(mbi)) {
            logf("VirtualQuery 失敗 err=%u", GetLastError());
            return kProtectFailed;
        }
        if (writable_protection(mbi.Protect)) continue; // 已可寫（如版本資料頁）
        DWORD old_prot = 0;
        if (!VirtualProtect(page, 0x1000, PAGE_EXECUTE_READWRITE, &old_prot)) {
            logf("VirtualProtect 失敗 page=0x%p err=%u", static_cast<void*>(page), GetLastError());
            for (const auto& p : protected_pages) {
                DWORD ignored = 0;
                VirtualProtect(p.page, 0x1000, p.old, &ignored);
            }
            return kProtectFailed;
        }
        protected_pages.push_back({page, old_prot});
    }

    // ── 3. 寫入 + 覆核 ──
    std::uint32_t done = 0;
    bool write_failed = false;
    for (std::uint32_t i = 0; i < cfg.n; ++i) {
        const Site& s = cfg.sites[i];
        unsigned char* addr = base + s.rva;
        std::memcpy(addr, s.replacement, s.len);
        if (!site_matches(addr, s.replacement, s.len)) {
            logf("寫入覆核失敗：站點%u rva=0x%X", i, s.rva);
            write_failed = true;
            break;
        }
        FlushInstructionCache(GetCurrentProcess(), addr, s.len);
        done = i + 1;
    }
    if (write_failed) {
        // 回滾全部已寫站點（含失敗那一筆——它可能寫入了一部分）
        bool rollback_ok = true;
        for (std::uint32_t j = 0; j <= done && j < cfg.n; ++j) {
            const Site& s = cfg.sites[j];
            unsigned char* addr = base + s.rva;
            std::memcpy(addr, s.original, s.len);
            rollback_ok = site_matches(addr, s.original, s.len) && rollback_ok;
            FlushInstructionCache(GetCurrentProcess(), addr, s.len);
        }
        // 還原頁保護
        bool restore_ok = true;
        for (const auto& p : protected_pages) {
            DWORD ignored = 0;
            restore_ok = VirtualProtect(p.page, 0x1000, p.old, &ignored) != 0 && restore_ok;
        }
        if (rollback_ok && restore_ok) {
            logf("已回滾 %u 筆站點與頁保護", done + 1);
            return kWriteFailed;
        }
        logf("回滾不完整 rollback=%d restore=%d —— 不安全狀態", rollback_ok ? 1 : 0,
             restore_ok ? 1 : 0);
        return kUnsafeStop;
    }

    // ── 4. 還原頁保護 + 複查 ──
    for (const auto& p : protected_pages) {
        DWORD ignored = 0;
        if (!VirtualProtect(p.page, 0x1000, p.old, &ignored)) {
            logf("保護還原失敗 page=0x%p err=%u —— 不安全狀態",
                 static_cast<void*>(p.page), GetLastError());
            return kUnsafeStop;
        }
        MEMORY_BASIC_INFORMATION mbi = {};
        if (VirtualQuery(p.page, &mbi, sizeof(mbi)) != sizeof(mbi) || mbi.Protect != p.old) {
            logf("保護還原複查失敗 page=0x%p —— 不安全狀態", static_cast<void*>(p.page));
            return kUnsafeStop;
        }
    }
    logf("八刀全部寫入覆核完成（%u 站點，%u 頁保護已還原）", cfg.n,
         static_cast<unsigned>(protected_pages.size()));
    return kOk;
}

// ───────────────────────────── 主流程 ─────────────────────────────

// 由啟動器以 CreateRemoteThread 呼叫（參數必須為 NULL）。
extern "C" DWORD WINAPI RhdPatchStart(LPVOID) {
    Config cfg;
    if (!load_test_config(cfg)) {
        HMODULE host = GetModuleHandleW(nullptr);
        if (!host) {
            logf("拿不到主模組基址");
            signal_result(L"RHD_INPROC_EVENT_FAIL", L"Fail");
            return kNoModule;
        }
        cfg.base = reinterpret_cast<std::uint64_t>(host);
        cfg.version_rva = kVersionRva;
        cfg.version_old = kVersionOld;
        cfg.version_new = kVersionNew;
        cfg.timeout_ms = 120000;
        cfg.sites = kSites;
        cfg.n = static_cast<std::uint32_t>(sizeof(kSites) / sizeof(kSites[0]));
        cfg.test = false;
    }

    logf("RhdPatchStart 進入（%s 模式，base=0x%llX，%u 站點）", cfg.test ? "測試" : "真實",
         static_cast<unsigned long long>(cfg.base), cfg.n);

    // ── 輪詢等解殼完成指標（原廠 TestFunction 語意：01 出現才動手） ──
    // 版本頁在 SecureEngine 脫殼期間可能尚未提交：進程內直接解參考未提交頁
    // 會以存取違規弄死客戶端，因此每次讀取前先以 VirtualQuery 確認 MEM_COMMIT。
    volatile std::uint32_t* version = reinterpret_cast<volatile std::uint32_t*>(
        static_cast<std::uintptr_t>(cfg.base) + cfg.version_rva);
    const ULONGLONG deadline = GetTickCount64() + cfg.timeout_ms;
    bool already = false;
    for (;;) {
        MEMORY_BASIC_INFORMATION mbi = {};
        bool committed = VirtualQuery(reinterpret_cast<LPCVOID>(
                                          const_cast<std::uint32_t*>(version)),
                                      &mbi, sizeof(mbi)) == sizeof(mbi) &&
                         mbi.State == MEM_COMMIT;
        if (committed) {
            const std::uint32_t v = *version;
            if (v == cfg.version_old) break;
            if (v == cfg.version_new) {
                already = true;
                break;
            }
        }
        if (GetTickCount64() > deadline) {
            logf("等待解殼指標(0x%08X)逾時 %ums（頁已提交=%d）", cfg.version_old,
                 cfg.timeout_ms, committed ? 1 : 0);
            signal_result(L"RHD_INPROC_EVENT_FAIL", L"Fail");
            return kTimeout;
        }
        Sleep(50);
    }

    if (already) {
        // 冪等：版本已是 05 → 全部站點必須已等於補丁值，否則視為不一致
        unsigned char* base = reinterpret_cast<unsigned char*>(
            static_cast<std::uintptr_t>(cfg.base));
        for (std::uint32_t i = 0; i < cfg.n; ++i) {
            const Site& s = cfg.sites[i];
            if (!site_matches(base + s.rva, s.replacement, s.len)) {
                logf("冪等檢查失敗：站點%u rva=0x%X 非補丁值", i, s.rva);
                signal_result(L"RHD_INPROC_EVENT_FAIL", L"Fail");
                return kPreimageMismatch;
            }
        }
        logf("版本已是新版且站點一致（冪等成功）");
        signal_result(L"RHD_INPROC_EVENT_DONE", L"Done");
        return kAlreadyPatched;
    }

    const std::uint32_t rc = apply_sites(cfg);
    if (rc == kOk) {
        signal_result(L"RHD_INPROC_EVENT_DONE", L"Done");
    } else {
        signal_result(L"RHD_INPROC_EVENT_FAIL", L"Fail");
    }
    return rc;
}

// ───────────────────────────── 進入點 ─────────────────────────────

BOOL WINAPI DllMain(HINSTANCE hinst, DWORD reason, LPVOID) {
    if (reason != DLL_PROCESS_ATTACH) return TRUE;
    // Detours with-DLL 載入（含 restore payload）時必須還原匯入表；
    // 純 LoadLibraryW 注入沒有 payload，DetourRestoreAfterWith 回 FALSE 不應擋載入。
    if (DetourIsHelperProcess()) {
        if (!DetourRestoreAfterWith()) return FALSE;
        return TRUE;
    }
    DisableThreadLibraryCalls(hinst);
    return TRUE;
}
