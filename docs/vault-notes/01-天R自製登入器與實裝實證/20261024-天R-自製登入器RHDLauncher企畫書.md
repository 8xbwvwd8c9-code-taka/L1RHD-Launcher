---
title: 天R 自製登入器 RHDLauncher 企畫書
tags:
  - 天R
  - RHD
  - 登入器
  - RHDLauncher
  - Rust
  - egui
  - relay
  - 企劃書
created: 2026-10-24
updated: 2026-10-31
status: SUPERSEDED
aliases:
  - RHDLauncher 企畫書
  - 天R 自製登入器規格
  - RHD 登入器重寫
---

# 天R 自製登入器 RHDLauncher 企畫書

> [!CAUTION] 2026-10-31 現況改寫（以最新證據為準，本篇降級為歷史規格）
> 本文「OPEN（尚未實作）」的狀態與三項關鍵判斷**已被 2026-10-25～10-30 的 B1→B4 實機證據推翻**，最新架構以 [[20261030-天R登入器成功登入流程固化SOP]] 與 [[20261030-天R登入器B4中繼補發S_KEY與進入世界實證]] 為準：
> 1. **狀態**：登入器已實作完成並兩次實機進入世界（B4，`G:\L1RHD\RHDLauncher-B4-skkey.exe`）。「尚未實作」作廢。
> 2. **§1.2 排除條款（不注入 DLL、不改寫客戶端記憶體）**：已被推翻——現行 B4 路線就是**進程內補丁**（`rhd_inproc.dll` 注入客戶端寫八刀）。原因是六刀/八刀補丁表已完整破解（[[20261025-天R登入器執行期採證與LinLogin六刀補丁破解]]），且官方 `LinLogin.dll`／歐霸天堂 `Login.dll` 本身就是同構的 Detours 行程內補丁模組，架構上等價安全。
> 3. **§2.4「不能取代 Login.exe」**：已被推翻——B1 起不再需要 `Login.exe`（`direct_launch=true`，CREATE_SUSPENDED 直啟 `LogonX64.bin`＋行程內補丁）。
> 本文其餘取證（節區表、否定實驗、relay 七件事、hosts/lineage.cfg 規格）仍有效，保留供追溯。

> **狀態**：~~OPEN（企畫完成，尚未實作）~~ → **SUPERSEDED（已由 B4 實作並實機驗證，見上方 CAUTION）**
> **對象**：`I:\RHD\L1RHD`（客戶端）、`X:\L1RHD\L1RHDserver`（Java 伺服器，`gs.jar`）
> **取代對象**：`I:\RHD\RHDLauncher\rhd_launcher.py`（Python + tkinter，717 行，能跑但依賴 Python 環境）
> **相關**：[[20261023-S880-自製登入器LC880Launcher完成與登入實證]]、[[20261022-815登入器清單格式與遠端下載突破]]、[[踩坑記錄與失敗方案歸檔]]、[[天R-L1RHD核心安全與專屬漏洞深度審計報告]]
> **本文性質**：實作規格書。後續 AI 只讀本篇 + 現場驗證即可從零做出成品，不必重跑逆向。

---

## 0. 一頁摘要

寫一個 **Rust 單檔 `RHDLauncher.exe`**，把「開遊戲」壓縮成一鍵：

1. 自我提權（`Lin.bin` / `Login.exe` 的 manifest 都是 `requireAdministrator`，不提權連 CreateProcess 都過不了）
2. 起內建 TCP 中繼：`127.0.0.1:2000` + `127.0.0.1:29467`（含 `::1`）→ `目標IP:目標Port`（預設 `127.0.0.1:3000`）
3. 寫 hosts 受控區段，擋掉官方清單伺服器 `kaixingongyi.com`（這是「以前必須斷網」的真正原因）
4. 可選改寫 `lineage.cfg` 的 IP/Port 欄位
5. 背景拉起 `Login.exe`（**不取代它**，見 §2.4），存活監控
6. 帳密以加密方式保存（**不寫入本筆記、不明文落盤**），用 UI Automation / SendInput 自動填入
7. 全部狀態即時顯示在 egui 介面上，可縮到系統匣

**不做的**：不逆向 `Login.exe`、不 hook `ws2_32.connect`、不重寫 C_LOGIN 協議。理由見 §2.4，這是本企畫最重要的決策。

---

## 1. 需求與範圍

### 1.1 使用者原始需求

> 「我希望你能寫一個我們自己版本的登陸器，參照其他開源的方式走，黑曜石筆記內有。」

拆解成可驗收的三條：

| # | 需求 | 驗收方式 |
|---|---|---|
| R1 | 我們自己的登入器，不是官方 `Login.exe` 直接丟給使用者 | 使用者雙擊 `RHDLauncher.exe`，全程不手動開 `Login.exe` |
| R2 | 參照開源／既有做法（LC880 那套） | 架構對照見 §3.3，說明哪些照抄、哪些刻意不同 |
| R3 | 免 Python、免裝環境、單檔可散布 | 目標機器只放一個 `.exe` + `launcher.json` 即可執行 |

### 1.2 明確排除的範圍

以下**不在**本企畫內，且已在 §2.4 用實驗證明「不是不想做，是成本不可控」：

- 逆向 `Login.exe` / `LinLogin.dll`（VMProtect 全虛擬化）
- 實作 C_LOGIN / Init 封包協議
- 注入 DLL、inline hook、改寫客戶端記憶體
- 繞過客戶端的遠端驗證（`3.37.249.199:80`）

若日後真的要走「零 Login.exe」，另開研究線，見 §9.3。

---

## 2. 既有啟動鏈實況（2026-10-24 實測取證）

這一節是整份企畫的地基。**不要跳過**，後續 AI 若質疑「為什麼不直接啟動 Lin.bin」，答案全在這裡。

### 2.1 檔案角色表

