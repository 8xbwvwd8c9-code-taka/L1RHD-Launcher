# B1 Direct-Launch Mode — 零黑盒直啟與六刀補丁（AI 說明文件）

> **2026-10-29：下文為歷史紀錄，不可照此編譯、部署或啟動。** 舊 B1 跨程序逐筆寫碼已禁用；不得以調整等待秒數代替同步驗證。原版 `Login.exe`、`LinLogin.dll` 也不在目前啟動流程中。新自製 DLL 目前只通過測試程式的載入測試，真實遊戲仍為 HOLD。詳見 `native/BOOTSTRAP.md`。

> **本文件同時供人類與 AI 代理人閱讀。** 若你是 AI 代理人被要求維護 RHDLauncher 的啟動邏輯，請先讀完本文件再動手。
> 證據等級：靜態反組譯＋執行期記憶體取證（PASS，2026-10-24/25）；實機驗收：**待測試**。

---

## 1. 這是什麼

RHDLauncher 有兩種啟動客戶端的方式，由 `launcher.json` 的 `direct_launch` 開關選擇：

| 開關 | 路線 | 機制 | 狀態 |
| :--- | :--- | :--- | :--- |
| `false`（預設） | **路線 A：託管** | 啟動原版 `Login.exe`（VMProtect 加殼黑盒），由其注入 `LinLogin.dll` 完成交接 | ✅ 實測 PASS |
| `true` | **B1：零黑盒直啟** | 直接啟動 `bin64\Lin.bin`，待 SecureEngine 解殼完成後，由 launcher 自身寫入六刀記憶體補丁 | ⏳ 已完成開發，**未實機測試** |

B1 的意義：完全擺脫 `Login.exe`、`LinLogin.dll`、`eat.exe` 三個閉源黑盒，啟動鏈上每一行程式碼都在本專案源碼內。

## 2. BIN 版本號對應（本任務要求的核心資訊）

天R客戶端（`bin64\Lin.bin` ＝ `LogonX64.bin`，MD5 `18328DB21B0A13C63F399AF80DC82A33`）的版本號
是登入日誌（`lin.log`，XOR 0x4D 混淆）第一行 `client version` 的值，同時上報伺服器（protobuf `CS_CLIENT_VERSION_INFO` 第 4 欄）：

| 值 | 意義 | 誰寫的 |
| :--- | :--- | :--- |
| `2002122601` | 未補丁原始版本 ＝ **SecureEngine 解殼完成指標** | 客戶端自身（硬編碼於加殼檔內，解殼後寫入 `ImageBase+0x5CBB800`） |
| `2002122605` | 交接完成版本 ＝ GameGuard 成功路徑＋握手欄位就緒 | 補丁寫入（原版鏈=LinLogin.dll；B1=launcher 的 `patches.rs`） |

**B1 以「讀到 01」為補丁時機、以「寫入 05」為完成標誌** —— 與原版 `LinLogin.dll` 的 `TestFunction` 內部邏輯完全一致。

## 3. 六刀補丁表（`src/patches.rs` 的單一事實來源）

全靜態位元組序列（無任何執行期計算），相對客戶端模組基址（實測 `0x140000000`，程式以模組表動態取得）：

```text
RVA        位元組                語意
0x5CBB800  6D F7 55 77          版本欄1 = 2002122605
0x5CBB808  6D F7 55 77          版本欄2 = 2002122605
0xE658B1   90 90                NOP 條件跳轉 → 檢查函式恆返成功
0x97FC8C   E9 C9 03 00 00       jmp +0x3C9 → GameGuard 成功路徑（跳過含錯誤碼 380/500 的分派器）
0x97D020   C3 90 90 90 C3 90    函式開頭 RET（閹割 GG 相關檢查）
0x9E4CA8   F2 C7 0E 96          握手欄位 imm32 = 0x960EC7F2（CS_CLIENT_VERSION_INFO 構造現場）
```

