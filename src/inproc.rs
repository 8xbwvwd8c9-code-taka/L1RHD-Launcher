//! 進程內補丁啟動路線（TR-BUG-458 路線 B2）。
//!
//! 與被禁用的舊 B1（src/patches.rs：跨程序 WriteProcessMemory 逐刀改寫客戶端程式碼）
//! 的分工差異：
//!   - 啟動器對客戶端唯一的一次跨程序寫入，是注入機制本身的「DLL 路徑字串」；
//!   - 八刀補丁由自製 `rhd_inproc.dll` 在客戶端進程內完成（與原版 LinLogin.dll 同位格），
//!     寫入時機照原廠契約：等版本欄出現 2002122601（脫殼完成指標）才動手；
//!   - DLL 以「pid 衍生」具名事件回報結果（Local\RHDInprocDone-<pid> / Fail-<pid>），
//!     本模組在取得 pid 後建立同名事件對接。
//!
//! 放行主執行緒前失敗會終止並等待新客戶端；放行後的補丁失敗則由使用者自行關閉。

use crate::logging::LogHandle;
use crate::servers::ServerProfile;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Login.ini 內容（客戶端以明文 Login.ini 覆蓋官方加密 Lineage.ini 的既有機制）。
/// 連線目標固定本機中繼：ip=127.0.0.1，port=中繼第一個 listen 埠。
pub fn login_ini_text(listen_port: u16) -> String {
    format!(
        "[Server]\r\nip=127.0.0.1\r\nport={listen_port}\r\n\r\n[Options]\r\nRememberUser=1\r\nLanguage=1\r\n"
    )
}

/// 事件名稱（必須與 native/inproc.cpp 的 swprintf 格式逐字一致）。
pub fn event_names(pid: u32) -> (String, String) {
    (
        format!("Local\\RHDInprocDone-{pid}"),
        format!("Local\\RHDInprocFail-{pid}"),
    )
}

/// rhd_inproc.dll 的搜尋順序：登入器所在目錄 → 遊戲根目錄。
pub fn find_dll(game_dir: &str) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(dir) = crate::servers::exe_dir().parent().map(|p| p.to_path_buf()) {
        candidates.push(dir.join("rhd_inproc.dll"));
    }
    candidates.push(crate::servers::exe_dir().join("rhd_inproc.dll"));
    candidates.push(Path::new(game_dir).join("rhd_inproc.dll"));
    for c in &candidates {
        if c.is_file() {
            return Ok(c.clone());
        }
    }
    Err(format!(
        "找不到 rhd_inproc.dll（搜尋過：{}）",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("、")
    ))
}

/// DLL 回傳碼（與 native/inproc_contract.h 對應）。
const RC_OK: u32 = 0;
const RC_ALREADY_PATCHED: u32 = 0x200;
const RC_NO_MODULE: u32 = 1;
const RC_TIMEOUT: u32 = 0x101;
const RC_PREIMAGE: u32 = 0x110;
const RC_PROTECT: u32 = 0x120;
const RC_WRITE: u32 = 0x130;
const RC_UNSAFE: u32 = 0x140;

fn describe_rc(rc: u32) -> String {
    match rc {
        RC_OK => "八刀全部寫入覆核完成".into(),
        RC_ALREADY_PATCHED => "版本已是 2002122605 且站點一致（冪等）".into(),
        RC_NO_MODULE => "DLL 內拿不到客戶端主模組基址".into(),
        RC_TIMEOUT => "等待解殼指標逾時（SecureEngine 未在 120 秒內寫出 01）".into(),
        RC_PREIMAGE => "原值預檢失敗：客戶端狀態與八刀原值表不符，已零寫入拒絕".into(),
        RC_PROTECT => "頁面保護提升失敗，已零寫入拒絕".into(),
        RC_WRITE => "寫入覆核失敗，已回滾".into(),
        RC_UNSAFE => "頁保護還原失敗：不安全狀態，請回報 log".into(),
        other => format!("未知結果碼 0x{other:X}"),
    }
}

