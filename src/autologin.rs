//! 自動登入輔助 (企畫書 §5.8，風險最高，最後做)
//!
//! 本階段先實作「視窗階層 dump」；自動填表待 dump 結果再選 A/B/C 路徑。
//! 帳密一律 DPAPI 加密，絕不明文落盤。

use windows::Win32::Foundation::{HWND, LPARAM};

unsafe fn win_text(hwnd: HWND, which: u8) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowTextW};
    let mut buf = [0u16; 512];
    let n = if which == 0 {
        GetWindowTextW(hwnd, &mut buf)
    } else {
        GetClassNameW(hwnd, &mut buf)
    };
    let n = (n.max(0) as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

struct Ctx {
    filter: String,
    out: Vec<String>,
}

unsafe extern "system" fn child_proc(chwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let ctx = &mut *(lparam.0 as *mut Ctx);
    let class = win_text(chwnd, 1);
    let text = win_text(chwnd, 0);
    ctx.out
        .push(format!("   [CHILD] hwnd={:?} class={} text={}", chwnd.0, class, text));
    true.into()
}

unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    use windows::Win32::UI::WindowsAndMessaging::{EnumChildWindows, IsWindowVisible};
    let ctx = &mut *(lparam.0 as *mut Ctx);
    if !IsWindowVisible(hwnd).as_bool() {
        return true.into();
    }
    let title = win_text(hwnd, 0);
    if title.is_empty() {
        return true.into();
    }
    if !ctx.filter.is_empty() && !title.contains(ctx.filter.as_str()) {
        return true.into();
    }
    let class = win_text(hwnd, 1);
    ctx.out
        .push(format!("[TOP] hwnd={:?} class={} title={}", hwnd.0, class, title));
    let _ = EnumChildWindows(Some(hwnd), Some(child_proc), lparam);
    true.into()
}

/// 列出可見視窗（可依標題過濾），並 dump 子控制項類別與文字。
pub fn dump_windows(filter: &str) -> Vec<String> {
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
    let mut ctx = Ctx {
        filter: filter.to_string(),
        out: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.out
}

// ---------- DPAPI 密碼儲存 ----------

fn cred_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("."));
    exe.parent()
        .unwrap_or(std::path::Path::new("."))
        .join("launcher.cred")
}

pub fn save_cred(user: &str, pass: &str) -> Result<(), String> {
    let plain = format!("{user}\u{1}{pass}");
    let enc = dpapi_protect(plain.as_bytes())?;
    std::fs::write(cred_path(), enc).map_err(|e| format!("寫入憑據失敗：{e}"))
}

pub fn load_cred() -> Option<(String, String)> {
    let enc = std::fs::read(cred_path()).ok()?;
    let plain = dpapi_unprotect(&enc).ok()?;
    let s = String::from_utf8(plain).ok()?;
    let mut it = s.splitn(2, '\u{1}');
    Some((it.next()?.to_string(), it.next().unwrap_or("").to_string()))
}

pub fn clear_cred() -> std::io::Result<()> {
    match std::fs::remove_file(cred_path()) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn dpapi_protect(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("DPAPI 尚未實作（Stage 5）".to_string())
}

fn dpapi_unprotect(_data: &[u8]) -> Result<Vec<u8>, String> {
    Err("DPAPI 尚未實作（Stage 5）".to_string())
}