| 檔案 | 大小 | MD5 / 特徵 | 真實角色 |
|---|---|---|---|
| `Login.exe` | 8,679,936 | `.vmp0` vsize=0x6023F4 **raw=0**<br>`.vmp1` vsize=0x83FD24 raw=0x83FE00 | 只是啟動器：拉起客戶端 → `wait game Initialize` → 等 6~10 秒 → **自己退出**。核心邏輯 100% 在 VMProtect 虛擬機內。 |
| `LinLogin.dll` | 5,713,408 | `.vmp0` vsize=0x36F636 raw=0<br>只匯出 3 個函式 | 登入模組。匯出 `Initializedll` / `loadingdll` / `TestFunction`，同樣 VMProtect。 |
| `LinLogin-f287ddb94a5e.dll` | 5,713,408 | 與上者同大小 | 同檔的防篡改改名副本（客戶端會改檔名載入）。 |
| `bin64\LogonX64.bin` | 18,989,568 | **MD5 `18328DB21B0A13C63F399AF80DC82A33`** | 登入＋封包過濾器本體。**內建 `ws2_32.connect` hook**。 |
| `bin64\Lin.bin` | 18,989,568 | **MD5 同上，完全相同** | 遊戲本體。 |
| `bin64\Lin_x64.bin` | 18,720,768 | 不同 | 另一分支版本（本次未分析）。 |
| `Login.zip` | 8,321,774 | 內含 `Login.exe` 8,679,936 | 與目錄中的 `Login.exe` **同位元組完全相同**，也是加殼版，**沒有乾淨版本可用**。 |

> **關鍵事實 1**：`LogonX64.bin` 與 `Lin.bin` 是**同一個檔案**。行為差異來自**啟動時的檔名**（`argv[0]`），不是內容。

### 2.2 正常啟動時間線（09:18 那次，成功進遊戲）

| 時間 | 來源 | 訊息 |
|---|---|---|
| 09:18:54 | `Logs\LogonX642026-10-24.log` | `正在初始化游戏过滤例程` / `准备处理游戏NP钩子` |
| 09:18:56 | 同上 | `start patach game packet code 8.8.8.8!` |
| 09:19:04 | `Logs\Login2026-10-24.log` | `wait game Initialize` |
| 09:19:10 | `LogonX64` | `=======正在重定向连接>>127.0.0.1 2000 =======` |
| 09:19:10 | relay | `[#1] 客戶端連入 127.0.0.1:54204（埠 2000）` → 已接上 `:3000` |
| 09:19:10 | `Login` | `game Initialize Complete! Start Exit Login Executes`（Login.exe 退出） |
| 09:20:09 | `LogonX64` | 第二次 `=======正在重定向连接>>127.0.0.1 2000 =======` |
| 09:20:09 | relay | `[#2] 客戶端連入 127.0.0.1:53646（埠 2000）` |
| 09:20:09 | 檔案系統 | `lineage.cfg` 被客戶端自己改寫（`LastWriteTime` 對得上） |
| 09:20:12 | `lin.log` | `HandleENterWorld`、`HandleUserStartDone`、全部 `Hud*Layout Visible` → **進入遊戲世界** |

`lin.log` 是 XOR `0x4D` 的 UTF-8 文字；角色名段落是 **Big5 位元組**（XOR 後不是合法 UTF-8，會顯示成亂碼，屬正常）。

### 2.3 三個否定實驗（證明「直接啟動」行不通）

#### 實驗 A：hardlink 成 `.exe` + `ShellExecute runas` 啟動 `Lin.bin`

```
建立 bin64\Lin_probe.exe → hardlink 到 Lin.bin（MD5 不變，不佔空間）
Start-Process -FilePath Lin_probe.exe -WorkingDirectory I:\RHD\L1RHD -Verb RunAs
```

結果：`pid 49796`、窗口標題 `Lineage`、**7 秒後進程消失**。
`relay_run.log` 最後一筆仍是 `09:20:09` → **完全沒有發起任何連線**。

> 為什麼要 hardlink：`Start-Process -Verb RunAs` 走 ShellExecute，對 `.bin` 副檔名會報「沒有任何應用程式與此操作的指定檔案有關聯」。hardlink 給它一個 `.exe` 名字即可繞過，且內容/MD5/大小完全不變。

#### 實驗 B：`ProcessStartInfo` + `UseShellExecute=false`（CreateProcess）啟動 `Lin.bin`

結果：`WinError 740`（要求的作業需要提升的權限）→ 證明 **`Lin.bin` 的 manifest 是 `requireAdministrator`**。

#### 實驗 C：已提權環境下，用**真實檔名** `LogonX64.bin` 走 CreateProcess

```
（提權的 pwsh）
$psi.FileName = 'I:\RHD\L1RHD\bin64\LogonX64.bin'
$psi.WorkingDirectory = 'I:\RHD\L1RHD'
$psi.UseShellExecute = $false
```

結果：

```
elevated=yes  whoami=DESKTOP-XXXXXXX\admin
CreateProcess ok pid=41532
  [ 1s] alive=True win='Lineage' lin.log=54707 conn=-          cmdline="I:\RHD\L1RHD\bin64\LogonX64.bin"
  [ 2s] alive=True win='Lineage' conn=Established 61812->3.37.249.199:80
  [ 5s] alive=True win='Lineage' conn=Established 61812->3.37.249.199:80
  [ 6s] alive=False win=''       conn=-
```

> `CreateProcess` **不檢查副檔名**，PE 內容就能跑，且 `argv[0]` 保留真實檔名 `LogonX64.bin`。這是唯一能用真實檔名啟動的方式。

**結論**：客戶端脫離 `Login.exe` 後，會先連 AWS 首爾（`3.37.249.199:80` 或 `54.116.234.232:80`，兩者都是 `ec2-*.ap-northeast-2.compute.amazonaws.com`，80 埠有活的 HTTP 服務，根路徑回 404），拿不到東西就 **6 秒自殺**。它不會自己去連 `127.0.0.1:2000`。

### 2.4 為什麼不能取代 `Login.exe`（本企畫的核心決策）

三個獨立的障礙，任何一個都足以讓「重寫登入器」變成月級工程：

1. **VMProtect 全虛擬化**：`Login.exe` 的 `.text` raw=0（整段被抽到 VM 位元組碼）。`.vmp0` 的 vsize 6.3MB、raw 0 bytes，代表要看懂它只能動態追 VM handler。`LinLogin.dll` 同理。這不是「反編譯一下」的量級。
2. **遠端驗證未知**：§2.3 實驗 C 證明客戶端啟動會打 AWS 的 HTTP API。要讓它不連，得先知道它送什麼、期望回什麼。目前沒有任何證據。
3. **啟動契約不透明**：`Login.exe` 到底餵了什麼給客戶端（共享記憶體？具名管道？`LinLogin.dll` 的 `Initializedll` 注入了什麼？）完全沒有證據。而 §2.3 實驗 A/C 證明「不餵就跑不動」。

**因此本企畫的定位是「把 Login.exe 關在使用者看不到的地方」**，而不是消滅它。

