//! SocketMain connect 重定向 Hook
//!
//! 依據黑曜石筆記 §10.18（證據等級 PASS）：
//!   SocketMain@Lineage 的 connect 呼叫點 = ImageBase + 0xA7A8C4
//!   呼叫慣例：rcx = SOCKET, rdx = sockaddr_in*（16 bytes）, r8d = 0x10
//!   原始指令序列（Lin_unpacked.exe 實測）：
//!     48 8d 54 24 20     lea rdx, [rsp+0x20]
//!     41 b8 10 00 00 00  mov r8d, 0x10
//!     ff 15 xx xx xx xx  call [ws2_32!connect]
//!
//! 策略：不做 inline hook（風險高、需寫機器碼），改在目標位址設一個
//! 硬體斷點風格的『攔截後改寫』——實務上採用最穩妥的做法：
//! 由 launcher 在本行程空間預先改寫客戶端已載入的 .text 為一小段跳板。
//!
//! ⚠ 本模組目前僅提供位址解析與 patch 前置檢查；實際套用需在客戶端
//!   行程內（遠端）執行，屬進階操作，預設不自動啟用。

use std::ffi::c_void;

/// ImageBase + 0xA7A8C4：SocketMain 的 connect 呼叫點
pub const RVA_CONNECT_SOCKETMAIN: usize = 0xA7A8C4;
/// 預期的前 12 bytes（用於驗證客戶端版本未被替換）
pub const EXPECT_CONNECT_PREFIX: [u8; 10] = [
    0x48, 0x8D, 0x54, 0x24, 0x20, // lea rdx, [rsp+0x20]
    0x41, 0xB8, 0x10, 0x00, 0x00, // mov r8d, 0x10（前 3 bytes）
];

/// 驗證目標記憶體是否為預期指令（防換版誤 patch）
pub fn verify_connect_site(image_base: usize) -> Result<usize, String> {
    let addr = image_base + RVA_CONNECT_SOCKETMAIN;
    unsafe {
        let p = addr as *const u8;
        let mut got = [0u8; 10];
        std::ptr::copy_nonoverlapping(p, got.as_mut_ptr(), 10);
        if got != EXPECT_CONNECT_PREFIX {
            return Err(format!(
                "connect 位址 0x{addr:X} 指令不符預期：{:02X?}（可能是客戶端換版）",
                got
            ));
        }
    }
    Ok(addr)
}

/// 建立 sockaddr_in 的 16 bytes
pub fn sockaddr_in(ip: [u8; 4], port: u16) -> [u8; 16] {
    let mut sa = [0u8; 16];
    sa[0] = 2; // AF_INET
    sa[1] = 0;
    sa[2] = (port >> 8) as u8;
    sa[3] = (port & 0xFF) as u8;
    sa[4..8].copy_from_slice(&ip);
    sa
}

/// 讀出目前記憶體中的 sockaddr_in（診斷用）
pub fn read_sockaddr_in(ptr: *const c_void) -> ([u8; 4], u16) {
    let mut sa = [0u8; 16];
    unsafe {
        std::ptr::copy_nonoverlapping(ptr as *const u8, sa.as_mut_ptr(), 16);
    }
    let port = ((sa[2] as u16) << 8) | sa[3] as u16;
    ([sa[4], sa[5], sa[6], sa[7]], port)
}

/// 改寫 sockaddr_in（在 hook 內呼叫）
pub fn write_sockaddr_in(ptr: *mut c_void, ip: [u8; 4], port: u16) {
    let sa = sockaddr_in(ip, port);
    unsafe {
        std::ptr::copy_nonoverlapping(sa.as_ptr(), ptr as *mut u8, 16);
    }
}

/// 模組基底（客戶端載入後的實際 ImageBase，受 ASLR 影響）
/// 取得模組基底（含重試：掛起狀態下模組可能尚未完成列舉）
pub fn module_base(pid: u32, module: &str) -> Result<usize, String> {
    let mut last = String::new();
    for _attempt in 0..5 {
        match module_base_once(pid, module) {
            Ok(b) => return Ok(b),
            Err(e) => {
                last = e;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    Err(last)
}

fn module_base_once(pid: u32, module: &str) -> Result<usize, String> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::System::Diagnostics::ToolHelp::*;

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
            .map_err(|e| format!("Module snapshot 失敗：{e}"))?;
        if snap == INVALID_HANDLE_VALUE {
            return Err("無效的 module snapshot".into());
        }
        let mut me = MODULEENTRY32W::default();
        me.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut result = None;
        if Module32FirstW(snap, &mut me).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &me.szModule[..me
                        .szModule
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(me.szModule.len())],
                );
                if name.eq_ignore_ascii_case(module) {
                    result = Some(me.modBaseAddr as usize);
                    break;
                }
                if Module32NextW(snap, &mut me).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        result.ok_or_else(|| format!("找不到模組 {module}"))
    }
}
/// 取得本行程載入的客戶端模組基底（若本行程即客戶端）
pub fn module_base_for_self() -> Result<usize, String> {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    for name in ["LogonX64.bin", "Lin.bin"] {
        let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        if let Ok(h) = unsafe { GetModuleHandleW(windows::core::PCWSTR(w.as_ptr())) } {
            if !h.is_invalid() {
                return Ok(h.0 as usize);
            }
        }
    }
    Err("本行程未載入客戶端模組".into())
}
/// 以 EnumProcessModulesEx 取得主模組基底（對 CREATE_SUSPENDED 行程較可靠）
pub fn main_module_base_psapi(hproc: windows::Win32::Foundation::HANDLE) -> Result<usize, String> {
    use windows::Win32::System::ProcessStatus::{EnumProcessModulesEx, LIST_MODULES_ALL};
    use windows::Win32::Foundation::HMODULE;

    let mut mods: [HMODULE; 1024] = [HMODULE::default(); 1024];
    let mut needed = 0u32;
    let ok = unsafe {
        EnumProcessModulesEx(
            hproc,
            mods.as_mut_ptr(),
            (mods.len() * std::mem::size_of::<HMODULE>()) as u32,
            &mut needed,
            LIST_MODULES_ALL,
        )
    };
    if ok.is_err() {
        return Err(format!("EnumProcessModulesEx 失敗：{ok:?}"));
    }
    if needed == 0 {
        return Err("EnumProcessModulesEx 回傳 0 個模組".into());
    }
    // 第一個模組即主映像
    let base = mods[0].0 as usize;
    if base == 0 {
        return Err("主模組基底為 0".into());
    }
    Ok(base)
}