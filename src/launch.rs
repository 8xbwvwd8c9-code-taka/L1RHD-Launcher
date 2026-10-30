//! 伺服器連線狀態與遊戲程序檢查。
//!
//! 程序清單檢查失敗必須回傳錯誤，不得視為沒有正在運行的遊戲。

use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
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

// FIX: TR-BUG-567 [已修復但未實機測試] 同名程序須再核對設定遊戲目錄的映像路徑。
fn normalized_windows_path(path: &Path) -> Option<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut path = path.to_string_lossy().replace('/', "\\");
    if let Some(path_without_prefix) = path.strip_prefix(r"\\?\") {
        path = path_without_prefix.to_string();
    }
    while path.len() > 3 && path.ends_with('\\') {
        path.pop();
    }
    Some(path.to_ascii_lowercase())
}

fn is_client_image_path(image_path: &Path, game_dir: &Path) -> bool {
    let Some(file_name) = image_path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let file_name = file_name.to_ascii_lowercase();
    if file_name != "lin.bin" && file_name != "logonx64.bin" && !file_name.starts_with("lin_") {
        return false;
    }
    let Some(image_dir) = image_path.parent().and_then(normalized_windows_path) else {
        return false;
    };
    let Some(game_bin_dir) = normalized_windows_path(&game_dir.join("bin64")) else {
        return false;
    };
    image_dir == game_bin_dir
}

/// 檢查指定遊戲目錄的客戶端程序是否存在。
pub fn client_process_running(game_dir: &str) -> Result<Option<String>, String> {
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE,
    };
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
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
                if low == "logonx64.bin" || low == "lin.bin" || low.starts_with("lin_") {
                    let process = match OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        false,
                        pe.th32ProcessID,
                    ) {
                        Ok(process) => Some(process),
                        Err(e)
                            if e.code()
                                == windows::core::HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) =>
                        {
                            None
                        }
                        Err(e) => {
                            return Err(format!("開啟候選遊戲程序失敗（{name}, pid {}）：{e}", pe.th32ProcessID));
                        }
                    };
                    if let Some(process) = process {
                        let image_path = (|| {
                            let mut buffer = vec![0u16; 32768];
                            let mut size = buffer.len() as u32;
                            QueryFullProcessImageNameW(
                                process,
                                PROCESS_NAME_WIN32,
                                windows::core::PWSTR(buffer.as_mut_ptr()),
                                &mut size,
                            )
                            .map_err(|e| format!("讀取候選遊戲程序路徑失敗（{name}, pid {}）：{e}", pe.th32ProcessID))?;
                            Ok::<_, String>(PathBuf::from(String::from_utf16_lossy(
                                &buffer[..size as usize],
                            )))
                        })();
                        let _ = CloseHandle(process);
                        let image_path = image_path?;
                        if is_client_image_path(&image_path, Path::new(game_dir)) {
                            return Ok(Some(format!("{}(pid {})", image_path.display(), pe.th32ProcessID)));
                        }
                    }
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
    fn matches_client_image_only_under_configured_game_bin64() {
        let game_dir = std::path::Path::new(r"C:\Games\Lineage");
        assert!(is_client_image_path(
            std::path::Path::new(r"C:\Games\Lineage\bin64\Lin.bin"),
            game_dir
        ));
        assert!(is_client_image_path(
            std::path::Path::new(r"c:/games/lineage/BIN64/LogonX64.bin"),
            game_dir
        ));
        assert!(!is_client_image_path(
            std::path::Path::new(r"C:\Temp\Lin.bin"),
            game_dir
        ));
        assert!(!is_client_image_path(
            std::path::Path::new(r"C:\Games\Lineage\bin64-backup\Lin.bin"),
            game_dir
        ));
        assert!(!is_client_image_path(
            std::path::Path::new(r"C:\Games\Lineage\bin64\not-lin.exe"),
            game_dir
        ));
    }

    #[test]
    fn fake_client_waiter() {
        if std::env::var_os("RHD_FAKE_CLIENT_TEST").is_some() {
            let mut byte = [0];
            let _ = std::io::stdin().read(&mut byte);
        }
    }

    #[test]
    fn recognizes_existing_client_without_terminating_it() {
        let dir = std::env::temp_dir().join(format!(
            "rhd-client-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let bin_dir = dir.join("bin64");
        std::fs::create_dir(&bin_dir).unwrap();
        let exe = bin_dir.join("Lin.bin");
        let unrelated_game = dir.join("other-game");
        std::fs::create_dir_all(unrelated_game.join("bin64")).unwrap();
        std::fs::copy(std::env::current_exe().unwrap(), unrelated_game.join("bin64").join("Lin.bin"))
            .unwrap();
        struct Fixture(Option<Child>, std::path::PathBuf, std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                if let Some(child) = self.0.as_mut() {
                    drop(child.stdin.take());
                    let _ = child.wait();
                }
                let _ = std::fs::remove_file(&self.1);
                let _ = std::fs::remove_dir_all(&self.2);
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
            let found = client_process_running(fixture.2.to_str().unwrap()).unwrap();
            if found.as_deref().is_some_and(|s| s.contains(&format!("pid {}", child.id()))) {
                assert!(child.try_wait().unwrap().is_none());
                assert!(client_process_running(unrelated_game.to_str().unwrap())
                    .unwrap()
                    .is_none());
                break;
            }
            assert!(std::time::Instant::now() < deadline, "fixture process not detected");
            assert!(child.try_wait().unwrap().is_none(), "fixture exited unexpectedly");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