> **重要對照**：`LC880Launcher` 當年也沒有取代 `Login.exe`。它的架構是 `C# UI → Python engine → 在 Lin.bin 內 hook ws2_32.connect → relay 分類連線 / Init 代答 / C_LOGIN 等長改寫`，**前提仍是 Login.exe 先把客戶端正常拉起來**。本企畫在「不 hook」上比它保守一級，換取「完全不用碰加殼客戶端記憶體」的穩定性。

---

## 3. 目標架構

### 3.1 總覽

```
使用者雙擊 RHDLauncher.exe
        │
        ├─(1) 提權檢查 ──失敗──> ShellExecuteW(runas, 自己) ──> 重新進入
        │
        ├─(2) 起 relay（tokio task）
        │       bind 127.0.0.1:2000   ┐
        │       bind 127.0.0.1:29467  ├─> connect 目標IP:目標Port
        │       bind [::1]:2000       ┘        （預設 127.0.0.1:3000）
        │
        ├─(3) hosts 受控區段
        │       127.0.0.1 kaixingongyi.com
        │       127.0.0.1 www.kaixingongyi.com
        │       + ipconfig /flushdns
        │
        ├─(4) 可選：改寫 I:\RHD\L1RHD\lineage.cfg（tag 0x07 IP / tag 0x08 Port）
        │
        ├─(5) 啟動 I:\RHD\L1RHD\Login.exe（自身已提權 → 直接 CreateProcess）
        │
        ├─(6) 等待登入視窗出現 → 自動填帳密 → 送出
        │
        └─(7) 監控進程存活 → UI 狀態 / 系統匣 / 日誌
```

### 3.2 執行模型

- **主執行緒**：egui 事件迴圈（`eframe::run_native`），不可阻塞。
- **tokio runtime**：獨立執行緒持有（`tokio::runtime::Runtime` + `Handle`），跑 relay 的 accept/pump。
- **通訊**：UI ↔ runtime 用 `crossbeam-channel`（日誌與狀態事件單向推送），
  設定變更用 `Arc<Mutex<AppConfig>>`。
- **絕不**在 UI 迴圈內做 `std::net` 阻塞呼叫。這是 Python 版用 threading 避開、Rust 版最容易寫錯的地方。

### 3.3 與既有實作的對照

| 能力 | `rhd_launcher.py`（現況） | `LC880Launcher`（參考） | 本企畫 `RHDLauncher` |
|---|---|---|---|
| 語言／執行期 | Python 3 + tkinter | C# WinForms + Python(PyInstaller) | **Rust 單檔** |
| UI | tkinter，無樣式 | WinForms | **egui，自繪** |
| relay | socket + thread | Python relay | **tokio + async** |
| hosts 封鎖 | 有 | 無（不需） | **有（照抄 Python 版語意）** |
| `lineage.cfg` 改寫 | 有（實驗性） | 無 | **有，且改成「每次啟動前覆蓋」** |
| 提權 | 要求使用者手動右鍵 | C# manifest | **manifest + 執行期自我重啟** |
| 啟動對象 | `Login.exe` 或 `LogonX64.bin` | `Lin.bin`（Login 之後） | **`Login.exe`（唯一能跑的路）** |
| connect hook | 無 | 有 | **無（見 §2.4）** |
| 帳密自動填 | 無 | 有（C_LOGIN 等長改寫） | **UI Automation / SendInput** |
| 日誌 | 文字檔 | 文字檔 | **文字檔 + UI 面板 + 可匯出** |

---

## 4. 技術選型

### 4.1 版本基準（2026-10-24 實測環境）

| 項目 | 版本／狀態 |
|---|---|
| `rustc` | 1.98.1 (48a229cea 2026-09-01) |
| `cargo` | 1.98.1 (797e8a9bc 2026-08-05) |
| `egui` | **0.37.0**（`cargo search egui` 當前最新） |
| `tokio` | **1.54.1** |
| crates.io | 可連線（registry cache 在 `%USERPROFILE%\.cargo\registry\cache\index.crates.io-1949cf8c6b5b557f`） |

### 4.2 依賴清單

```toml
[package]
name    = "rhd-launcher"
version = "0.1.0"
edition = "2021"

[dependencies]
eframe       = "0.37"          # egui 的 native 外殼
egui_extras  = "0.37"          # 表格 / 圖片載入
tokio        = { version = "1.54", features = ["rt-multi-thread", "net", "time", "sync", "macros"] }
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
crossbeam-channel = "0.5"
windows      = { version = "0.61", features = [
    "Win32_Foundation",
    "Win32_UI_WindowsAndMessaging",
    "Win32_UI_Accessibility",
    "Win32_Security",
    "Win32_System_Threading",
    "Win32_System_Registry",
] }

[build-dependencies]
winresource = "0.1"

[profile.release]
opt-level = "z"      # 體積優先
lto       = true
strip     = true
panic     = "abort"
```

> 版本以 `cargo search <crate>` 的當下最新為準；升級前先跑 §8 的驗收清單。
> `windows` crate 版本若 API 對不上，以 `cargo doc -p windows --open` 的實際簽名為準。

### 4.3 GUI 選型理由（已與使用者確認：egui）

| 方案 | 否決／採用理由 |
|---|---|
| **egui / eframe** | **採用**。純 Rust、即時模式、單檔編譯、易做深色主題。唯一成本是需內嵌中文字型（見下）。 |
| `native-windows-gui` | 體積最小（<1MB）但外觀是傳統 Win32，與「我們自己的登入器」的品牌感不符。 |
| Tauri | 依賴 WebView2 執行期，且產物不是單一 exe，違反 R3。 |
| Slint | 生態較小，中文字型同樣要自帶，優勢不足以抵換 learning cost。 |

**中文字型處理**（egui 預設不含 CJK，不處理會整片豆腐）：

1. 從 `C:\Windows\Fonts\msjh.ttc`（微軟正黑體）或 `msyh.ttc` 複製。
2. 放 `assets/NotoSansTC-Regular.otf`（或 `msjh`）並 `include_bytes!` 編進執行檔。
3. 啟動時：

```rust
let mut fonts = egui::FontDefinitions::default();
fonts.font_data.insert(
    "cjk".to_owned(),
    egui::FontData::from_static(include_bytes!("../assets/NotoSansTC-Regular.otf")),
);
fonts.families.get_mut(&egui::FontFamily::Proportional).unwrap()
     .insert(0, "cjk".to_owned());
ctx.set_fonts(fonts);
```

> 字型檔約 4~6MB，會直接反映在 exe 體積。想省體積可先在建置機用 `pyftsubset` 只留 Big5 常用字，但**不要**為了體積犧牲可讀性——這個 launcher 的價值就在「看得舒服」。

### 4.4 提權設計

