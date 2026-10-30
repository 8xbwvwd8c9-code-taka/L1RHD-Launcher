# TR-BUG-458 — 安全閘門 [已修復但未實機測試]

舊 B1 跨程序逐筆寫碼、原版登入器入口及強殺程序已停用。自製 DLL 僅在自製 fixture 驗證載入；真實遊戲同步點、登入與當機根因仍為 OPEN。

Rust 離線測試 5/5、原生 Debug／Release 各 4/4；相對中文路徑、篡改 fixture、舊入口編譯及 CLI 阻擋通過。完整證據見黑曜石 `BUG工作日誌/案件/TR-BUG-458/fix-codex-b1-safe-20261029.md`。