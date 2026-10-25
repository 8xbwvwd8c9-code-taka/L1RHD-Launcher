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