`Lin.bin` / `Login.exe` 都是 `requireAdministrator`（§2.3 實驗 B 的 WinError 740 為證）。因此 `RHDLauncher` 自己也必須是管理者，否則**連啟動遊戲都會失敗**。

兩層保險：

1. **`build.rs` 嵌入 manifest**（靜態提權，開機就問）：

```rust
// build.rs
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_manifest(r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="requireAdministrator" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
</assembly>"#);
        res.compile().unwrap();
    }
}
```

2. **執行期檢查 + 自我重啟**（給「使用者用 `cargo run` 或非提權 shell 開啟」的場合）：

```rust
fn is_admin() -> bool { unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() } }

fn relaunch_elevated() -> bool {
    let exe = std::env::current_exe().unwrap();
    // ShellExecuteW(None, "runas", exe, None, cwd, SW_SHOWNORMAL) > 32
    // 回傳 true 代表已送出（本 process 應立即 exit(0)）
}
```

> 順序：`main()` 第一行就檢查 `is_admin()`；`false` → `relaunch_elevated()` → `exit(0)`。**不要**在提權前起 relay 或寫 hosts，否則會留下半成品狀態。

---

## 5. 模組設計

### 5.1 目錄結構

```
I:\RHD\RHDLauncher-rs\
├── Cargo.toml
├── build.rs
├── assets\
│   └── NotoSansTC-Regular.otf
├── src\
│   ├── main.rs          # 入口：提權檢查 → 建 runtime → 開視窗
│   ├── config.rs        # launcher.json 讀寫 + 預設值 + 遷移
│   ├── relay.rs         # tokio TCP 中繼
│   ├── hosts.rs         # hosts 受控區段
│   ├── cfgfile.rs       # lineage.cfg TLV 讀寫
│   ├── elevate.rs       # IsUserAnAdmin / ShellExecuteW runas
│   ├── launch.rs        # 啟動 Login.exe + 進程存活監控
│   ├── autologin.rs     # UI Automation / SendInput 填帳密
│   ├── logging.rs       # 環形緩衝 + 檔案落地 + UI 廣播
│   └── ui\
│       ├── mod.rs       # eframe::App 實作
│       ├── widgets.rs   # 可重用元件（狀態燈、Port 清單、日誌面板）
│       └── theme.rs     # 深色主題 / 字型注入
└── README.md
```

### 5.2 `config.rs`

設定檔位置：**與 exe 同目錄**的 `launcher.json`（不是 AppData，方便整個資料夾搬移）。

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub game_dir: String,            // "I:\\RHD\\L1RHD"
    pub login_exe: String,           // "Login.exe"
    pub listen_host: String,         // "127.0.0.1"
    pub listen_ports: Vec<u16>,      // [2000, 29467]
    pub target_host: String,         // "127.0.0.1"
    pub target_port: u16,            // 3000
    pub block_domain: bool,          // true
    pub blocked_domains: Vec<String>,// ["kaixingongyi.com", "www.kaixingongyi.com"]
    pub rewrite_cfg: bool,           // false（預設關）
    pub auto_login: bool,            // true
    pub auto_login_delay_ms: u64,    // 3000（等 Login.exe 視窗）
    pub minimize_to_tray: bool,      // true
}

impl Default for AppConfig { /* 對應 §0 的預設值 */ }
```

**規則**：
- 讀檔失敗或欄位缺漏 → 用 `Default` 補，**不要**讓 launcher 起不來。
- 每次 UI 變更即寫回（`serde_json::to_string_pretty`，UTF-8 無 BOM）。
- 舊版 Python 的 `launcher.json` 欄位名一致，可直接沿用（見附錄 C）。
- **帳密不放這裡**（見 §5.8）。

### 5.3 `relay.rs`（核心）

**語意必須與 `rhd_launcher.py` 的 `Relay` 類完全一致**，因為那是已在真實環境驗證過的行為。

```rust
pub struct RelayHandle {
    pub running: bool,
    pub bound: Vec<(u16, std::net::SocketAddr)>,
    pub conn_seq: Arc<AtomicU64>,
}

