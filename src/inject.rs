//! 取代 Login.exe 的核心：啟動 bin64\LogonX64.bin 並注入 LinLogin.dll
//!
//! 依據 本機歷史分析文件 §6 還原出的原版流程：
//!   CreateProcess(bin64\LogonX64.bin) → 遠端執行緒注入 LinLogin.dll
//!   → 等待初始化 → 提權 SeDebugPrivilege → 啟動 eat.exe 1 1 global → 自身退出
//!
//! 因此我們不需要 Login.exe：只要複製這三步即可。
//! 已驗證 LinLogin.dll 匯出：Initializedll / loadingdll / TestFunction（__stdcall，無參數）。

use crate::logging::LogHandle;
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 提權到 SeDebugPrivilege（OpenProcess/WriteProcessMemory 需要）
pub fn enable_debug_privilege() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Foundation::LUID;
    use windows::Win32::Security::{
        AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
        TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = windows::Win32::Foundation::HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let name = wide("SeDebugPrivilege");
        let mut luid = LUID::default();
        let ok = LookupPrivilegeValueW(None, windows::core::PCWSTR(name.as_ptr()), &mut luid).is_ok();
        if !ok {
            let _ = CloseHandle(token);
            return false;
        }
        let mut tp = TOKEN_PRIVILEGES::default();
        tp.PrivilegeCount = 1;
        tp.Privileges[0].Luid = luid;
        tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
        let r = AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None).is_ok();
        let _ = CloseHandle(token);
        r
    }
}

/// 啟動客戶端並注入 LinLogin.dll。成功回傳 pid。
/// 依黑曜石筆記 §4.1 雙階段架構：hook 必須在 ResumeThread 之前完成。
pub fn launch_and_inject(game_dir: &str, log: LogHandle) -> Result<u32, String> {
    launch_and_inject_ex(game_dir, log, None)
}

