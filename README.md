# RHDLauncher

Rust 桌面啟動器，提供伺服器設定、TCP relay、更新腳本整合與遊戲啟動管理。背景使用純色繪製，不依賴客戶端美術素材。

現行遊戲啟動路線為 **B2 進程內啟動**：自製 `rhd_inproc.dll`（`native/`，Rust 啟動器注入）在客戶端進程內部完成啟動契約補丁，啟動器本身不跨程序改寫客戶端程式碼。細節見 [docs/B2-INPROC-LAUNCH.md](docs/B2-INPROC-LAUNCH.md)。

## 建置

需求：Windows、Rust stable 與 Cargo；進程內 DLL 另需 CMake + MSVC C++（x64）。

cargo build --release                    # 啟動器：target/release/rhd-launcher.exe
powershell -File native\build.ps1 -BuildDir <建置產物目錄> -Configuration Release
                                         # rhd_inproc.dll + 20 項 ctest 驗收

首次執行會在程式旁建立設定檔。請依 servers.example.ini 設定伺服器位址、連接埠及遊戲目錄；帳號密碼由遊戲客戶端處理。不要把實際伺服器設定或憑證提交到 Git。

`dist/` 收錄本次發佈的編譯產物（`rhd-launcher.exe`、`rhd_inproc.dll`）與 `SHA256SUMS.txt`；從源碼自行建置時以 `cargo build --release` 與 `native/build.ps1` 產出為準。

## 遊戲客戶端 BIN 相容性

目前有證據核實的對應版本如下。倉庫不包含遊戲客戶端 BIN。

| 項目 | 已核實值 |
|---|---|
| 遊戲版本 | Lineage Remastered x64，2020-02-12 build |
| BIN | bin64/Lin.bin；bin64/LogonX64.bin 是相同內容 |
| 檔案大小 | 18,989,568 bytes |
| PE TimeDateStamp | 0x5E43D8D0 |
| SHA-256 | 3490FF3F11D2E92DF4A99F72991054F6A2757AF737664FA1E05ED3F41A172AF0 |
| 經 Login.exe 啟動時的執行期版本標記 | 2002122605 |
| 直接啟動 BIN 時的執行期版本標記 | 2002122601 |

2002122605 與 2002122601 是不同啟動路徑的執行期回報值，不代表兩個不同的 BIN 檔案版本。這份相容性資料只確認上述 x64 指紋；其他 BIN 世代及 x86 版本尚未核實。啟動器不會自動下載或替換客戶端檔案。

## 功能與限制

- Windows 桌面介面與伺服器清單。
- 本機 TCP relay 轉送客戶端連線；遊戲啟動成功後交棒常駐中繼 daemon，登入器約 3 秒退出。
- 可選更新腳本與更新後套用流程。
- hosts 受控區段管理。
- B2 進程內啟動（`inproc_launch=true` 預設）：補丁在客戶端進程內完成，含全部站點原值預檢與失敗回滾；任何客戶端狀態不符時零寫入拒絕並明確回報。
- 舊版 direct-launch（外部寫碼）五個模組以 `compile_error!` 封存於 src/，僅作禁令說明，不可重新編入。
- 真實遊戲啟動／登入／進世界屬**未驗證**狀態，需由持有客戶端與伺服器的人在隔離環境自行驗收。

## 專案說明

AI 開發指南見 AI_GUIDE.md；B1（歷史）與 B2（現行）路線說明見 docs/。執行檔（dist/ 除外）、客戶端資產、個人設定、日誌及分析工具不包含在此原始碼倉庫。