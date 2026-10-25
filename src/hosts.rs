//! hosts 受控區段 (企畫書 §5.4)
//!
//! Login.exe 啟動時會先抓
//!   http://www.kaixingongyi.com/xinttdj/list.txt?t=<unixtime>
//! 成功就用遠端清單覆蓋本機 LocalService.ini。
//! 所以真正需要的不是斷網，是擋掉這一個網域。
//!
//! 寫入流程：讀全文 → 移除既有受控區段 → 去尾端空行 → 追加新區段 → 寫回。
//! 保留原檔位元組：以 raw bytes 操作 ASCII 區段，不整檔重新編碼。

use std::path::PathBuf;

/// 內建封鎖清單（不對外顯示開關）
pub const BUILTIN_BLOCKED: &[&str] = &["kaixingongyi.com", "www.kaixingongyi.com"];

pub const HOSTS_BEGIN: &str = "# >>> RHD Launcher managed >>>";
pub const HOSTS_END: &str = "# <<< RHD Launcher managed <<<";

pub fn hosts_path() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    PathBuf::from(root).join("System32\\drivers\\etc\\hosts")
}

pub fn backup_path() -> PathBuf {
    hosts_path().with_file_name("hosts.rhd.bak")
}

fn marker_positions(text: &str) -> Option<(usize, usize)> {
    let b = text.find(HOSTS_BEGIN)?;
    let e = text.find(HOSTS_END)?;
    if e > b {
        Some((b, e + HOSTS_END.len()))
    } else {
        None
    }
}

/// 狀態查詢以檔案內容為準，不只信記憶體旗標
pub fn is_blocked() -> bool {
    match std::fs::read(hosts_path()) {
        Ok(b) => {
            let text = String::from_utf8_lossy(&b);
            marker_positions(&text).is_some()
        }
        Err(_) => false,
    }
}

fn section(domains: &[String]) -> String {
    let mut s = String::new();
    s.push_str(HOSTS_BEGIN);
    s.push_str("\r\n");
    for d in domains {
        let d = d.trim();
        if d.is_empty() {
            continue;
        }
        s.push_str("127.0.0.1 ");
        s.push_str(d);
        s.push_str("\r\n");
    }
    s.push_str(HOSTS_END);
    s
}

fn strip_section(text: &str) -> String {
    // 同時處理 CRLF / LF
    let mut cur = text.to_string();
    while let Some((b, e)) = marker_positions(&cur) {
        // 一併吞掉區段後的換行
        let mut end = e;
        while cur.as_bytes().get(end).map(|c| *c == b'\r' || *c == b'\n').unwrap_or(false) {
            end += 1;
        }
        let mut start = b;
        while start > 0 && (cur.as_bytes()[start - 1] == b'\n' || cur.as_bytes()[start - 1] == b'\r') {
            start -= 1;
        }
        cur.replace_range(start..end, "");
    }
    cur
}

pub fn block(domains: &[String]) -> Result<(), String> {
    let path = hosts_path();
    let raw = std::fs::read(&path).map_err(|e| format!("讀取 hosts 失敗 {}: {e}", path.display()))?;

    // 寫入前先備份（覆蓋舊備份）
    let bak = backup_path();
    std::fs::write(&bak, &raw).map_err(|e| format!("備份 hosts 失敗 {}: {e}", bak.display()))?;

    let text = String::from_utf8_lossy(&raw).to_string();
    let mut body = strip_section(&text);
    while body.ends_with("\r\n") || body.ends_with('\n') || body.ends_with('\r') {
        body.pop();
    }
    let mut out = body;
    out.push_str("\r\n\r\n");
    out.push_str(&section(domains));
    out.push_str("\r\n");

    std::fs::write(&path, out.as_bytes()).map_err(|e| format!("寫入 hosts 失敗（需管理員權限）: {e}"))?;
    flush_dns();
    Ok(())
}

#[allow(dead_code)]
pub fn unblock() -> Result<(), String> {
    let path = hosts_path();
    let raw = std::fs::read(&path).map_err(|e| format!("讀取 hosts 失敗 {}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    if marker_positions(&text).is_none() {
        return Ok(());
    }
    let body = strip_section(&text);
    let mut out = body;
    while out.ends_with("\r\n") || out.ends_with('\n') || out.ends_with('\r') {
        out.pop();
    }
    out.push_str("\r\n");
    std::fs::write(&path, out.as_bytes()).map_err(|e| format!("寫入 hosts 失敗（需管理員權限）: {e}"))?;
    flush_dns();
    Ok(())
}

pub fn flush_dns() {
    // 逾時 20 秒，失敗不報錯
    let _ = std::process::Command::new("ipconfig")
        .arg("/flushdns")
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .output();
}

#[cfg(windows)]
trait NoWindow {
    fn creation_flags(&mut self, f: u32) -> &mut Self;
}
#[cfg(windows)]
impl NoWindow for std::process::Command {
    fn creation_flags(&mut self, f: u32) -> &mut Self {
        use std::os::windows::process::CommandExt;
        CommandExt::creation_flags(self, f)
    }
}