fn remote_module_base(
    hprocess: windows::Win32::Foundation::HANDLE,
    want: &str,
) -> Option<usize> {
    use windows::Win32::System::ProcessStatus::{
        EnumProcessModulesEx, GetModuleBaseNameW, LIST_MODULES_ALL,
    };
    let mut mods = [0isize; 1024];
    let mut needed = 0u32;
    unsafe {
        EnumProcessModulesEx(
            hprocess,
            mods.as_mut_ptr() as *mut _,
            (mods.len() * std::mem::size_of::<isize>()) as u32,
            &mut needed,
            LIST_MODULES_ALL,
        )
        .ok()?;
    }
    let count = (needed as usize / std::mem::size_of::<isize>()).min(mods.len());
    for m in mods.iter().take(count) {
        let mut buf = [0u16; 260];
        let n = unsafe {
            GetModuleBaseNameW(
                hprocess,
                Some(windows::Win32::Foundation::HMODULE(*m as *mut c_void)),
                &mut buf,
            )
        };
        if n == 0 {
            continue;
        }
        let name = String::from_utf16_lossy(&buf[..n as usize]);
        if name.eq_ignore_ascii_case(want) {
            return Some(*m as usize);
        }
    }
    None
}

/// 完整啟動鏈：Login.ini → CREATE_SUSPENDED 啟動客戶端 → 注入 rhd_inproc.dll
/// → ResumeThread → 建立結果事件 → CreateRemoteThread(RhdPatchStart) → 等結果。
pub fn launch_inproc(profile: &ServerProfile, log: LogHandle) -> Result<(), String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, GetModuleHandleW, LoadLibraryW,
    };
    use windows::Win32::System::Memory::{
        VirtualAllocEx, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, CreateProcessW, CreateRemoteThread, GetExitCodeProcess,
        ResumeThread, WaitForSingleObject, LPTHREAD_START_ROUTINE, CREATE_SUSPENDED,
        STARTUPINFOW, PROCESS_INFORMATION,
    };
    use windows::Win32::Foundation::WAIT_OBJECT_0;

    // 1. 前置檢查：客戶端不得已在執行；檢查失敗不得視為「沒有遊戲」
    match crate::launch::client_process_running(&profile.game_dir) {
        Ok(Some(name)) => return Err(format!("偵測到既有遊戲 {name}，請先自行關閉")),
        Ok(None) => {}
        Err(e) => return Err(format!("無法檢查遊戲程序：{e}")),
    }

    let root = PathBuf::from(&profile.game_dir);
    let exe = root.join("bin64").join("Lin.bin");
    if !exe.is_file() {
        return Err(format!("找不到客戶端 {}", exe.display()));
    }
    let dll = find_dll(&profile.game_dir)?;

    // 2. Login.ini（連線目標＝本機中繼）
    let listen_port = profile.listen_ports.first().copied().unwrap_or(2000);
    let ini = root.join("Login.ini");
    std::fs::write(&ini, login_ini_text(listen_port))
        .map_err(|e| format!("寫入 {} 失敗：{e}", ini.display()))?;
    log.info(format!("Login.ini 已寫入（127.0.0.1:{listen_port} → 中繼）"));

    // 3. CREATE_SUSPENDED 啟動客戶端（工作目錄＝遊戲根目錄）
    let exe_w = wide(&exe.to_string_lossy());
    let cwd_w = wide(&profile.game_dir);
    let mut si = STARTUPINFOW::default();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR(exe_w.as_ptr()),
            None,
            None,
            None,
            false,
            CREATE_SUSPENDED,
            None,
            PCWSTR(cwd_w.as_ptr()),
            &si,
            &mut pi,
        )
        .map_err(|e| format!("CreateProcess 失敗：{e}"))?;
    }
    let pid = pi.dwProcessId;
    log.info(format!("客戶端已建立 pid {pid}（掛起）"));

    // 4. 注入 rhd_inproc.dll（跨程序寫入僅 DLL 路徑字串）
    let inject_result: Result<(), String> = (|| {
        let dll_w = wide(&dll.to_string_lossy());
        let bytes = dll_w.len() * 2;
        unsafe {
            let remote = VirtualAllocEx(
                pi.hProcess,
                None,
                bytes,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            );
            if remote.is_null() {
                return Err(format!("VirtualAllocEx 失敗 err={}", GetLastError()));
            }
            let mut written = 0usize;
            WriteProcessMemory(
                pi.hProcess,
                remote,
                dll_w.as_ptr() as *const c_void,
                bytes,
                Some(&mut written),
            )
            .map_err(|e| format!("WriteProcessMemory(路徑) 失敗：{e}"))?;

            let k32 = GetModuleHandleW(w!("kernel32.dll"))
                .map_err(|e| format!("GetModuleHandleW：{e}"))?;
            let load = GetProcAddress(k32, windows::core::s!("LoadLibraryW"))
                .ok_or("找不到 LoadLibraryW")?;
            let lp: LPTHREAD_START_ROUTINE = Some(std::mem::transmute(load));
            let th = CreateRemoteThread(pi.hProcess, None, 0, lp, Some(remote), 0, None)
                .map_err(|e| format!("CreateRemoteThread(LoadLibraryW) 失敗：{e}"))?;
            let wait = WaitForSingleObject(th, 15000);
            let mut exit_code = 0u32;
            let _ = windows::Win32::System::Threading::GetExitCodeThread(th, &mut exit_code);
            let _ = CloseHandle(th);
            if wait != WAIT_OBJECT_0 || exit_code == 0 {
                return Err("遠端 LoadLibraryW 失敗（rhd_inproc.dll 載入被拒）".into());
            }
        }
        Ok(())
    })();
    if let Err(e) = inject_result {
        let cleanup = terminate_suspended_process(pi.hProcess);
        let _ = unsafe { CloseHandle(pi.hThread) };
        let _ = unsafe { CloseHandle(pi.hProcess) };
        return Err(match cleanup {
            Ok(()) => e,
            Err(cleanup_error) => format!("{e}；清理掛起客戶端失敗：{cleanup_error}"),
        });
    }
    log.info("rhd_inproc.dll 已注入");

    // 5. 放行主執行緒（SecureEngine 開始脫殼）
    let resumed = unsafe { ResumeThread(pi.hThread) };
    if resumed == u32::MAX {
        let cleanup = terminate_suspended_process(pi.hProcess);
        let _ = unsafe { CloseHandle(pi.hThread) };
        let _ = unsafe { CloseHandle(pi.hProcess) };
        return Err(match cleanup {
            Ok(()) => "ResumeThread 失敗".into(),
            Err(cleanup_error) => format!("ResumeThread 失敗；清理掛起客戶端失敗：{cleanup_error}"),
        });
    }

    // 6. 建立結果事件（DLL 以 pid 衍生名稱開啟並觸發）。
    //    具名核心物件隨最後一個 handle 消失：這裡必須**持有** handle 到補丁流程結束，
    //    立刻 CloseHandle 會讓 DLL 端 OpenEventW 回 err=2（2026-10-30 實測）。
    let (done_name, fail_name) = event_names(pid);
    let mut event_handles: Vec<windows::Win32::Foundation::HANDLE> = Vec::new();
    unsafe {
        for name in [&done_name, &fail_name] {
            let w = wide(name);
            match CreateEventW(None, true, false, PCWSTR(w.as_ptr())) {
                Ok(h) => event_handles.push(h),
                Err(e) => {
                    for h in &event_handles {
                        let _ = CloseHandle(*h);
                    }
                    let _ = CloseHandle(pi.hThread);
                    let _ = CloseHandle(pi.hProcess);
                    return Err(format!("CreateEventW({name}) 失敗：{e}"));
                }
            }
        }
    }

    // 7. 遠端呼叫 RhdPatchStart（DLL 自己等脫殼指標；逾時 120s）
    let patch_result: Result<u32, String> = (|| {
        let dll_w = wide(&dll.to_string_lossy());
        let local = unsafe { LoadLibraryW(PCWSTR(dll_w.as_ptr())) }
            .map_err(|e| format!("本機 LoadLibraryW(rhd_inproc.dll) 失敗：{e}"))?;
        let start = unsafe { GetProcAddress(local, windows::core::s!("RhdPatchStart")) }
            .ok_or("rhd_inproc.dll 缺少 RhdPatchStart 匯出")?;
        let rva = (start as usize) - (local.0 as usize);

        let mut remote_base = None;
        for _ in 0..10 {
            if let Some(b) = remote_module_base(pi.hProcess, "rhd_inproc.dll") {
                remote_base = Some(b);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let remote_base = remote_base.ok_or("客戶端模組清單找不到 rhd_inproc.dll")?;
        let remote_start = (remote_base + rva) as isize;
        log.info(format!(
            "RhdPatchStart 遠端位址 0x{remote_start:X}（RVA 0x{rva:X}）"
        ));

        let lp: LPTHREAD_START_ROUTINE = Some(unsafe { std::mem::transmute(remote_start) });
        let th = unsafe {
            CreateRemoteThread(pi.hProcess, None, 0, lp, None, 0, None)
                .map_err(|e| format!("CreateRemoteThread(RhdPatchStart) 失敗：{e}"))
        }?;
        // 心跳式等待：每 10 秒回報一次進度，凍結後重建時可把凍結窗口
        // 收斂到 10 秒級（2026-10-31 採證筆記 §3.1-4）。逾時語意不變：130 秒。
        let wait = {
            let t0 = std::time::Instant::now();
            let deadline = std::time::Duration::from_secs(130);
            loop {
                let w = unsafe { WaitForSingleObject(th, 10000) };
                if w == WAIT_OBJECT_0 || t0.elapsed() >= deadline {
                    break w;
                }
                log.info(format!(
                    "等待客戶端脫殼與補丁完成… t={}s",
                    t0.elapsed().as_secs()
                ));
            }
        };
        let mut rc = 0u32;
        unsafe {
            let _ = windows::Win32::System::Threading::GetExitCodeThread(th, &mut rc);
            let _ = CloseHandle(th);
        }
        if wait != WAIT_OBJECT_0 {
            return Err("RhdPatchStart 執行逾時（130 秒）".into());
        }
        Ok(rc)
    })();

    let _ = unsafe { CloseHandle(pi.hThread) };
    let _ = unsafe { CloseHandle(pi.hProcess) };
    for h in event_handles {
        let _ = unsafe { CloseHandle(h) };
    }

    match patch_result {
        Ok(rc @ (RC_OK | RC_ALREADY_PATCHED)) => {
            log.info(format!(
                "進程內補丁完成（pid {pid}，rc=0x{rc:X}：{}）",
                describe_rc(rc)
            ));
            Ok(())
        }
        Ok(rc) => Err(format!(
            "進程內補丁失敗（rc=0x{rc:X}：{}）；客戶端未終止，請自行關閉後再試",
            describe_rc(rc)
        )),
        Err(e) => Err(e),
    }
}

// FIX: TR-BUG-559 放行前失敗終止並等待新客戶端 - [已修復但未實機測試]
fn terminate_suspended_process(process: windows::Win32::Foundation::HANDLE) -> Result<(), String> {
    use windows::Win32::Foundation::WAIT_OBJECT_0;
    use windows::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};

    unsafe {
        TerminateProcess(process, 1).map_err(|e| format!("TerminateProcess：{e}"))?;
    }
    if unsafe { WaitForSingleObject(process, 5000) } != WAIT_OBJECT_0 {
        return Err("等待子程序退出逾時或失敗".into());
    }
    Ok(())
}

