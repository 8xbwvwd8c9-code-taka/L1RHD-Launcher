# AI 開發指南

## 專案範圍

本專案是 Windows 上的 Rust/egui 啟動器。只修改本倉庫中的程式及文件；不要讀取、提交或產生使用者的客戶端、帳密、私人伺服器設定、執行日誌或本機逆向資料。

## 主要模組

- src/main.rs：啟動流程、管理員權限檢查、自我檢查入口、常駐中繼 daemon 模式。
- src/config.rs、src/servers.rs：設定與伺服器清單（`inproc_launch` 預設 true）。
- src/inproc.rs：B2 進程內啟動路線——注入自製 rhd_inproc.dll、建立 pid 衍生結果事件、CreateRemoteThread(RhdPatchStart)、交棒 daemon。
- src/relay.rs：本機 TCP relay。
- src/hosts.rs：管理 hosts 中由本程式標記的區段。
- src/launch.rs：遊戲程序檢查（`client_process_running` 等唯讀工具）。
- src/updater.rs：manifest 吃檔引擎（Login.local 同格式清單→MD5 比對→下載差異→驗證套用；http-only、防路徑穿越、單檔 512 MiB／整輪 4 GiB 上限）＋舊腳本整合（`run_script`，目前無呼叫點）。吃檔掛點在 `ui/mod.rs` 的 `boot_sequence`（`update_manifest_url` 留空＝停用；`update_strict` 控制失敗是否擋啟動）。
- src/ui/：egui 介面與元件；boot_sequence 是 B2 入口。
- src/inject.rs、src/remotehook.rs、src/sockhook.rs、src/patches.rs、src/loginhost.rs：**已封存**——全部以 `compile_error!` 阻止編譯，檔案僅存在以說明「外部寫碼 direct-launch 永久停用」的禁令，不得移除閘門或重新接回。
- native/：自製 C++ DLL 與測試。`inproc.cpp`（進程內八刀補丁＋交易紀律）是補丁表的**唯一事實來源**；`inproc_contract.h`（DLL/fixture 共用契約與回傳碼）、`inproc_fixture.cpp`＋`inproc_probe.cpp`（自製宿主與同構注入鏈測試）、`bootstrap.cpp`/`probe.cpp`（載入探針）、`transaction_fixture.cpp`（補丁交易的可執行規格，12 情境）、`build.ps1`（cmake+build+ctest 20 項）。

## 已核實的客戶端版本

- 目前唯一記錄完整指紋的目標是 Lineage Remastered x64 2020-02-12 build。
- bin64/Lin.bin 與 bin64/LogonX64.bin 的 SHA-256 為 3490FF3F11D2E92DF4A99F72991054F6A2757AF737664FA1E05ED3F41A172AF0；PE timestamp 為 0x5E43D8D0。
- 經 Login.exe 啟動時，執行期日誌回報 2002122605；直接啟動 BIN 時回報 2002122601。這兩個數值是啟動路徑標記，不可當成檔案指紋。
- 八刀補丁的原值表與補丁值必須與 native/inproc.cpp 的編譯期常數一致；README 版本表的指紋（SHA-256、timestamp）也須對齊。其他 BIN/x86 版本未驗證，不可宣稱支援。

## 開發規則

- 保持 Windows 支援與既有 Cargo 依賴；新增依賴前先確認標準函式庫或現有 crate 能否處理。
- 不要把 IP、帳密、絕對路徑、個人名稱或存取權杖寫入程式、範例或提交。
- 修改 hosts 時只處理本程式的明確標記區段；保留檔案其他內容。
- 更新腳本是外部程式執行邊界；保留錯誤回報與使用者可理解的狀態。
- 設定範例只能使用文件用位址與空白憑證。
- 不要把執行中驗收描述成已完成；未執行就明確標示未驗證。fixture 驗收不等於遊戲驗收。
- 補丁寫入必須維持交易紀律：全部站點原值預檢→頁保護提升→寫入覆核→FlushInstructionCache→還原複查；任何客戶端狀態不符都必須零寫入拒絕，不得放寬為「跳過不符站點」。
- 客戶端啟動期不得從啟動器跨程序 WriteProcessMemory／VirtualProtectEx 改寫客戶端程式碼（凍結事故禁令）；啟動器唯一的跨程序寫入是注入用 DLL 路徑字串。
- native 源檔含 UTF-8 中文註解：MSVC 必須 `/utf-8`（CMakeLists 已設定），新增目標同樣要加。

## 本機驗證

Windows 開發環境可使用 cargo test / cargo build --release 與 native\build.ps1（內含 ctest 20 項）。涉及 UAC、hosts、程序啟動、注入或網路連線的行為，須在隔離環境人工驗收。