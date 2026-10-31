# RHD 自製登入器

**現況（2026-10-31，驗收 PASS）**：B4+ 路線（中繼 2000/29467→3000 ＋ 進程內八刀 `rhd_inproc.dll` ＋ 中繼補發 S_KEY ＋ opcode 199 明文登入代理）已實機驗收通過——2026-10-30 兩次進世界、2026-10-31 改進版（`20261031-heartbeat1`）登入驗收 PASS。改進版新增：BUILD_TAG 啟動標記、凍結風險掃描（AscFileFilter／D: 空間）、常駐中繼與補丁等待 10 秒心跳、manifest 吃檔引擎。

本專案不使用原版 `Login.exe` 或 `LinLogin.dll`。舊 B1 對執行中客戶端逐筆寫入程式碼的方式已禁用；不得重新編入、部署或透過調整等待時間恢復。現存舊版執行檔是歷史產物，不可當成此版本。

`servers.ini` 與 `launcher.json` 放在登入器旁。`game_dir` 留空時使用登入器所在目錄；相對路徑以該目錄為基準。舊設定中的絕對路徑會退回登入器所在目錄。

登入器在程序清單檢查失敗或偵測到既有遊戲時拒絕啟動，不強制關閉既有程序。Relay 日誌只記錄方向與位元組長度。

## 自動吃檔（選填）

`launcher.json` 加上 `"update_manifest_url": "http://<私服>/patch/manifest.local"` 即啟用：每次按「進入遊戲」時，先拉取遠端 manifest（與客戶端 `Login.local` 同格式：`[相對路徑]`＋`md5=<hex>`），逐檔 MD5 比對後下載差異、驗 MD5、暫存改名就地套用。資源 URL 取自 manifest URL 所在目錄（`data\x.tbl` → `<manifest 目錄>/data/x.tbl`）。

- 流程同構於官方 `NCTinyUpdater`（protobuf 對帳＋Download→Decompress 管線）的簡化版；不改寫客戶端 `Login.local` 本體。
- 安全設計：MD5 下載後複驗、暫存檔改名、路徑穿越防護（拒 `..`／磁碟號／絕對路徑）、單檔 512 MiB／整輪 4 GiB 上限、5000 條上限。
- `update_strict=true` 時吃檔失敗會中止啟動；預設 `false`（記錄後照常啟動）。
- 本建置為 **http-only**（ureq 未含 TLS feature；私服內網情境）。留空 `update_manifest_url`＝停用。

離線驗證：

```powershell
cargo check --offline
cargo test --offline
.\native\build.ps1 -BuildDir <建置產物目錄>
```

原生載入探針、固定版本的 Detours 原始碼與測試界線見 [native/BOOTSTRAP.md](native/BOOTSTRAP.md)。歷史啟動紀錄見 [docs/B1-DIRECT-LAUNCH.md](docs/B1-DIRECT-LAUNCH.md)，不可按其舊步驟操作。
