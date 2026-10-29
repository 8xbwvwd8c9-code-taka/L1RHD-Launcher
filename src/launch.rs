//! 伺服器連線狀態與遊戲程序檢查。
//!
//! 程序清單檢查失敗必須回傳錯誤，不得視為沒有正在運行的遊戲。

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

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

/// 轉換成人類可讀的結束描述
#[allow(dead_code)]
pub fn describe_exit(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("已退出（代碼 {c}）"),
        None => "已被終止".to_string(),
    }
}

/// 檢查客戶端進程是否存在（LogonX64.bin / Lin.bin）
pub fn client_process_running() -> Result<Option<String>, String> {
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| format!("建立程序快照失敗：{e}"))?;
        if snap == INVALID_HANDLE_VALUE {
            return Err("建立程序快照失敗：無效控制代碼".into());
        }
        let mut pe = PROCESSENTRY32W::default();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let result = (|| {
            Process32FirstW(snap, &mut pe)
                .map_err(|e| format!("讀取程序快照失敗：{e}"))?;
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
                    return Ok(Some(format!("{name}(pid {})", pe.th32ProcessID)));
                }
                if let Err(e) = Process32NextW(snap, &mut pe) {
                    return if e.code() == windows::core::HRESULT::from_win32(ERROR_NO_MORE_FILES.0) {
                        Ok(None)
                    } else {
                        Err(format!("列舉程序快照失敗：{e}"))
                    };
                }
            }
        })();
        let _ = CloseHandle(snap);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::process::{Child, Command, Stdio};

    #[test]
    fn fake_client_waiter() {
        if std::env::var_os("RHD_FAKE_CLIENT_TEST").is_some() {
            let mut byte = [0];
            let _ = std::io::stdin().read(&mut byte);
        }
    }

    #[test]
    fn recognizes_existing_client_without_terminating_it() {
        if client_process_running().unwrap().is_some() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "rhd-client-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let exe = dir.join("Lin.bin");
        struct Fixture(Option<Child>, std::path::PathBuf, std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Some(child) = self.0.as_mut() {
                    drop(child.stdin.take());
                    let _ = child.wait();
                }
                let _ = std::fs::remove_file(&self.1);
                let _ = std::fs::remove_dir(&self.2);
            }
        }
        let mut fixture = Fixture(None, exe, dir);
        std::fs::copy(std::env::current_exe().unwrap(), &fixture.1).unwrap();
        let child = Command::new(&fixture.1)
            .args(["--exact", "launch::tests::fake_client_waiter", "--nocapture"])
            .env("RHD_FAKE_CLIENT_TEST", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        fixture.0 = Some(child);
        let child = fixture.0.as_mut().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let found = client_process_running().unwrap();
            if found.as_deref().is_some_and(|s| s.contains(&format!("pid {}", child.id()))) {
                assert!(child.try_wait().unwrap().is_none());
                break;
            }
            assert!(std::time::Instant::now() < deadline, "fixture process not detected");
            assert!(child.try_wait().unwrap().is_none(), "fixture exited unexpectedly");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