pub fn start(
    listen_host: &str,
    listen_ports: &[u16],
    target_host: &str,
    target_port: u16,
    log: LogHandle,
) -> Result<RelayHandle, String>
```

必做的七件事（少一件就會出現 Python 版當年踩過的坑）：

1. **跳過自我迴圈**：若 `target_host` 是 loopback 且 `port == target_port`，該埠直接略過，否則無限迴圈。
2. **IPv4 綁定失敗 → 用 `SO_REUSEADDR` 硬搶一次**，並在日誌標明「原本被其他程式佔用，已強制接管」。2000 埠常見被 VMware NAT（`vmnat.exe`）佔用，這是真實現象。
3. **同時綁 `::1`**：部分客戶端會用 IPv6 loopback。Windows 上要**先設 `IPV6_V6ONLY=1` 再 bind**，否則 `::1` 會遮蔽 `127.0.0.1`。
4. **每條連線雙向 pump，位元組原封不動**（`tokio::io::copy_bidirectional` 即可，但要在結束時對對方 `shutdown`）。
5. **連目標失敗要立刻關閉下游並記日誌**，不要讓客戶端掛著等。
6. `conn_seq` 遞增編號，日誌格式沿用 `[#N] 客戶端連入 host:port（埠 P）`——**這段格式不要改**，它是跟舊日誌比對的依據。
7. **停止時要能優雅關閉所有 listener 與既有連線**（`CancellationToken` 或 `tokio::select!`）。

> 效能不需要特別優化：這是本機 loopback，遊戲流量 < 100KB/s。`copy_bidirectional` 的 8KB buffer 足夠。

### 5.4 `hosts.rs`

**目的**：`Login.exe` 啟動時第一件事是抓
`http://www.kaixingongyi.com/xinttdj/list.txt?t=<unixtime>`，
抓成功就用**遠端清單覆蓋本機 `LocalService.ini`**，伺服器清單只剩作者的。
抓失敗（＝以前的做法是「斷網」）才退回本機清單。

**所以真正需要的不是斷網，是擋掉這一個網域。**

實作規格（與 Python 版逐條對齊）：

- 路徑：`%SystemRoot%\System32\drivers\etc\hosts`
- 受控區段標記：
  ```
  # >>> RHD Launcher managed >>>
  127.0.0.1 kaixingongyi.com
  127.0.0.1 www.kaixingongyi.com
  # <<< RHD Launcher managed <<<
  ```
- 寫入前**先備份**成 `hosts.rhd.bak`（覆蓋舊備份）。
- 寫入流程：讀全文 → 移除既有受控區段 → 去尾端空行 → 追加新區段 → 寫回。
  **保留原檔位元組**：只針對 ASCII 區段操作，不要整檔重新編碼（Python 版用 `surrogateescape` 就是為了這個）。
- 寫入後執行 `ipconfig /flushdns`（`std::process::Command`，逾時 20 秒，失敗不報錯）。
- 狀態查詢**以檔案內容為準**（`contains(HOSTS_BEGIN)`），不要只信記憶體旗標——使用者可能手動改回去。

> **不要**用「封鎖 IP」或「防火牆規則」替代。hosts 是最小侵入、可一鍵還原的做法。

### 5.5 `cfgfile.rs`（`lineage.cfg`）

**格式（已用 hex 驗證，2026-10-24 二次校對）**：

```
[0..26]   "lineage configuration file"   (26 bytes, 無 NUL)
[26..30]  1a 00 00 00                    (u32 = 26，magic 長度)
[30..36]  00 00 04 00 00 00              (語意不明，實作時勿碰)
[36..40]  6e 2d 72 00                    ("n-r\0"，版本標記)
[40..]    TLV 序列： [tag:u32le][len:u32le][value:len bytes]
```

> **踩過的偏移坑**：第一次解析時把 TLV 起點算成 offset 30，結果第一個 tag 讀出 `0x40000`、後續全歪。**正確起點是 40**。實作時不要硬編 offset，用 `windows(8).position(|w| w == sig)` 找特徵（Python 版就是這樣，所以它一直沒踩到這個坑）。

已確認的欄位：

| tag | len | 值 | 語意 |
|---|---|---|---|
| `0x04` | 4 | `950` | 代碼頁 |
| `0x07` | 4 | `127.0.0.1` | **IP**（4 bytes 原始位址，唯一穩定欄位） |
| `0x08` | 4 | 11702 → **55398**（會變） | **不是連線埠**，見下 |
| `0x14` | 4 | `886` | 未知 |
| `0x1A` | 4 | 5 → **6**（會變） | 未知 |
| `0x1B` | 4 | `5` | 未知 |
| `0x1C` | 4 | `1` | 未知 |
| `0x1D` | 4 | `12` | 未知 |
| `0x2711` | 變長 | Big5 字串 + NUL | 伺服器顯示名稱。**此 tag 疑似沒有 len 欄位**（實測 `11 27 00 00` 後直接接 `b3 aa b4 ae b0 ed b5 f1 00`），解析器要對此特例容錯 |
| `0xFF` | — | `ff ff ff ff 00 00 00 00` | 結束標記 |

**⚠ 已證實「tag 0x08 不是連線埠」（OPEN → 部分結案）**：

| 觀察時刻 | `tag 0x08` 值 | 當時做的事 |
|---|---|---|
| 09:20:09 | `11702`（`0x2DB6`） | 客戶端完成登入、進遊戲世界 |
| **09:32:08** | **`55398`（`0xD866`）** | 我用真實檔名啟動 `LogonX64.bin`（pid 41532），6 秒後退出 |

`lineage.cfg` 的 mtime 停在 **09:32:08**，正好是那次啟動的時刻 → **客戶端每次啟動都會改寫這個檔**。而 `tag 0x08` 從 11702 變成 55398，`tag 0x1A` 從 5 變成 6，兩者都隨啟動漂移。

同時，實際連線走的是 **2000**（`LogonX64` 日誌 `重定向连接>>127.0.0.1 2000`），舊版 Python 註解又寫「29467（0x731B）」。**四個數字全不同** → `tag 0x08` 絕不是「連線埠」。

**推論**：`0x08` 可能是上次連線的隨機 session 值或時間導出的校驗值。**在拿到反證之前，`rewrite_cfg` 對「改連線位址」沒有任何作用**，它唯一的實際價值是「把 IP 固定成 127.0.0.1」，避免使用者手動改壞。

**因此實作要求**：

- `read()` / `write()` 分開，且**預設 `rewrite_cfg = false`**。
- 改寫用「找特徵 + 就地覆蓋」而非重建檔案：
  ```rust
  let sig = [tag, 0, 0, 0, 4, 0, 0, 0];   // tag=0x07 或 0x08
  let pos = data.windows(8).position(|w| w == sig)?;
  data[pos+8 .. pos+12].copy_from_slice(&value4);
  ```
- 改寫後**不要**假裝成功：日誌要印出新值與 hex，讓使用者能自己核對。
- 每次啟動前若 `rewrite_cfg = true`，**無條件覆蓋**（因為 §2.2 已證實客戶端自己會在 09:20:09 把值寫回去）。

### 5.6 `elevate.rs`

```rust
pub fn is_admin() -> bool;
pub fn relaunch_elevated() -> std::io::Result<bool>;  // Ok(true)=已送出
```

- `IsUserAnAdmin` 來自 `windows::Win32::UI::Shell`。
- 提權用 `ShellExecuteW(hwnd=None, "runas", exe, None, cwd, SW_SHOWNORMAL)`，
  回傳值 `> 32` 才算成功；使用者按「否」會拿到 `ERROR_CANCELLED (1223)`，要顯示可讀訊息而非 panic。
- **不在提權前做任何副作用**（見 §4.4）。

### 5.7 `launch.rs`

```rust
pub struct Child { pub pid: u32, pub started_at: SystemTime }

pub fn launch_login(game_dir: &Path, exe_name: &str) -> Result<Child, String>;
pub fn watch(pid: u32, on_exit: impl FnOnce());
```

- 因為 launcher 自己已是管理者，`CreateProcess`／`Command::new` **直接可用**，不需要 `ShellExecute`（§2.3 實驗 B 的 WinError 740 不會發生）。
- `current_dir(game_dir)` 必須設對。`lin.log`、`lineage.cfg`、`<角色名>.loc` 都寫在**遊戲根目錄**，不是 `bin64\`。
- 進程監控用 `wait_for_single_object` 或輪詢 `OpenProcess`+`GetExitCodeProcess`（1 秒一次即可）。
- **不要**在 `Login.exe` 退出時就認定失敗：它本來就會在 `game Initialize Complete!` 後自己退出（§2.2）。真正的存活判斷要看客戶端進程（`LogonX64.bin` / `Lin.bin`）與 relay 的連線數。

### 5.8 `autologin.rs`（風險最高，最後做）

目標：使用者只輸入一次帳密，之後一鍵進遊戲。

三條路徑，按可靠性排序：

| 方案 | 做法 | 風險 |
|---|---|---|
| **A（首選）** | 先確認 `Login.exe` 自身有「記住帳密」功能。若有 → **什麼都不用做**，launcher 只負責按「開始」。 | 無 |
| B | UI Automation（`Win32_UI_Accessibility`）找 Edit 控制項 → `ValuePattern::SetValue` | `Login.exe` 是 CEF 混合體，UIA 對 CEF 支援有限 |
| C | `SetForegroundWindow` + `SendInput` 打字 + `SendInput` 按 Enter | 容易打錯視窗；必須先驗證前景視窗 title/class 是目標 |

**實作紀律**：

- 預設 `auto_login = true`，但**第一次執行時強制走「手動」**，把實際視窗階層 dump 出來（`EnumChildWindows` + `GetClassName` + `GetWindowText`）寫進 `launcher.log`，供後續 AI 判斷該用哪條路。
- 帳密儲存：Windows **DPAPI**（`CryptProtectData`，使用者範圍），密文放 `launcher.cred`。
  **絕不**明文寫 `launcher.json`，**絕不**寫入本筆記。
- 提供「清除已存帳密」按鈕。

> 若 §5.8 任何一條路都失敗，**降級為手動**，不阻塞交付。這是本階段唯一允許的「部分完成」。

### 5.9 `logging.rs`

- 檔案：exe 同目錄 `launcher.log`，`append` 模式，每行 `YYYY-MM-DD HH:MM:SS 訊息`。
- UI：`Arc<Mutex<VecDeque<LogLine>>>`，上限 5000 行，避免長時間執行吃記憶體。
- 廣播：`crossbeam_channel::Sender<LogLine>`，UI 每幀 `try_recv` 清空。
- **分級**：`Info` / `Warn` / `Error`，UI 用顏色區分（灰 / 黃 / 紅）。

### 5.10 `ui\`

版面（由上而下）：

```
┌─ RHD 大叔的回憶天堂 ──────────────────────── [–][□][×] ┐
│ 狀態列： ● 中繼運行中   ● 目標可連線   ● hosts 已封鎖   │
├───────────────────────────────────────────────────────┤
│ [伺服器]                                              │
│   目標 IP  [127.0.0.1      ]  Port [3000   ]          │
│   本機監聽 [2000, 29467    ]  (逗號分隔)               │
│   ☑ 封鎖官方清單 (kaixingongyi.com)                   │
│   ☐ 同時改寫 lineage.cfg                              │
├───────────────────────────────────────────────────────┤
│ [帳號]  ☑ 自動登入   [帳號____] [密碼****] [清除]      │
├───────────────────────────────────────────────────────┤
│  [ 一鍵進遊戲 ]  [ 只開中繼 ]  [ 停止 ]  [ 解除封鎖 ]  │
│  [ 開啟遊戲資料夾 ]  [ 匯出日誌 ]                      │
├───────────────────────────────────────────────────────┤
│ 日誌                                                  │
│  09:41:02 中繼啟動：127.0.0.1:2000,29467 -> 127.0.0.1:3000
│  09:41:02 目標 127.0.0.1:3000 可連線                  │
│  09:41:03 已封鎖網域：kaixingongyi.com                │
│  ...                                                  │
└───────────────────────────────────────────────────────┘
```

- 深色主題（`egui::Visuals::dark()` + 自訂 accent 色）。
- 中文字型注入在 `theme::install_fonts(ctx)`，`main.rs` 第一幀呼叫一次。
- 按鈕禁用邏輯：中繼執行中 → 「只開中繼」變灰；未執行 → 「停止」變灰。
- 關閉視窗若中繼仍在跑 → 詢問「要縮到系統匣還是結束？」（`minimize_to_tray` 預設 true）。

---

## 6. 關鍵流程

### 6.1 首次啟動（冷啟動，什麼都沒設定）

1. `main()` → `is_admin()` 為 false → `relaunch_elevated()` → `exit(0)`
2. 提權後的實例：讀 `launcher.json`（不存在 → 寫入 `Default`）
3. 掃描 `game_dir` 是否存在、`Login.exe` 是否在
4. 檢查 2000/29467 是否被佔用 → UI 狀態列標黃
5. 掃描 `lin.log` / `Login*.log` 是否可讀（**只讀不寫**，客戶端可能開著檔案）
6. 顯示主視窗，日誌印出診斷摘要（§6.4）

### 6.2 一鍵進遊戲（正常路徑）

```
[使用者按「一鍵進遊戲」]
   ├─ 驗證設定（IP 格式、Port 範圍、至少一個監聽埠）
   ├─ 目標可達性測試（TCP connect，timeout 2s）
   │    失敗 → 提示「請先確認 L1RHDserver\run.bat 已啟動」→ 詢問是否仍要繼續
   ├─ hosts 封鎖（若勾選且未生效）
   ├─ lineage.cfg 改寫（若勾選）
   ├─ 起 relay
   │    失敗 → 中止，UI 顯示哪個埠綁不上（含「被 VMware NAT 佔用」提示）
   ├─ CreateProcess(Login.exe, cwd=game_dir)
   ├─ 起 watchdog：監控 Login.exe 退出 + relay 連線數 + 客戶端進程出現
   ├─ （可選）等 Login 視窗 → 填帳密
   └─ 遊戲進程出現 → UI 切「遊戲運行中」，按鈕組切換為 [結束遊戲後停止中繼]
```

### 6.3 異常路徑

| 情況 | 行為 |
|---|---|
| 目標 `127.0.0.1:3000` 不可達 | 黃色警告 + 詢問是否繼續（**不硬擋**，使用者可能先開 launcher 再開伺服器） |
| 2000 綁不上 | 紅字 + 具體建議（檢查 VMware NAT / 檢查是否已有另一個 launcher 在跑） |
| `Login.exe` 找不到 | 紅字 + 開啟資料夾按鈕 |
| `Login.exe` 起了但 30 秒內沒有 relay 連線 | 提示「客戶端未嘗試連線，請確認 hosts 封鎖是否生效」 |
| hosts 寫入失敗 | 紅字「需要管理員權限」+ 提示重啟為管理者 |
| UAC 被取消 | 靜默退出，不留半成品（因為提權前沒做任何事） |

### 6.4 診斷摘要（啟動時印在日誌）

```
game_dir      = I:\RHD\L1RHD          [OK]
Login.exe     = 8,679,936 bytes        [OK]
lineage.cfg   = IP 127.0.0.1 / tag08 11702   [讀取成功，未修改]
hosts         = 未封鎖                 [可封鎖]
Port 2000     = 空閒                   [可綁定]
Port 29467    = 空閒                   [可綁定]
目標 3000     = 可連線                 [OK]
管理員        = 是                     [OK]
```

---

## 7. 資料結構速查

```rust
// 狀態機
#[derive(Clone, Copy, PartialEq)]
pub enum Stage { Idle, Starting, RelayUp, ClientUp, InGame, Error }

// UI ↔ runtime 訊息
pub enum Evt {
    Log { level: Lvl, msg: String },
    RelayBound { port: u16, addr: SocketAddr },
    RelayConn { seq: u64, peer: SocketAddr, port: u16 },
    RelayClosed { seq: u64 },
    ChildExit { pid: u32, code: u32 },
    StageChanged(Stage),
}
```

---

## 8. 實作階段與驗收

每一階段都必須**通過驗收才進下一階段**。狀態標記沿用 vault 規範：`OPEN` / `PARTIAL` / `PASS`。

### Stage 1：骨架 + 提權（0.5 天）

- [ ] `cargo new --bin rhd-launcher`，`Cargo.toml` 加 §4.2 依賴
- [ ] `build.rs` 嵌 manifest → 確認產物含 `requireAdministrator`
- [ ] `is_admin()` + `relaunch_elevated()` 可用
- [ ] 空白 egui 視窗能開、中文字型正確顯示「測試」二字不變豆腐
- **驗收**：雙擊 exe → 彈 UAC → 是 → 視窗出現且中文正常

### Stage 2：relay（1 天）

- [ ] `relay.rs` 完成 §5.3 七件事
- [ ] UI 三顆按鈕（只開中繼 / 停止 / 狀態燈）可用
- [ ] 日誌格式與 Python 版一致
- **驗收**：`Test-NetConnection 127.0.0.1 -Port 2000` 成功；對 3000 起一個 `ncat -l 3000`，從 2000 打入資料能收到；停止後埠釋放

### Stage 3：hosts + cfg（0.5 天）

- [ ] 封鎖 / 還原都可用，且 `hosts.rhd.bak` 正確產生
- [ ] `lineage.cfg` 能讀出 tag 0x07/0x08 並顯示（**先只讀不改**）
- **驗收**：`nslookup kaixingongyi.com` 回 127.0.0.1；還原後恢復正常；`lineage.cfg` 讀值與 §5.5 表格一致

### Stage 4：啟動鏈（1 天）

- [ ] 完整流程 §6.2 可跑
- [ ] 進程 watchdog 正確
- **驗收**：按「一鍵進遊戲」→ Login.exe 起 → 客戶端連入 2000 → 進到角色選單（**不要求自動登入**）

### Stage 5：自動登入（風險項，1~2 天或不排）

- [ ] 先做「視窗階層 dump」工具，記錄真實控制項
- [ ] 依 dump 結果選 §5.8 A/B/C
- [ ] DPAPI 加密儲存
- **驗收**：從零（無任何已存帳密）到進遊戲，全程只按一次按鈕

### 全域驗收（Definition of Done）

1. 目標機**沒有裝 Python**，只放 `RHDLauncher.exe` + `launcher.json` → 能一鍵進遊戲
2. 關閉 launcher → relay 停止 → 埠釋放（`Get-NetTCPConnection -LocalPort 2000` 為空）
3. 「解除 hosts 封鎖」後 `kaixingongyi.com` 能正常解析
4. 連續執行 10 次，無記憶體泄漏（工作集穩定）、無殘留進程
5. `launcher.log` 可完整回溯一次登入的每一步

---

## 9. 已知坑（踩過的，不要再踩）

### 9.1 環境類

| 坑 | 現象 | 對策 |
|---|---|---|
| `.bin` 沒有檔案關聯 | `Start-Process -Verb RunAs xxx.bin` 報「沒有任何應用程式與此操作的指定檔案有關聯」 | ShellExecute 需要副檔名關聯。要嘛 hardlink 改 `.exe`，要嘛在已提權環境用 `CreateProcess` |
| `CreateProcess` 不能提權 | `WinError 740` | UAC 提權只能走 ShellExecute；或先提權再 CreateProcess |
| `::1` 遮蔽 `127.0.0.1` | 綁了 `::1` 後 IPv4 連不進來 | **先** 設 `IPV6_V6ONLY=1` **再** bind |

### 9.2 判斷類（最容易誤判，真實發生過）

| 坑 | 說明 |
|---|---|
| **把殘留會話的畫面當成實驗成功** | 09:26 直接啟動 `Lin.bin` 後 `lin.log` 持續增長、場景正常切換，一度以為「獨立啟動能進遊戲」。實際上那是 **09:18 留下的 `LogonX64.bin`（pid 37576）仍在線**，日誌是它寫的。<br>**紀律**：任何「啟動成功」的結論，必須同時具備 ① 本次進程 pid 活著 ② 該 pid 有 TCP 連線 ③ 伺服器端有對應記錄。三者缺一不可。 |
| 把 `11702` 當成連線埠 | `lineage.cfg` tag 0x08 = 11702，但實測沒有任何監聽。日誌 `MainSocket Connect host : 127.0.0.1 (11702)` 的括號是 handle 不是埠。<br>**後續更正**：該值在 09:32 的啟動後變成 55398，證明它會漂移，**根本不是埠**（見 §5.5 觀察表）。 |
| TLV 起點算成 offset 30 | 首個 tag 讀出 `0x40000`、後續全歪。正確起點是 **40**（`0x26` magic + 4 + 6 + 4）。用特徵搜尋可完全避開。 |
| 誤讀 `lin.log` | 它是 XOR `0x4D`；XOR 後角色名段落仍是 Big5 位元組，不是 UTF-8，顯示成 `[????1]` 是正常的，**不代表亂碼 bug**。 |
| 把 `LogonX64.bin` 當成獨立程式 | 它跟 `Lin.bin` **MD5 相同**，是同一支程式的兩種模式，差別只在檔名。 |

### 9.3 若日後要挑戰「零 Login.exe」（🎉 2026-10-24 下午逆向重大突破）

> ⚠️ **最新突破（2026-10-24 15:30）**：
> 本節原列之三大前置障礙已在 [[20261024-天R客戶端BIN反編譯與主體脫殼]] §10.16～§10.20 **全數攻克並取得硬佐證**：
> 1. `3.37.249.199:80` 真身已確證為 BeanFun HTTPS 帳號驗證端點（`0x140073FA0`），直接由客戶端發送 JSON `{"Account": "%s", ...}`，脫離 `Login.exe` 自殺係因 `Account` 為空所致（§10.16）。
> 2. `Lin.bin` 脫殼後確認底層 `ws2_32.connect` 於 **`0x140A7A8C4`** 呼叫（`SocketMain@Lineage`），只要在該點修改 `rdx`（`sockaddr_in`）即可達成硬體級伺服器轉向，免除 TCP Relay（§10.18）。
> 3. S_KEY 金鑰交換（Opcode 15）、256-byte RC4 S-box（`0x145EAD030`）、4-byte 遞增封包序號與 8-byte CBC 滾動加解密，已與伺服器核心 `LineageEncryption.java` 達成 **100% 逐位元完全閉環**（§10.19～§10.20）。
> 
> 當前 RHDLauncher（Rust relay 版）已足夠穩定服務現狀；未來若欲進階至純自製無黑盒之「零 Login.exe」原生登入器，所有底層技術地基已完全就緒。

### 9.3 原定排查線路與對照歷史（已攻克）

先做這三件事，按順序，任何一步沒結果就不要往下：

1. **抓 `3.37.249.199:80` 的 HTTP 請求**。方法：路由把該 IP 導到本機（需要 loopback 虛擬網卡 + `route add`），或用 WinDivert 在 `connect` 前攔。目標是拿到 path / query / header / 期望的 response 格式。
2. **確認 `Login.exe` → 客戶端 的交接機制**。方法：正常啟動時用 Process Monitor 監控 `CreateProcess`、`CreateFileMapping`、`CreateNamedPipe`、`WriteFile`。特別注意 `Login.exe` 退出前那 1 秒。
3. **確認 `LinLogin.dll` 的 3 個匯出被誰、怎麼呼叫**。`TestFunction` 這個名字暗示它是驗證入口。

三件事都通了，才輪到「自己實作登入協議」。在那之前，本企畫的 §2.4 結論不變。

### 9.4 伺服器端配合事項

- `config\server.properties`：`GameserverHostname = *`、`GameserverPort = 3000`
- `config\sql.properties`：`jdbc:mysql://localhost/xxtt8`
- 客戶端要的是 `2000`／`29467`，伺服器卻開在 `3000` → **這就是 relay 存在的理由**
- 若目標與監聽同埠（例如都填 29467），relay 必須跳過該埠（§5.3 第 1 條）

---

## 10. 附錄

### A. 本次實測的原始證據位置

| 檔案 | 內容 |
|---|---|
| `I:\RHD\L1RHD\Logs\LogonX642026-10-24.log` | UTF-16LE，重定向日誌（§2.2） |
| `I:\RHD\L1RHD\Logs\Login2026-10-24.log` | UTF-16LE，`wait game Initialize` |
| `I:\RHD\L1RHD\lin.log` | XOR `0x4D`，客戶端詳細日誌 |
| `I:\RHD\L1RHD\bin64\debug.log` | CEF 日誌（僅 `cef_extensions.pak` 缺失警告，無 URL 資訊） |
| `I:\RHD\L1RHD\lineage.cfg` | TLV 設定檔，411 bytes |
| `I:\RHD\.re\cx\out\probe_realname.txt` | §2.3 實驗 C 完整輸出 |
| `I:\RHD\.re\cx\out\pe_info.txt` | 三個 PE 的節區與匯出表 |
| `I:\RHD\.re\cx\out\relay_run.log` | relay 連線時序 |

> 上述為**本機實體檔**，依 vault 規範不入庫。

### B. 檔案雜湊（用於判斷客戶端是否被換版）

```
Lin.bin   / LogonX64.bin : MD5 18328DB21B0A13C63F399AF80DC82A33   (18,989,568 bytes)
Lin_x64.bin              : 18,720,768 bytes
Login.exe (目錄與 zip 內): 8,679,936 bytes, TimeDateStamp 2020-11-14 06:00:24
LinLogin.dll             : 5,713,408 bytes, TimeDateStamp 2020-03-30 09:09:50
```

### C. Python → Rust 對照表（搬邏輯用）

| Python (`rhd_launcher.py`) | Rust (`src/`) | 注意 |
|---|---|---|
| `DEFAULT_CONFIG` (L69) | `config.rs::AppConfig::default()` | 欄位名保持一致 |
| `load_config` / `save_config` | `config.rs::load/save` | 舊檔要能直接讀 |
| `parse_ports` (L122) | `config.rs::parse_ports` | 去重、範圍 1..=65535 |
| `ip_to_bytes` (L139) | `cfgfile.rs::ip_to_bytes` | 驗證四段 0..=255 |
| `target_alive` (L146) | `launch.rs::target_alive` | timeout 2s |
| `hosts_path` / `block_domains` / `unblock_domains` (L159-267) | `hosts.rs` | **逐條對齊**，含 `\r\n` 處理 |
| `patch_lineage_cfg` / `read_lineage_cfg` (L273-324) | `cfgfile.rs` | 就地覆蓋，不重建檔 |
| `class Relay` (L330-491) | `relay.rs` | §5.3 七件事 |
| `class App` (L497-711) | `ui\mod.rs` | tkinter → egui |
| `on_start` (L626) | `ui::actions::start_all` | 流程 §6.2 |

### D. 待決事項（實作前需確認）

| # | 事項 | 影響 |
|---|---|---|
| D1 | `lineage.cfg` tag `0x08` 到底是不是連線埠 | 若不是，`rewrite_cfg` 就只是安慰劑 |
| D2 | `Login.exe` 是否有「記住帳密」 | 有 → §5.8 可整段跳過 |
| D3 | 使用者是否接受每次都要按 UAC | 想省掉就得買/簽憑證，或改用工作排程器 |
| D4 | 是否需要多伺服器設定檔（切換不同私服） | 需要就擴充 `AppConfig` 為 `Vec<ServerProfile>` |

---

## 11. 變更記錄

| 日期 | 變更 |
|---|---|
| 2026-10-24 | 初版。完成 §2 全部取證（三項否定實驗）、§3~§8 規格、§9 坑位、§10 對照表。狀態 OPEN，等待進入 Stage 1。 |
| 2026-10-24 | 二次校對 `lineage.cfg`：TLV 起點更正為 offset **40**；證實 `tag 0x08` 會隨每次啟動漂移（11702 → 55398），不是連線埠；補 §9.2 兩條坑位。 |
| 2026-10-31 | **現況改寫**：狀態 OPEN→SUPERSEDED。B4（中繼＋進程內八刀＋補發 S_KEY＋明文登入代理）已實作並兩次實機進世界；§1.2「不注入／不改記憶體」排除條款與 §2.4「不能取代 Login.exe」均已由 B1→B4 實機證據推翻（見文首 CAUTION）。 |
