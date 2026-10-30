# 天R 可運行連線方案（已驗證 2026-10-24 19:28）

> **2026-10-29：以下是歷史組合，不是現行啟動指引。** 原版 `Login.exe`、`LinLogin.dll` 與舊 B1 外部逐筆寫碼均不得用於本任務。新自製 DLL 僅完成測試程式載入驗證；真實遊戲啟動維持 HOLD。

> 此文件記錄**已實測可進遊戲**的完整組合。改動前請先確認此清單。

## 一、必要條件（缺一不可）

| # | 項目 | 值 | 驗證方式 |
| :-- | :--- | :--- | :--- |
| 1 | 伺服器 KeyPacket | **必須發送**（opcode 15） | 連 3000 應收到 `0D 00 0F ...` |
| 2 | Relay 監聽 | `2000`、`29467` → `127.0.0.1:3000` | `Get-NetTCPConnection -LocalPort 2000` |
| 3 | Relay timeout | **必須 `settimeout(None)`** | 否則 5 秒斷線 |
| 4 | `Login.exe` | **原版**（8,679,936 bytes） | 自製版會卡 GameGuard 380。由 RHDLauncher 代為啟動即可 |
| 5 | DB 角色名 | **簡體**（`测试1` = `E6B58BE8AF9531`） | 繁體會被踢 |
| 6 | DB 帳號 | `<account>` / 密碼 `<account>` | |
| 7 | `GameGuard` 目錄 | 完整 24 檔（不可改名） | |

## 二、啟動順序

### 方案 A：一鍵（推薦，已驗證）

```
雙擊 I:\RHD\RHDLauncher-rs\Start_Game.bat
```

腳本會自動：檢查伺服器 → 啟動 relay → 啟動 Login.exe。

### 方案 B：全用我們的 Rust 版本（已驗證 19:38）

```
1. 伺服器  I:\RHD\L1RHDserver\run.bat（等 40 秒）
2. 雙擊    I:\RHD\RHDLauncher-rs\RHDLauncher.exe（按 UAC 是）
   → 內建 relay 自動監聽 2000/29467 -> 3000
   → 自動以原版 Login.exe 啟動客戶端
3. 畫面上選伺服器 → 入場 → 選角色 → 入場
```

**實測結果**：RHDLauncher(pid 21040) 內建 relay 接管 2000/29467，
客戶端連入 2000 → relay → 3000，連線數 3，完全不需要 Python。

## 三、RHDLauncher 與原版流程對照

| 功能 | 原版 Login.exe | RHDLauncher | 判定 |
| :--- | :--- | :--- | :--- |
| 讀伺服器清單 | LocalService.ini | servers.ini | ✅ 更好（明文可改） |
| UI | DuiLib（第三方框架） | egui 自製 | ✅ 更好 |
| 虛擬螢幕/中繼 | 無 | 內建 relay | ✅ 更好 |
| 封鎖官方清單 | 無 | hosts（內建） | ✅ 更好 |
| GameGuard 通過 | ★ 只有它能 | 改為呼叫原版 | ⚠️ 必要保留 |
| 注入 LinLogin.dll | ★ 只有它能 | 改為呼叫原版 | ⚠️ 必要保留 |
| 版本號覆寫 …05 | ★ 只有它能 | 改為呼叫原版 | ⚠️ 必要保留 |

> **結論**：RHDLauncher 已取代原版「UI + 設定 + 中繼」的全部功能，
> 僅保留原版負責 GameGuard 與 DLL 注入的最後一哩（因為其 VMProtect 無法複製）。

## 四、封包流程（實測對照）

```
S>C  13 bytes  0D 00 0F 42 90 E1 7A A4 29 70 53 00 00     KeyPacket (S_KEY)
C>S 161 bytes  A1 00 01 00 00 00 F4 34 03 96 ...          版本/Init
S>C  90 bytes  5A 00 F0 35 03 08 00 10 76 ...              版本回應
C>S  17 bytes  11 00 02 00 00 00 C7 35 35 36 36 00 35 35 36 36 00   C_AuthLogin (<account>/<account>)
S>C  32 bytes  20 00 EA 00 ...                             S_LoginResult(0) 成功
S>C 237 bytes  ED 00 60 20 ...                             公告
C>S   8 bytes  08 00 03 00 00 00 77 00                    C_LoginToServer (選角)
S>C  55 bytes  37 00 F0 89 00 0A 16 ...                    角色資料
S>C  41 bytes  20 00 C5 B4 FA B8 D5 31 00 ...              角色資訊
```

## 五、已踩過的坑（勿重犯）

| 坑 | 現象 | 正確做法 |
| :--- | :--- | :--- |
| 停發 KeyPacket | 客戶端連上但不送任何資料 | **保留發送** |
| relay `create_connection(timeout=5)` 未清除 | 5 秒後 `timed out` 斷線 | `settimeout(None)` |
| DB 角色名繁體 | `空PC进入游戏被踢, 请求角色名= 测试1` | DB 改簡體 |
| 自製 Login.exe（LoginHost） | GameGuard 380 | 用原版 |
| 直接啟動 `Lin.bin` | `GetLastError(found==false)` 後停住 | 必須經 `Login.exe` 交接 |
| 自製注入 `LinLogin.dll` | 版本號仍 `...01`（應為 `...05`） | 需原版流程 |
| `GameGuard` 目錄改名 | 380 | 保持原名 |

## 六、帳密與資料現況

| 項目 | 值 |
| :--- | :--- |
| 帳號 | `<account>` |
| 密碼 | `<account>` |
| 角色 | `测试1`（簡體），15 級，南邊森林 |
| DB 備份 | `characters_backup_20261024` |

## 七、繁化現況

- **編碼架構不用改**：`S_TWYX`/`C_TWYX` 已在 `LineageClient` 建構子設 `true`
- **DB 已是繁體**（NPC、物品名稱）
- **待辦**：伺服器程式碼內的簡體硬編碼字串（例：`请扔给...金币`）
- 既有紀錄：[[20261024-L1RHD核心語系提示繁體化修復紀錄]] 已修 TR-BUG-52~59

## 八、變更記錄

| 日期 | 變更 |
| :--- | :--- |
| 2026-10-24 | 初版。實測進遊戲成功（角色 15 級於南邊森林）。固化啟動組合與 7 個必要條件。 |
| 2026-10-24 | 新增一鍵腳本 `Start_Game.bat`；新增 RHDLauncher 獨立運作驗證（內建 relay 接管，無需 Python）。 |
