# AI 開發指南

## 專案範圍

本專案是 Windows 上的 Rust/egui 啟動器。只修改本倉庫中的程式及文件；不要讀取、提交或產生使用者的客戶端、帳密、私人伺服器設定、執行日誌或本機逆向資料。

## 主要模組

- src/main.rs：啟動流程、管理員權限檢查、自我檢查入口。
- src/config.rs、src/servers.rs：設定與伺服器清單。
- src/relay.rs：本機 TCP relay。
- src/hosts.rs：管理 hosts 中由本程式標記的區段。
- src/launch.rs、src/loginhost.rs：啟動及監控遊戲程序。
- src/updater.rs：更新腳本整合。
- src/ui/：egui 介面與元件。
- src/inject.rs、src/remotehook.rs、src/sockhook.rs、src/patches.rs：歷史相容與 direct-launch 程式碼；direct-launch 預設關閉，變更前須確認目標版本、啟用條件及程序副作用。

## 已核實的客戶端版本

- 目前唯一記錄完整指紋的目標是 Lineage Remastered x64 2020-02-12 build。
- bin64/Lin.bin 與 bin64/LogonX64.bin 的 SHA-256 為 3490FF3F11D2E92DF4A99F72991054F6A2757AF737664FA1E05ED3F41A172AF0；PE timestamp 為 0x5E43D8D0。
- 經 Login.exe 啟動時，執行期日誌回報 2002122605；直接啟動 BIN 時回報 2002122601。這兩個數值是啟動路徑標記，不可當成檔案指紋。
- README 的版本表與 src/patches.rs 的 LIN_TIMESTAMP、VERSION_RAW 必須保持一致。其他 BIN/x86 版本未驗證，不可宣稱支援。

## 開發規則

- 保持 Windows 支援與既有 Cargo 依賴；新增依賴前先確認標準函式庫或現有 crate 能否處理。
- 不要把 IP、帳密、絕對路徑、個人名稱或存取權杖寫入程式、範例或提交。
- 修改 hosts 時只處理本程式的明確標記區段；保留檔案其他內容。
- 更新腳本是外部程式執行邊界；保留錯誤回報與使用者可理解的狀態。
- 設定範例只能使用文件用位址與空白憑證。
- 不要把執行中驗收描述成已完成；未執行就明確標示未驗證。

## 本機驗證

Windows 開發環境可使用 cargo check 與 cargo build --release。涉及 UAC、hosts、程序啟動或網路連線的行為，須在隔離環境人工驗收。