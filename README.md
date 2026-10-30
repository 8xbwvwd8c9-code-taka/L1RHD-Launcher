# RHD 自製登入器

目前遊戲啟動為 **HOLD**：自製 DLL 已在測試程式完成載入驗證，真實遊戲的初始化同步點尚未證明。

本專案不使用原版 `Login.exe` 或 `LinLogin.dll`。舊 B1 對執行中客戶端逐筆寫入程式碼的方式已禁用；不得重新編入、部署或透過調整等待時間恢復。現存舊版執行檔是歷史產物，不可當成此版本。

`servers.ini` 與 `launcher.json` 放在登入器旁。`game_dir` 留空時使用登入器所在目錄；相對路徑以該目錄為基準。舊設定中的絕對路徑會退回登入器所在目錄。

登入器在程序清單檢查失敗或偵測到既有遊戲時拒絕啟動，不強制關閉既有程序。Relay 日誌只記錄方向與位元組長度。

離線驗證：

```powershell
cargo check --offline
cargo test --offline
.\native\build.ps1 -BuildDir <建置產物目錄>
```

原生載入探針、固定版本的 Detours 原始碼與測試界線見 [native/BOOTSTRAP.md](native/BOOTSTRAP.md)。歷史啟動紀錄見 [docs/B1-DIRECT-LAUNCH.md](docs/B1-DIRECT-LAUNCH.md)，不可按其舊步驟操作。