fn GetLastError() -> u32 {
    // windows crate 0.61 的錯誤多以 Result 呈現；此處僅供 VirtualAllocEx 這類
    // 以指標回報失敗的 API 取最後錯誤碼。
    unsafe { windows::Win32::Foundation::GetLastError().0 }
}

/// 終止「同名但非本行程」的登入器實例（含殘留 daemon 與未關閉的舊視窗）。
/// 場景：舊實例占住中繼埠，新實例（提權）可終止同使用者同名行程後重綁。
/// 刻意只比對自身 exe 檔名，絕不碰客戶端（Lin.bin／LogonX64）或其他程序。
pub fn terminate_stale_launcher_instances(log: &LogHandle) -> usize {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

    let Ok(self_exe) = std::env::current_exe() else {
        return 0;
    };
    let Some(self_name) = self_exe.file_name().map(|s| s.to_string_lossy().to_lowercase()) else {
        return 0;
    };
    let me = std::process::id();
    let mut killed = 0usize;
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return 0,
        };
        let mut pe = PROCESSENTRY32W::default();
        pe.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let end = pe
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(pe.szExeFile.len());
                let name = String::from_utf16_lossy(&pe.szExeFile[..end]).to_lowercase();
                if name == self_name && pe.th32ProcessID != me {
                    match OpenProcess(PROCESS_TERMINATE, false, pe.th32ProcessID) {
                        Ok(h) => {
                            if TerminateProcess(h, 2).is_ok() {
                                killed += 1;
                                log.info(format!(
                                    "已終止殘留登入器實例 pid {}（佔用中繼埠）",
                                    pe.th32ProcessID
                                ));
                            }
                            let _ = CloseHandle(h);
                        }
                        Err(_) => {}
                    }
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    killed
}