寫入程序（`patches::wait_and_patch`）：
1. `OpenProcess(VM_READ|VM_WRITE|VM_OPERATION|QUERY)`；
2. 模組快照找 `LogonX64.bin`/`Lin.bin` 基址（不信賴固定 ImageBase）；
3. 每 50ms 輪詢 `base+0x5CBB800`，直到 dword ＝ `2002122601`（冪等：若已是 05 直接成功返回；逾時 120s）；
4. 逐刀：讀原值記 log → `VirtualProtectEx(RWX)` → `WriteProcessMemory` → 讀回覆核 → 還原頁保護；
5. 六刀全數覆核成功才算 Ok，否則 Err 並記 log。

## 4. 啟用方式

`RHDLauncher.exe` 同目錄的 `launcher.json` 加一行：

```json
{ "direct_launch": true }
```

其余流程（自動更新/吃檔 → 2000/29467→伺服器 中繼）兩條路線完全共用，不受開關影響。

**權限需求（2026-10-25 更新）**：manifest 已改為 `asInvoker`——**雙擊即開，不再彈 UAC**。
B1 模式不需要管理員（hosts 封鎖可略過、補丁寫入與客戶端同權限即可）；
路線 A（託管原版 Login.exe）建議仍以管理員執行，否則 hosts 封鎖失效（僅影響防遠端清單綁架）。

## 5. 驗收指標（2026-10-25 01:43 實機驗收：補丁層 PASS）

- [x] launcher log 出現「解殼完成指標出現」（3178ms）與「六刀全數寫入完成」
- [x] 每刀的「原值」記錄 ≠ 寫入值、覆核全數成功
- [x] `lin.log`（XOR 0x4D）第一行為 `client version 2002122605`（已解碼實證）
- [x] 無 GameGuard 380 對話框、無 6 秒自殺（客戶端存活至登入畫面）
- [x] 客戶端連入本機中繼（2000→3000；伺服器未開時僅出現 3000 拒絕連線，屬預期）
- [ ] 完整登入鏈進世界（待伺服器開啟後補驗）

實測原值情報（第 4 刀位補丁前 imm32 為 `0x930FD7E2`＝官方客戶端上報值；LinLogin 改為 `0x960EC7F2`）。
基址取得策略（按防禦深度）：**PEB ImageBaseAddress（NtQueryInformationProcess）** → 慣用基址 0x140000000（驗 TimeDateStamp）→ VirtualQueryEx 掃描。SecureEngine 會隱藏模組表並抹除映像標頭，前兩輪策略（Toolhelp／MZ 掃描）實測失效。

運行注意：B1 模式下 launcher **不自我退出**（補丁執行緒在本進程內）；關閉遊戲後手動關閉 launcher（hosts 受控區段隨之還原）。

失敗時診斷順序：log 裡六刀哪刀失敗 → `I:\RHD\.re\cap` 管線重跑對比補丁前後記憶體 → 若 380 復發先補跑 `eat.exe 1 1 global` 對照（使用者判定 eat.exe 為吃檔工具可忽略，此為排除法）。

## 6. 證據鏈（AI 代理人引用時的出處）

| 事實 | 出處 |
| :--- | :--- |
| 六刀表與反組譯 | 黑曜石《20261025-天R登入器執行期採證與LinLogin六刀補丁破解》§10.1/§10.6 |
| 執行期傾印（解密 LinLogin 14MB / 脫殼 Lin 120MB） | `I:\RHD\.re\cap\out\run3\` |
| 逆向報告主檔 | `I:\RHD\.re\REPORT_Login.md` §11 |
| 採證管線（可重跑） | `I:\RHD\.re\cap\orch1.ps1` |
| lin.log XOR 0x4D | 同筆記 §3 |

## 7. 修改風險與界線（AI 紅線）

- **嚴禁**把六刀改成「猜出來的新值」——表內每一位元組都有執行期證據背書；要改先重跑採證管線取證。
- 客戶端版本更新（MD5 變動）後六刀 RVA 可能失效：`wait_and_patch` 的輪詢逾時是第一道警報，勿以固定 ImageBase 或移除冪等檢查的方式「修」它。
- 本專案為 Rust；**與 Java 伺服器（L1RHD）無任何介面變更**——路線 A 實測已證明伺服器接受版本 05 客戶端狀態，B1 產生的是位元組級等價狀態。
- 回退：`launcher.json` 刪掉 `direct_launch` 或設 `false` 即回路線 A，無需回滾程式碼。