/// target = Some((ip, port)) 時，會在 ResumeThread 前安裝 connect hook
pub fn launch_and_inject_ex(
    game_dir: &str,
    log: LogHandle,
    target: Option<([u8; 4], u16)>,
) -> Result<u32, String> {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Memory::{VirtualAllocEx, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};
    use windows::Win32::System::Threading::*;

    enable_debug_privilege();

    let dir = Path::new(game_dir);
    let client = dir.join("bin64").join("LogonX64.bin");
    let dll = dir.join("LinLogin.dll");
    if !client.exists() {
        return Err(format!("找不到客戶端 {}", client.display()));
    }
    if !dll.exists() {
        return Err(format!("找不到 {}", dll.display()));
    }

    // 1. 以掛起狀態建立行程
    let cmd = wide(&client.to_string_lossy());
    let cwd = wide(game_dir);
    unsafe {
        let mut si = STARTUPINFOW::default();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi = PROCESS_INFORMATION::default();
        let ok = CreateProcessW(
            windows::core::PCWSTR(cmd.as_ptr()),
            None,
            None,
            None,
            false,
            CREATE_SUSPENDED,
            None,
            windows::core::PCWSTR(cwd.as_ptr()),
            &si,
            &mut pi,
        );
        if ok.is_err() {
            return Err(format!("CreateProcess 失敗：{:?}", ok));
        }
        let pid = pi.dwProcessId;
        log.info(format!("已建立客戶端行程 pid {pid}（掛起）"));

        // 2. 配置記憶體寫入 DLL 路徑
        let dll_w = wide(&dll.to_string_lossy());
        let bytes = dll_w.len() * 2;
        let remote = VirtualAllocEx(pi.hProcess, None, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
        if remote.is_null() {
            let _ = TerminateProcess(pi.hProcess, 1);
            return Err("VirtualAllocEx 失敗".into());
        }
        let mut written = 0usize;
        if WriteProcessMemory(
            pi.hProcess,
            remote,
            dll_w.as_ptr() as *const c_void,
            bytes,
            Some(&mut written),
        )
        .is_err()
        {
            let _ = TerminateProcess(pi.hProcess, 1);
            return Err("WriteProcessMemory 失敗".into());
        }

        // 3. 取 LoadLibraryW 位址（kernel32 在兩個行程中基址相同）
        let k32 = GetModuleHandleW(windows::core::w!("kernel32.dll")).map_err(|e| e.to_string())?;
        let addr = GetProcAddress(k32, windows::core::s!("LoadLibraryW"))
            .ok_or("找不到 LoadLibraryW")?;

        // 4. 建立遠端執行緒呼叫 LoadLibraryW(path)
        let lp: LPTHREAD_START_ROUTINE = Some(std::mem::transmute(addr));
        let th = CreateRemoteThread(pi.hProcess, None, 0, lp, Some(remote), 0, None);
        match th {
            Ok(h) => {
                let _ = WaitForSingleObject(h, 10000);
                let _ = CloseHandle(h);
                log.info("LinLogin.dll 注入完成");
            }
            Err(e) => {
                log.error(format!("CreateRemoteThread 失敗：{e}"));
                let _ = TerminateProcess(pi.hProcess, 1);
                return Err(format!("注入失敗：{e}"));
            }
        }


        // 4.2 呼叫 LinLogin.dll 匯出函式
        // 依據：匯出表 = Initializedll / TestFunction / loadingdll
        // 做法：DLL 已載入客戶端且基址與本行程相同（同一次開機 ASLR），
        //       直接用本行程 LoadLibrary 取得基址 + 匯出偏移，遠端 CreateRemoteThread 呼叫
        {
            let local_dll = windows::Win32::System::LibraryLoader::LoadLibraryW(
                windows::core::PCWSTR(wide(&dll.to_string_lossy()).as_ptr()),
            );
            match local_dll {
                Ok(hmod) => {
                    let base = hmod.0 as usize;
                    log.info(format!("LinLogin.dll 本機基址 = 0x{base:X}"));
                    for name in ["Initializedll", "loadingdll", "TestFunction"] {
                        let nm = format!("{name}\0");
                        let ptr = GetProcAddress(
                            hmod,
                            windows::core::PCSTR(nm.as_ptr()),
                        );
                        match ptr {
                            Some(fp) => {
                                let addr = fp as usize;
                                log.info(format!("{name} @ 0x{addr:X}"));
                                let lp2: LPTHREAD_START_ROUTINE =
                                    Some(std::mem::transmute(addr));
                                match CreateRemoteThread(pi.hProcess, None, 0, lp2, None, 0, None) {
                                    Ok(h2) => {
                                        let _ = WaitForSingleObject(h2, 5000);
                                        let _ = CloseHandle(h2);
                                        log.info(format!("已呼叫 LinLogin.dll!{name}"));
                                    }
                                    Err(er) => log.warn(format!("呼叫 {name} 失敗：{er}")),
                                }
                            }
                            None => log.warn(format!("找不到匯出 {name}")),
                        }
                    }
                }
                Err(e) => log.warn(format!("本機 LoadLibraryW 失敗：{e}")),
            }
        }

        // 5. 先恢復主執行緒（模組才會完成載入，EnumProcessModulesEx 才能列舉）
        ResumeThread(pi.hThread);
        std::thread::sleep(std::time::Duration::from_millis(300));

        // 4.5 §10.18 / §4.1：在 ResumeThread 之前安裝 SocketMain connect hook
        log.info(format!("hook 目標參數 = {:?}", target));
        if let Some((ip, port)) = target {
            log.info("開始查詢客戶端模組基底…");
            let base_res = crate::sockhook::main_module_base_psapi(pi.hProcess)
                .or_else(|e| {
                    log.warn(format!("EnumProcessModulesEx 失敗（{e}），改用 module snapshot"));
                    crate::sockhook::module_base(pid, "LogonX64.bin")
                });
            match base_res {
                Ok(base) => match crate::sockhook::verify_connect_site(base) {
                    Ok(patch_addr) => {
                        log.info(format!(
                            "connect hook 位址驗證通過 base=0x{base:X} patch=0x{patch_addr:X}"
                        ));
                        let orig = &crate::sockhook::EXPECT_CONNECT_PREFIX;
                        let mut sc = crate::remotehook::build_redirect_stub(
                            ip,
                            port,
                            patch_addr as u64,
                            orig,
                        );
                        match crate::remotehook::alloc_and_write(pi.hProcess, &sc) {
                            Ok(stub) => {
                                crate::remotehook::fix_jmp(
                                    &mut sc,
                                    stub,
                                    patch_addr as u64,
                                    orig.len(),
                                );
                                // 覆寫修正後的 jmp
                                let _ = windows::Win32::System::Diagnostics::Debug::WriteProcessMemory(
                                    pi.hProcess,
                                    (stub + (sc.len() - 5) as u64) as *const c_void,
                                    sc[sc.len() - 5..].as_ptr() as *const c_void,
                                    5,
                                    None,
                                );
                                match crate::remotehook::install_jmp(
                                    pi.hProcess,
                                    patch_addr as u64,
                                    stub,
                                    orig.len(),
                                ) {
                                    Ok(_) => log.info(format!(
                                        "connect hook 已安裝：{}.{}.{}.{}:{port} → 目標",
                                        ip[0], ip[1], ip[2], ip[3]
                                    )),
                                    Err(e) => log.error(format!("install_jmp 失敗：{e}")),
                                }
                            }
                            Err(e) => log.error(format!("shellcode 寫入失敗：{e}")),
                        }
                    }
                    Err(e) => log.error(format!("connect 位址驗證失敗，跳過 hook：{e}")),
                },
                Err(e) => log.error(format!("取得客戶端模組基底失敗：{e}")),
            }
        }

        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);

        let _ = (TH32CS_SNAPPROCESS, HANDLE::default());
        Ok(pid)
    }
}
/// 方案 A：以 Login.exe 身分啟動客戶端
/// 做法：把本程式複製成 <game_dir>\RHDLoginHost.exe，改名為 Login.exe 的角色，
/// 再由它去 CreateProcess 客戶端 → 父進程名稱 = Login.exe
pub fn launch_via_login_host(
    game_dir: &str,
    log: LogHandle,
) -> Result<u32, String> {
    use std::os::windows::process::CommandExt;

    let self_exe = std::env::current_exe().map_err(|e| format!("取自身路徑失敗：{e}"))?;
    let host_path = Path::new(game_dir).join("Login.exe");

    // 若目標不存在或版本不同，複製一份
    let need_copy = match (std::fs::metadata(&self_exe), std::fs::metadata(&host_path)) {
        (Ok(a), Ok(b)) => a.len() != b.len(),
        (Ok(_), Err(_)) => true,
        _ => false,
    };
    if need_copy {
        std::fs::copy(&self_exe, &host_path)
            .map_err(|e| format!("複製 Login.exe 失敗：{e}"))?;
        log.info(format!("已部署 LoginHost -> {}", host_path.display()));
    }

    // 以 --login-host 啟動（父進程會是這支 Login.exe）
    let child = Command::new(&host_path)
        .arg("--login-host")
        .current_dir(game_dir)
        .creation_flags(0x0000_0008) // DETACHED_PROCESS
        .spawn()
        .map_err(|e| format!("啟動 LoginHost 失敗：{e}"))?;

    log.info(format!("LoginHost 已啟動 pid {}（扮演 Login.exe）", child.id()));
    // 等客戶端出現
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if let Some(name) = crate::launch::client_process_running() {
            log.info(format!("客戶端已出現：{name}"));
            return Ok(child.id());
        }
    }
    Err("等待客戶端逾時".into())
}
/// 雙層啟動（依 lin.log 成功紀錄）：
///   成功版的父進程本身就是 LogonX64.bin，代表客戶端會自己再啟動一個自己。
///   因此我方只需：啟動一個「父層 LogonX64.bin」，讓它自行生出子進程。
/// 策略：不掛起、不注入，直接 CreateProcess，讓客戶端走它自己的完整流程。
pub fn launch_plain(game_dir: &str, exe_name: &str, log: LogHandle) -> Result<u32, String> {
    use std::os::windows::process::CommandExt;

    let dir = Path::new(game_dir);
    let exe = dir.join(exe_name);
    if !exe.exists() {
        return Err(format!("找不到 {}", exe.display()));
    }

    let child = Command::new(&exe)
        .current_dir(dir)
        // CREATE_NEW_PROCESS_GROUP，讓它獨立於 launcher
        .creation_flags(0x0000_0200)
        .spawn()
        .map_err(|e| format!("啟動 {} 失敗：{e}", exe.display()))?;

    let pid = child.id();
    log.info(format!("已啟動 {exe_name} pid {pid}（不注入，讓客戶端自行建立子進程）"));
    Ok(pid)
}
/// 依開源 Connector 實作的正確啟動方式：
///   執行檔 = bin64\Lin.bin
///   工作目錄 = 客戶端根目錄（不是 bin64）
///   注入 DLL = bdcap64.dll, libcocos2d.dll（bb64.dll 本機缺）
///   環境變數 = L1_DLL_PASSWORD, L1_CLIENT_SIDE_KEY
pub fn launch_correct(game_dir: &str, log: LogHandle) -> Result<u32, String> {
    use std::os::windows::process::CommandExt;

    enable_debug_privilege();

    let root = Path::new(game_dir);
    let bin = root.join("bin64");
    let exe = bin.join("Lin.bin");
    if !exe.exists() {
        return Err(format!("找不到 {}", exe.display()));
    }

    // 1. 寫 Login.ini 到客戶端根目錄
    let login_ini = root.join("Login.ini");
    let ini = "[Server]\r\nip=127.0.0.1\r\nport=2000\r\n\r\n[Options]\r\nRememberUser=1\r\nLanguage=1\r\n";
    if std::fs::write(&login_ini, ini).is_ok() {
        log.info(format!("Login.ini 已寫入 {}", login_ini.display()));
    }

    // 2. 收集要注入的 DLL
    let mut dlls: Vec<PathBuf> = Vec::new();
    // 依開源專案成功紀錄：只注入 libcocos2d.dll（bb64/bdcap64 會導致 Access denied）
    for name in ["libcocos2d.dll"] {
        let p = bin.join(name);
        if p.exists() {
            dlls.push(p);
        } else {
            log.warn(format!("DLL 不存在，略過：{name}"));
        }
    }
    if dlls.is_empty() {
        return Err("沒有可注入的 DLL".into());
    }
    let dll_names: Vec<String> = dlls
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    log.info(format!("將注入 {} 支 DLL：{:?}", dlls.len(), dll_names));

    // 3. 以 CREATE_SUSPENDED 啟動（工作目錄 = 根目錄）
    let mut cmd = Command::new(&exe);
    cmd.current_dir(root);
    cmd.env("L1_DLL_PASSWORD", "2052201973");
    cmd.env("L1_CLIENT_SIDE_KEY", "711666385");
    cmd.creation_flags(0x0000_0004); // CREATE_SUSPENDED

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("啟動 Lin.bin 失敗：{e}"))?;
    let pid = child.id();
    log.info(format!("Lin.bin 已啟動 pid {pid}（掛起，cwd=根目錄）"));

    // 4. 注入每支 DLL
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        use windows::Win32::System::Memory::{VirtualAllocEx, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE};
        use windows::Win32::System::Threading::*;

        let hproc = OpenProcess(PROCESS_ALL_ACCESS, false, pid)
            .map_err(|e| format!("OpenProcess 失敗：{e}"))?;

        let k32 = GetModuleHandleW(windows::core::w!("kernel32.dll"))
            .map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let load_addr = GetProcAddress(k32, windows::core::s!("LoadLibraryW"))
            .ok_or("找不到 LoadLibraryW")?;

        for dll in &dlls {
            let dll_w = wide(&dll.to_string_lossy());
            let bytes = dll_w.len() * 2;
            let remote = VirtualAllocEx(hproc, None, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            if remote.is_null() {
                log.warn("VirtualAllocEx 失敗");
                continue;
            }
            let mut w = 0usize;
            if WriteProcessMemory(
                hproc,
                remote,
                dll_w.as_ptr() as *const c_void,
                bytes,
                Some(&mut w),
            )
            .is_err()
            {
                log.warn("WriteProcessMemory 失敗");
                continue;
            }
            let lp: LPTHREAD_START_ROUTINE = Some(std::mem::transmute(load_addr));
            match CreateRemoteThread(hproc, None, 0, lp, Some(remote), 0, None) {
                Ok(h) => {
                    let _ = WaitForSingleObject(h, 8000);
                    let _ = CloseHandle(h);
                    log.info(format!(
                        "已注入 {}",
                        dll.file_name().unwrap().to_string_lossy()
                    ));
                }
                Err(e) => log.warn(format!("注入失敗：{e}")),
            }
        }

        // 5. 恢復主執行緒
        let _ = CloseHandle(hproc);
    }

    unsafe {
        let _ = resume_process(pid);
    }

    log.info(format!("Lin.bin pid {pid} 已恢復執行"));
    let _ = child.id();
    Ok(pid)
}

