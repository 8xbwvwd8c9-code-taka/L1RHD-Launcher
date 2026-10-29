//! 提權檢查與自我重啟 (企畫書 §4.4 / §5.6)

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// 是否已具備管理者權限。
pub fn is_admin() -> bool {
    // windows crate 0.61: IsUserAnAdmin 位於 Win32::UI::Shell
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

/// 以 runas 重新啟動自己。Ok(true) = 已送出，本進程應立即 exit(0)。
pub fn relaunch_elevated() -> std::io::Result<bool> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let exe = std::env::current_exe()?;
    let exe_w = wide(&exe.to_string_lossy());
    let verb_w = wide("runas");
    let cwd = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let cwd_w = wide(&cwd.to_string_lossy());

    let ret = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb_w.as_ptr()),
            PCWSTR(exe_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR(cwd_w.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // 回傳值 > 32 才算成功；1223 = 使用者取消 UAC
    let code = ret.0 as isize;
    if code > 32 {
        Ok(true)
    } else {
        Err(std::io::Error::from_raw_os_error(code as i32))
    }
}

/// 可讀的 UAC 取消說明
pub fn uac_cancelled(err: &std::io::Error) -> bool {
    err.raw_os_error() == Some(1223)
}

/// 靜默提權排程任務名（客戶端 Lin.bin 內嵌 requireAdministrator，
/// 補丁寫入也需提權 → 全鏈必須提權；用任務做到「僅第一次一次 UAC，之後零 UAC」）
pub const ELEV_TASK: &str = "RHDLauncherElevated";

fn schtasks(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("schtasks")
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 任務是否存在
pub fn task_exists() -> bool {
    schtasks(&["/Query", "/TN", ELEV_TASK])
}

/// 嘗試以排程任務靜默提權啟動自己（零 UAC）。
/// 回傳 true = 任務已觸發，本進程應立即 exit(0)。
pub fn try_task_elevation() -> bool {
    schtasks(&["/Run", "/TN", ELEV_TASK])
}

/// （須已提權）建立／更新指向當前 exe 的最高權限任務。
/// 呼叫時機：提權實例啟動後補建，讓下一次啟動走零 UAC 路徑。
pub fn create_elevation_task() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let tr = format!("\"{}\"", exe.display());
    schtasks(&[
        "/Create", "/F", "/TN", ELEV_TASK, "/TR", &tr, "/SC", "ONCE", "/ST", "00:00", "/RL",
        "HIGHEST",
    ])
}
