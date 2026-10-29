# B2 進程內啟動路線（rhd_inproc.dll）

B2 是本啟動器的現行遊戲啟動路線：登入器以 `CREATE_SUSPENDED` 啟動客戶端、把自製的
`rhd_inproc.dll` 注入客戶端進程，由 DLL **在客戶端進程內部**完成啟動期補丁。
啟動器對客戶端唯一的跨程序寫入，是注入機制本身的「DLL 路徑字串」。

歷史脈絡：早期 B1 路線由啟動器以 `WriteProcessMemory` 從外部逐刀改寫客戶端程式碼，
曾與保護驅動的啟動窗口相關地觸發整機凍結（根因未證實），該路線與其全部寫碼模組
（`patches.rs`、`inject.rs`、`remotehook.rs`、`sockhook.rs`、`loginhost.rs`）已以
`compile_error!` 永久封存，本倉庫保留檔案僅作為禁令說明，不可重新編入。

## 啟動時序

```
1. 寫 Login.ini（127.0.0.1:中繼埠；客戶端以明文 Login.ini 覆蓋官方加密設定的既有機制）
2. CreateProcessW(bin64\Lin.bin, CREATE_SUSPENDED, cwd=遊戲根目錄)
3. VirtualAllocEx + WriteProcessMemory（僅 DLL 路徑字串）
   → CreateRemoteThread(LoadLibraryW) → rhd_inproc.dll 載入（DllMain 只做最小初始化）
4. ResumeThread（SecureEngine 開始脫殼）
5. 建立結果事件 Local\RHDInprocDone-<pid> / Local\RHDInprocFail-<pid>
6. CreateRemoteThread(RhdPatchStart)
   → DLL 進程內輪詢版本欄（client base + RVA 0x5CBB800）：
     每次讀取前以 VirtualQuery 確認頁已提交；出現 2002122601（脫殼完成指標）才動手；
     出現 2002122605 走冪等驗證；120 秒逾時則明確失敗
7. DLL 完成後以事件回報結果；啟動器記錄 rc、交棒常駐中繼 daemon 後約 3 秒退出
```

## 補丁內容（八刀）與交易紀律

補丁表是 `native/inproc.cpp` 內的編譯期常數（唯一事實來源），對應客戶端
`bin64/Lin.bin`（見 README 的指紋表）啟動契約的八個固定 RVA：

| # | RVA | 作用 |
|---|-----|------|
| 1–2 | 0x5CBB800 / 0x5CBB808 | 版本欄 2002122601 → 2002122605 |
| 3 | 0xE658B1 | NOP 條件跳轉（函式恆返成功） |
| 4 | 0x97FC8C | GameGuard 結果處理強制走成功分支（380 分派器不再可達） |
| 5 | 0x97D020 | GG 旗標/字串檢查函式開頭 RET 閹割 |
| 6 | 0x9E4CA8 | CS_CLIENT_VERSION_INFO 握手欄位 imm32 → 0x960EC7F2 |
| 7 | 0xA7AC57 | 發送端跳過封包加密（明文） |
| 8 | 0xA7A9C5 | 接收端跳過 RC4 解密（明文） |

寫入遵循交易式紀律（與 `native/transaction_fixture.cpp` 的可執行規格一致）：

1. **全部站點原值先驗證**——任一原值不符即零寫入拒絕（rc=0x110）；
2. 對每個觸及頁面提升保護並記錄原保護（已可寫的頁跳過）；
3. 逐站寫入 → 讀回覆核 → `FlushInstructionCache`；
4. 任何失敗回滾全部已寫站點與頁保護；保護還原失敗回報 `kUnsafeStop`，不靜默繼續。

原值表已對該 BIN 的執行期解殼映像（補丁前／補丁後兩份）逐位元組驗證；
客戶端更新導致原值或 RVA 變動時，補丁會在**零寫入**下明確失敗，不會留下半補丁狀態。

## 建置

需求：Windows x64、Rust stable、CMake + MSVC C++（x64）、PowerShell。

```powershell
# Rust 啟動器
cargo build --release          # 產出 target/release/rhd-launcher.exe

# 自製 DLL 與測試（建置目錄請放在倉庫外）
powershell -File native\build.ps1 -BuildDir <建置產物目錄> -Configuration Release
# build.ps1 = cmake 設定 + 建置 + ctest（20 項：bootstrap 4、transaction 12、inproc 4）
```

`native/Detours-1dbd4ea.zip` 是固定版本的 Microsoft Detours 原始碼（MIT，見
`native/LICENSE.Detours.txt`），CMake 會核對其 SHA-256 後解壓建置，不會於建置時下載。

## 部署佈局

```
<遊戲根目錄>\
  RHDLauncher-B2.exe      # 啟動器主程式（dist/ 內同名 rhd-launcher.exe）
  rhd_inproc.dll          # 進程內補丁 DLL（dist/ 內同名）
  servers.ini             # 伺服器清單（見 servers.example.ini；不入庫）
  launcher.json           # {"inproc_launch": true} 為預設；false＝完全停用啟動
  rhd_inproc.log          # DLL 補丁日誌（執行期產生）
  bin64\Lin.bin           # 客戶端（不入庫）
```

DLL 搜尋順序：啟動器所在目錄 → 遊戲根目錄。`rhd_inproc.dll` 缺失或 `inproc_launch=false`
時，啟動按鈕保持停用狀態。

## 驗收狀態

| 項目 | 狀態 |
|------|------|
| native ctest（20 項，Release＋Debug） | PASS |
| 其中 inproc 四情境（成功／原值不符零寫入／逾時／冪等） | PASS（probe 以與生產相同的注入鏈驗證） |
| 啟動器 cargo test | PASS |
| 真實遊戲啟動、登入、進世界 | **未驗證**（需在隔離環境人工驗收） |

fixture 驗證不等於遊戲驗收：保護驅動對進程內寫入的相容性、以及真實登入鏈，
都必須由持有客戶端與伺服器的人自行驗證。驗收指標與回退方式（`inproc_launch=false`
一鍵停用）見上節；發生整機凍結時請保留 Windows 事件檢視器證據，勿以調整時序參數代替根因分析。

## 已知限制

- 客戶端 `bin64/Lin.bin` 內嵌 `requireAdministrator`：全鏈提權是硬需求。
- 補丁表只對 README 指紋表中的 x64 2020-02-12 build 有效；其他世代未驗證。
- DLL 執行緒在逾時（預設 120 秒）前不會放棄等待脫殼指標；脫殼異常時登入會明確失敗而非靜默。
