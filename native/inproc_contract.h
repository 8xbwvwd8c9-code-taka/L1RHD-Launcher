// rhd_inproc 進程內補丁契約：DLL 與測試 fixture 共用的唯一事實。
//
// 真實模式（編譯期表格）：RVA 表來自《20261025-天R登入器執行期採證與LinLogin六刀補丁破解》
// §10.6 + §17，並經 2026-10-29 對 dump_Lin.copy.img（補丁前）/ Lin_mem.copy.bin（補丁後）
// 雙映像逐位元組驗證。原值（preimage）是「全部目標先驗證、全部通過才准寫第一筆」的依據。
//
// 測試模式（fixture）：透過具名檔案映射傳入 TestCfg，覆蓋基址與表格，
// 讓同一份交易碼在自製 fixture 上驗證全部預檢與失敗處理。

#pragma once
#include <cstdint>

namespace rhd_inproc {

constexpr std::uint32_t kCfgMagic = 0x52484449; // "RHDI"

// 檔案映射佈局：[TestCfg header][Site × n]
struct Site {
    std::uint32_t rva;
    std::uint32_t len;
    std::uint8_t original[8];
    std::uint8_t replacement[8];
};
static_assert(sizeof(Site) == 24, "Site layout");

struct TestCfg {
    std::uint32_t magic;      // kCfgMagic
    std::uint32_t size;       // sizeof(TestCfg) + sizeof(Site) * n
    std::uint32_t n;          // 站點數（1..64）
    std::uint32_t timeout_ms; // 等版本指標的上限
    std::uint64_t base;       // 測試映像基址（覆蓋 GetModuleHandleW(NULL)）
    std::uint32_t version_rva;
    std::uint32_t version_old;
    std::uint32_t version_new;
};

// RhdPatchStart 回傳碼（執行緒結束碼與失敗事件並行回報）
enum Rc : std::uint32_t {
    kOk = 0,            // 全部站點寫入覆核成功
    kAlreadyPatched = 0x200, // 版本已是新版，且站點狀態一致（冪等）
    kNoModule = 1,      // 拿不到主模組基址
    kTimeout = 0x101,   // 等版本指標逾時
    kPreimageMismatch = 0x110, // 任一站點原值不符（寫入前拒絕，零寫入）
    kProtectFailed = 0x120,    // 頁面保護提升失敗（零寫入）
    kWriteFailed = 0x130,      // 寫入或覆核失敗（已回滾全部站點）
    kUnsafeStop = 0x140,       // 保護還原失敗：回報不安全狀態，不得繼續
};

} // namespace rhd_inproc
