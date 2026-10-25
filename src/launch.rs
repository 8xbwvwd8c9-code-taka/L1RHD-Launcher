//! 啟動 Login.exe 與進程監控 (企畫書 §5.7)
//!
//! launcher 自己已是管理者，Command::new 直接可用（不會出現 WinError 740）。
//! current_dir 必須設正確：lin.log / lineage.cfg / <角色名>.loc 都寫在遊戲根目錄。
//! Login.exe 退出不等於失敗（它本來就會在 game Initialize Complete! 後自己退出）。

use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

#[allow(dead_code)]
pub struct Launched {
    pub child: Child,
    pub pid: u32,
}

pub fn target_alive(host: &str, port: u16, timeout: Duration) -> bool {
    let addr = format!("{host}:{port}");
    let sock = match addr.to_socket_addrs() {
        Ok(mut it) => match it.next() {
            Some(a) => a,
            None => return false,
        },
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&sock, timeout).is_ok()
}

#[allow(dead_code)]
pub fn launch_login(game_dir: &str, exe_name: &str) -> Result<Launched, String> {
    let dir = Path::new(game_dir);
    let exe = dir.join(exe_name);
    if !exe.exists() {
        return Err(format!("找不到 {} —— 請確認遊戲路徑", exe.display()));
    }
    let child = Command::new(&exe)
        .current_dir(dir)
        .spawn()
        .map_err(|e| format!("啟動 {} 失敗：{e}", exe.display()))?;
    let pid = child.id();
    Ok(Launched { child, pid })
}

/// 轉換成人類可讀的結束描述
#[allow(dead_code)]
pub fn describe_exit(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("已退出（代碼 {c}）"),
        None => "已被終止".to_string(),
    }
}

/// 檢查客戶端進程是否存在（LogonX64.bin / Lin.bin）
pub fn client_process_running() -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut pe = PROCESSENTRY32W::default();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = None;
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &pe.szExeFile[..pe
                        .szExeFile
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(pe.szExeFile.len())],
                );
                let low = name.to_ascii_lowercase();
                if low.contains("logonx64") || low == "lin.bin" || low.starts_with("lin_") {
                    found = Some(format!("{name}(pid {})", pe.th32ProcessID));
                    break;
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}