/// 交棒：以 DETACHED_PROCESS 啟動自身 --relay-daemon，讓 UI 退出後中繼不斷線。
/// 刻意走 CreateProcessW（參數僅固定旗標，不經任何 shell）。
pub fn spawn_relay_daemon(log: &LogHandle) -> bool {
    use windows::core::{PWSTR, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        CreateProcessW, CREATE_NEW_PROCESS_GROUP, CREATE_UNICODE_ENVIRONMENT,
        DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
    };

    let Ok(self_exe) = std::env::current_exe() else {
        log.error("交棒失敗：取自身路徑錯誤");
        return false;
    };
    let exe_str = self_exe.to_string_lossy().to_string();
    let exe_w = wide(&exe_str);
    let mut cmd = wide(&format!("\"{exe_str}\" --relay-daemon"));

    let mut si = STARTUPINFOW::default();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessW(
            PCWSTR(exe_w.as_ptr()),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT,
            None,
            None,
            &si,
            &mut pi,
        )
    };
    match ok {
        Ok(()) => {
            let _ = unsafe { CloseHandle(pi.hThread) };
            let _ = unsafe { CloseHandle(pi.hProcess) };
            log.info(format!("常駐中繼 daemon 已交棒（pid {}）", pi.dwProcessId));
            true
        }
        Err(e) => {
            log.error(format!("交棒常駐中繼失敗：{e}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_ini_points_at_local_relay() {
        let t = login_ini_text(2000);
        assert!(t.contains("ip=127.0.0.1"));
        assert!(t.contains("port=2000"));
        assert!(t.contains("port=29467") == false);
        assert_eq!(login_ini_text(29467).matches("port=29467").count(), 1);
    }

    #[test]
    fn event_names_match_dll_swprintf_format() {
        let (done, fail) = event_names(44528);
        assert_eq!(done, "Local\\RHDInprocDone-44528");
        assert_eq!(fail, "Local\\RHDInprocFail-44528");
    }

    #[test]
    fn rc_table_covers_dll_values() {
        for rc in [RC_OK, RC_ALREADY_PATCHED, RC_NO_MODULE, RC_TIMEOUT, RC_PREIMAGE, RC_PROTECT, RC_WRITE, RC_UNSAFE] {
            assert!(!describe_rc(rc).starts_with("未知"));
        }
    }

    #[test]
    fn failed_suspended_child_is_terminated_and_waited() {
        use windows::core::{PCWSTR, PWSTR};
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{
            CreateProcessW, TerminateProcess, WaitForSingleObject, CREATE_SUSPENDED,
            PROCESS_INFORMATION, STARTUPINFOW,
        };

        let exe_path = std::env::var("COMSPEC").expect("COMSPEC must identify cmd.exe");
        let exe = wide(&exe_path);
        let mut command = wide(&format!("\"{exe_path}\" /c exit 0"));
        let mut startup = STARTUPINFOW::default();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut child = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessW(
                PCWSTR(exe.as_ptr()),
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED,
                None,
                PCWSTR::null(),
                &startup,
                &mut child,
            )
            .expect("create isolated suspended cmd.exe");
        }

        let cleanup = terminate_suspended_process(child.hProcess);
        let wait = unsafe { WaitForSingleObject(child.hProcess, 1000) };
        if wait != WAIT_OBJECT_0 {
            unsafe {
                let _ = TerminateProcess(child.hProcess, 1);
                let _ = WaitForSingleObject(child.hProcess, 5000);
            }
        }
        unsafe {
            let _ = CloseHandle(child.hThread);
            let _ = CloseHandle(child.hProcess);
        }
        cleanup.expect("terminate and wait for isolated child");
        assert_eq!(wait, WAIT_OBJECT_0, "child must be signaled before return");
    }
}