/// B1 路線（零黑盒直啟）：不掛 Login.exe、不注入任何 DLL。
/// 流程：寫 Login.ini → 啟動 bin64\Lin.bin（cwd=根目錄，複用 launch::launch_login）→
///       背景執行緒輪詢等解殼指標（RVA 0x5CBB800 出現 2002122601）→ 寫六刀補丁。
/// 證據：黑曜石《20261025-天R登入器執行期採證與LinLogin六刀補丁破解》§10.5–10.6。
/// 成功回傳客戶端 pid；補丁結果只記 log（不阻斷啟動）。
pub fn launch_b1(game_dir: &str, log: LogHandle) -> Result<u32, String> {
    let root = Path::new(game_dir);
    let exe = root.join("bin64").join("Lin.bin");
    if !exe.exists() {
        return Err(format!("找不到 {}", exe.display()));
    }

    // 1. Login.ini（開源 Connector 慣例；連線仍以客戶端硬編碼 127.0.0.1:2000 → 中繼轉送）
    let login_ini = root.join("Login.ini");
    let ini = "[Server]\r\nip=127.0.0.1\r\nport=2000\r\n\r\n[Options]\r\nRememberUser=1\r\nLanguage=1\r\n";
    let _ = std::fs::write(&login_ini, ini);

    // 2. 啟動客戶端（不掛起、不注入；六刀於解殼完成後外部寫入，同 LinLogin 的時序）
    let launched = crate::launch::launch_login(game_dir, r"bin64\Lin.bin")?;
    let pid = launched.pid;
    log.info(format!("B1 直啟 Lin.bin pid {pid}（零注入，等待解殼後寫六刀）"));

    // 3. 背景執行緒：等解殼指標 → 六刀（逾時/失敗只記 log，便於事後診斷）
    let tlog = log.clone();
    std::thread::spawn(move || match crate::patches::wait_and_patch(pid, tlog.clone()) {
        Ok(n) if n > 0 => tlog.info(format!("B1 補丁完成（{n} 刀）")),
        Ok(_) => {}
        Err(e) => tlog.error(format!("B1 補丁失敗：{e}")),
    });
    Ok(pid)
}

/// 恢復指定行程所有執行緒
unsafe fn resume_process(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
    let snap = match snap {
        Ok(h) => h,
        Err(_) => return false,
    };
    let mut te = THREADENTRY32::default();
    te.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    if Thread32First(snap, &mut te).is_err() {
        let _ = CloseHandle(snap);
        return false;
    }
    loop {
        if te.th32OwnerProcessID == pid {
            if let Ok(th) = OpenThread(THREAD_SUSPEND_RESUME, false, te.th32ThreadID) {
                ResumeThread(th);
                let _ = CloseHandle(th);
            }
        }
        if Thread32Next(snap, &mut te).is_err() {
            break;
        }
    }
    let _ = CloseHandle(snap);
    true
}