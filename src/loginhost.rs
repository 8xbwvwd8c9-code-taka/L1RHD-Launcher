//! 原版 Login.exe 背景接管
//!
//! 為什麼不能直接啟動 Lin.bin（2026-10-24 實測）：
//!   直接 CreateProcess(bin64\Lin.bin) → 客戶端連 AWS 3.37.249.199:80 驗證，
//!   驗證失敗彈出「GameGuard execute error : 380 / Fail to connect the GameGuard update server」。
//!   也就是客戶端的初始化契約綁在 Login.exe 上（企畫書 §2.4 已列為核心決策）。
//!
//! 因此本模組的定位是「把 Login.exe 關在使用者看不到的地方」：
//!   1. 用 CREATE_NO_WINDOW 啟動，不給它獨立主控台
//!   2. 啟動後在背景執行緒持續尋找它的視窗並 SW_HIDE，直到它自己交接完退出
//!   3. 使用者只會看到我們的登入器介面，看不到原版登入器畫面

use crate::logging::LogHandle;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct HiddenLogin {
    pub child: Child,
    pub pid: u32,
    stop: Arc<AtomicBool>,
}

impl HiddenLogin {
    pub fn stop_hiding(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 啟動 Login.exe 並在背景隱藏它的視窗。回傳控制代。
pub fn launch_hidden(game_dir: &str, exe_name: &str, log: LogHandle) -> Result<HiddenLogin, String> {
    let dir = Path::new(game_dir);
    let exe = dir.join(exe_name);
    if !exe.exists() {
        return Err(format!(
            "找不到 {} —— 請確認遊戲路徑",
            exe.display()
        ));
    }

    let child = Command::new(&exe)
        .current_dir(dir)
        .creation_flags()
        .spawn()
        .map_err(|e| format!("啟動 {} 失敗：{e}", exe.display()))?;

    let pid = child.id();
    let stop = Arc::new(AtomicBool::new(false));
    // 注意：不隱藏 Login.exe。它必須讓使用者看得到才能點「開始遊戲」，
    // 隱藏後玩家無法操作，且父進程結束時會連帶終止。
    log.info(format!("Login.exe 已啟動 pid {pid}（保留視窗供操作）"));

    Ok(HiddenLogin { child, pid, stop })
}

fn hide_loop(pid: u32, stop: Arc<AtomicBool>, log: LogHandle) {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, ShowWindow,
        SW_HIDE,
    };

    struct Ctx {
        pid: u32,
        hidden: usize,
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        let mut wpid = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut wpid));
        if wpid != ctx.pid {
            return true.into();
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return true.into();
        }
        // 只隱藏主視窗（標題非空的），避免誤隱藏 tooltip
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        if n == 0 {
            return true.into();
        }
        let _ = ShowWindow(hwnd, SW_HIDE);
        ctx.hidden += 1;
        true.into()
    }

    let mut reported = 0usize;
    for _ in 0..1200 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut ctx = Ctx { pid, hidden: 0 };
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut ctx as *mut Ctx as isize));
        }
        if ctx.hidden > 0 && reported == 0 {
            log.info(format!("已隱藏原版登入器視窗（pid {pid}）"));
            reported = ctx.hidden;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

trait NoWindow {
    fn creation_flags(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    fn creation_flags(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        CommandExt::creation_flags(self, CREATE_NO_WINDOW)
    }
}