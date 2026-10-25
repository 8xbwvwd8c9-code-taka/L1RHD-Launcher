//! B1 直啟補丁：複製 LinLogin.dll 的六刀記憶體補丁（路線 B，零黑盒）
//!
//! 證據與推導全記錄：黑曜石《20261025-天R登入器執行期採證與LinLogin六刀補丁破解》§10.6
//! 傾印證據：本機歷史採證目錄
//!
//! 機制摘要：
//! 1. 客戶端 bin64\Lin.bin（=LogonX64.bin）磁片檔由 SecureEngine 加殼，執行期自解。
//! 2. 解殼完成時，客戶端會在 ImageBase+0x5CBB800 寫出硬編碼版本 dword 2002122601。
//! 3. 等 01 出現後，寫入六刀（此時機 = LinLogin.dll 匯出 TestFunction 的內部邏輯，
//!    太早寫會被解殼器覆蓋）。
//! 4. 六刀寫完 → 版本對 05、GameGuard 錯誤分派器被跳過（380 不復現）、
//!    握手 imm32 改 0x960EC7F2 —— 與路線 A（原版 Login.exe 交接）客戶端狀態等價。

use crate::logging::LogHandle;
use std::time::{Duration, Instant};

/// 等待客戶端寫出「未補丁版本號」的輪詢間隔與逾時
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const POLL_TIMEOUT: Duration = Duration::from_secs(120);
/// 模組快照重試間隔（SecureEngine 殼早期模組表未備妥會回 ERROR_PARTIAL_COPY，屬暫態）
const SNAP_RETRY: Duration = Duration::from_millis(200);
/// 解殼指標出現後的緩衝期（秒）——等 SecureEngine 內核驅動完成解殼後的
/// 保護頁/完整性檢查佈署再寫刀。實測（2026-10-25）顯示在此窗口期用外部
/// WriteProcessMemory 寫入有機率觸發內核死鎖（全系統當機，事件 41/6008）。
/// 路線 A 的 LinLogin.dll 從進程「內部」寫入所以驅動容忍；B1 是外部寫入必須避峰。
const UNPACK_GRACE_SECS: u64 = 8;

/// 客戶端原始（未補丁）版本 dword —— SecureEngine 解殼完成指標
const VERSION_RAW: u32 = 2002122601;

/// 六刀靜態補丁表：RVA → 位元組序列（全靜態，無執行期計算）
/// 證據：LinLogin.dll 解密映像反組譯 + Lin 執行期映像命中比對（§10.1/§10.6）
const SIX_BLADES: &[(usize, &[u8], &str)] = &[
    // 先寫 GG 相關刀（順序防禦：即使後續刀來不及，380 已被這兩刀擋下）
    // jmp +0x3C9 → 0x98005A（GameGuard 成功路徑，跳過含 380 的錯誤分派器）
    (0x97FC8C, &[0xE9, 0xC9, 0x03, 0x00, 0x00], "JMP強制GG成功"),
    // 函式開頭 RET（閹割 GG 相關檢查函式）
    (0x97D020, &[0xC3, 0x90, 0x90, 0x90, 0xC3, 0x90], "RET閹割檢查"),
    // NOP 掉 2-byte 條件跳轉 → 函式恆返成功
    (0xE658B1, &[0x90, 0x90], "NOP條件跳轉"),
    // 版本欄 1：2002122601 → 2002122605（0x6DF75577）
    (0x5CBB800, &[0x6D, 0xF7, 0x55, 0x77], "版本欄1=2002122605"),
    // 版本欄 2
    (0x5CBB808, &[0x6D, 0xF7, 0x55, 0x77], "版本欄2=2002122605"),
    // CS_CLIENT_VERSION_INFO 握手欄位 imm32 → 0x960EC7F2
    (0x9E4CA8, &[0xF2, 0xC7, 0x0E, 0x96], "握手imm32=0x960EC7F2"),
];

/// Lin.bin / LogonX64.bin 的 PE TimeDateStamp（2020-02-12 build，MD5 18328DB2...）
/// —— 映像指紋，不受殼隱藏模組清單影響
const LIN_TIMESTAMP: u32 = 0x5E43_D8D0;
/// 實測慣用基址（2020 minidump 與 2026 run3 傾印皆為此值）——先試提示再全掃
const LIN_BASE_HINT: usize = 0x1_4000_0000;

/// 讀目標位址並驗證是否為 Lin.bin 映像（MZ + PE + TimeDateStamp 三重比對）
/// （保留給未抹標頭的場景；SecureEngine 實測會抹除映像標頭，主要路徑改走 PEB）
unsafe fn is_lin_image(hproc: windows::Win32::Foundation::HANDLE, addr: usize) -> bool {
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;

    let mut head = [0u8; 0x400];
    let mut read = 0usize;
    if ReadProcessMemory(
        hproc,
        addr as *const _,
        head.as_mut_ptr() as *mut _,
        0x400,
        Some(&mut read),
    )
    .is_err()
        || read < 0x200
        || &head[..2] != b"MZ"
    {
        return false;
    }
    let lfanew = u32::from_le_bytes(head[0x3C..0x40].try_into().unwrap()) as usize;
    if lfanew + 8 > 0x400 || &head[lfanew..lfanew + 4] != b"PE\0\0" {
        return false;
    }
    u32::from_le_bytes(head[lfanew + 4..lfanew + 8].try_into().unwrap()) == LIN_TIMESTAMP
}

/// 經 NtQueryInformationProcess → PEB → ImageBaseAddress 取得主映像基址。
/// 殼可以隱藏模組表、抹除映像標頭，但 PEB 的 ImageBase 是載入器事實（無法造假）。
unsafe fn base_from_peb(hproc: windows::Win32::Foundation::HANDLE) -> Option<usize> {
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    let ntdll = GetModuleHandleW(windows::core::w!("ntdll.dll")).ok()?;
    let addr = GetProcAddress(ntdll, windows::core::s!("NtQueryInformationProcess"))?;
    type NtQip = unsafe extern "system" fn(
        windows::Win32::Foundation::HANDLE,
        u32,
        *mut core::ffi::c_void,
        u32,
        *mut u32,
    ) -> i32;
    let f: NtQip = core::mem::transmute(addr);

    // PROCESS_BASIC_INFORMATION (x64)：PebBaseAddress 在 offset 8
    #[repr(C)]
    struct Pbi {
        reserved: [u8; 8],
        peb: usize,
        affinity: usize,
        priority: usize,
        pid: usize,
        reserved3: usize,
    }
    let mut pbi = Pbi {
        reserved: [0; 8],
        peb: 0,
        affinity: 0,
        priority: 0,
        pid: 0,
        reserved3: 0,
    };
    let mut ret_len = 0u32;
    let st = f(hproc, 0, &mut pbi as *mut _ as *mut _, 48, &mut ret_len);
    if st != 0 || pbi.peb == 0 {
        return None;
    }
    // PEB (x64)：ImageBaseAddress 在 PEB+0x10
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory as ReadPeb;
    let mut base = 0usize;
    let mut read = 0usize;
    if ReadPeb(
        hproc,
        (pbi.peb + 0x10) as *const _,
        &mut base as *mut _ as *mut _,
        8,
        Some(&mut read),
    )
    .is_ok()
        && read == 8
        && base > 0x1_0000
    {
        Some(base)
    } else {
        None
    }
}

/// 在目標進程尋找客戶端主映像基址（策略鏈）：
///   1. PEB ImageBaseAddress（NtQueryInformationProcess；殼無法造假）← 主路徑
///   2. 慣用基址 0x140000000 提示（標頭未被抹時驗證 TimeDateStamp）
///   3. VirtualQueryEx 位址空間掃描（同上驗證；殼抹標頭時無效，僅留作未來相容）
fn find_client_base(pid: u32) -> Result<usize, String> {
    use windows::Win32::System::Memory::{VirtualQueryEx, MEMORY_BASIC_INFORMATION};
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};

    unsafe {
        let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid)
            .map_err(|e| format!("OpenProcess(掃描): {e}"))?;

        // 1. PEB（主路徑）
        if let Some(b) = base_from_peb(h) {
            let _ = windows::Win32::Foundation::CloseHandle(h);
            return Ok(b);
        }

        // 2. 慣用基址提示
        if is_lin_image(h, LIN_BASE_HINT) {
            let _ = windows::Win32::Foundation::CloseHandle(h);
            return Ok(LIN_BASE_HINT);
        }

        // 3. 位址空間掃描（後備）
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let mut addr = 0usize;
        let mut found = None;
        while addr < 0x7F00_0000_0000 {
            let r = VirtualQueryEx(
                h,
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            if r == 0 {
                break;
            }
            let size = mbi.RegionSize;
            if mbi.State.0 == 0x1000 /* MEM_COMMIT */ && size >= 0x1000 && size <= 0x800_0000 {
                if is_lin_image(h, addr) {
                    found = Some(addr);
                    break;
                }
            }
            let next = addr.saturating_add(size);
            if next <= addr {
                break;
            }
            addr = next;
        }
        let _ = windows::Win32::Foundation::CloseHandle(h);
        found.ok_or_else(|| "掃描未找到 Lin.bin 映像（PEB 失敗且標頭被抹）".to_string())
    }
}

/// B1 主流程：輪詢等 01 → 寫六刀 → 覆核。
/// 回傳 Ok(寫入刀數) 或 Err(失敗原因)。
pub fn wait_and_patch(pid: u32, log: LogHandle) -> Result<usize, String> {
    use windows::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
    use windows::Win32::System::Memory::{VirtualProtectEx, PAGE_EXECUTE_READWRITE};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_OPERATION, PROCESS_VM_READ,
        PROCESS_VM_WRITE,
    };

    unsafe {
        let hproc = OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_VM_WRITE | PROCESS_VM_OPERATION,
            false,
            pid,
        )
        .map_err(|e| format!("OpenProcess: {e}"))?;

        // 1. 找基址（帶重試：實測客戶端剛啟動時 Toolhelp 快照會回
        //    ERROR_PARTIAL_COPY(0x12B)——SecureEngine 殼早期模組表未備妥，屬暫態）
        let mut base = Err("模組快照重試逾時".to_string());
        let snap_t0 = Instant::now();
        let mut last_report = Instant::now() - Duration::from_secs(5);
        for _ in 0..(POLL_TIMEOUT.as_millis() / SNAP_RETRY.as_millis()) {
            match find_client_base(pid) {
                Ok(b) => {
                    base = Ok(b);
                    break;
                }
                Err(e) => {
                    if last_report.elapsed() >= Duration::from_secs(5) {
                        log.warn(format!(
                            "模組快照重試中（{:.1}s）：{e}",
                            snap_t0.elapsed().as_secs_f32()
                        ));
                        last_report = Instant::now();
                    }
                    base = Err(e);
                }
            }
            std::thread::sleep(SNAP_RETRY);
        }
        let base = base?;
        log.info(format!("客戶端模組基址 {base:#x}"));

        // 2. 輪詢等解殼完成（dword 2002122601 出現）
        let ver_addr = base + 0x5CBB800;
        let t0 = Instant::now();
        loop {
            let mut buf = [0u8; 4];
            let mut read = 0usize;
            if ReadProcessMemory(
                hproc,
                ver_addr as *const _,
                buf.as_mut_ptr() as *mut _,
                4,
                Some(&mut read),
            )
            .is_ok()
                && read == 4
            {
                let v = u32::from_le_bytes(buf);
                if v == VERSION_RAW {
                    break;
                }
                if v == 0x6DF75577 {
                    // 已是 05（例如重複啟動）——冪等保護，直接回報成功
                    log.info("版本已是 2002122605（已補丁過），跳過".to_string());
                    let _ = windows::Win32::Foundation::CloseHandle(hproc);
                    return Ok(0);
                }
            }
            if t0.elapsed() > POLL_TIMEOUT {
                let _ = windows::Win32::Foundation::CloseHandle(hproc);
                return Err("等待解殼指標(2002122601)逾時 120s".into());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        log.info(format!(
            "解殼完成指標出現（耗時 {}ms），緩衝 {}s 等待驅動站穩後寫入六刀",
            t0.elapsed().as_millis(),
            UNPACK_GRACE_SECS
        ));
        // 緩衝期：等 SecureEngine 驅動完成解殼後的保護佈署，避開外部寫入的
        // 內核死鎖窗口（2026-10-25 兩次全系統當機事故的防禦，見筆記 §14）
        std::thread::sleep(Duration::from_secs(UNPACK_GRACE_SECS));

        // 3. 逐刀：讀原值記錄 → 需要時改頁保護 → 寫入 → 覆核 → 還原保護
        let mut done = 0usize;
        for (rva, bytes, desc) in SIX_BLADES {
            let addr = base + rva;
            let mut orig = vec![0u8; bytes.len()];
            let mut read = 0usize;
            let _ = ReadProcessMemory(
                hproc,
                addr as *const _,
                orig.as_mut_ptr() as *mut _,
                bytes.len(),
                Some(&mut read),
            );
            log.info(format!(
                "刀 {desc} @RVA {rva:#x} 原值 {}",
                orig.iter().map(|b| format!("{b:02X}")).collect::<String>()
            ));

            let mut old_protect = windows::Win32::System::Memory::PAGE_PROTECTION_FLAGS(0);
            let need_protect = VirtualProtectEx(
                hproc,
                addr as *const _,
                bytes.len(),
                PAGE_EXECUTE_READWRITE,
                &mut old_protect,
            )
            .is_ok();

            let mut written = 0usize;
            let w = WriteProcessMemory(
                hproc,
                addr as *const _,
                bytes.as_ptr() as *const _,
                bytes.len(),
                Some(&mut written),
            );
            let ok = w.is_ok() && written == bytes.len();

            // 覆核
            let mut check = vec![0u8; bytes.len()];
            let mut cread = 0usize;
            let _ = ReadProcessMemory(
                hproc,
                addr as *const _,
                check.as_mut_ptr() as *mut _,
                bytes.len(),
                Some(&mut cread),
            );
            let verified = ok && check == *bytes;

            if need_protect {
                let mut tmp = windows::Win32::System::Memory::PAGE_PROTECTION_FLAGS(0);
                let _ = VirtualProtectEx(
                    hproc,
                    addr as *const _,
                    bytes.len(),
                    old_protect,
                    &mut tmp,
                );
            }

            if verified {
                done += 1;
                log.info(format!("刀 {desc} 寫入並覆核成功"));
            } else {
                log.error(format!("刀 {desc} 寫入或覆核失敗（w={w:?}）"));
            }
        }

        let _ = windows::Win32::Foundation::CloseHandle(hproc);
        if done == SIX_BLADES.len() {
            log.info("六刀全數寫入完成 —— 客戶端狀態等同路線 A（版本 2002122605）".to_string());
            Ok(done)
        } else {
            Err(format!("六刀僅 {done}/{} 成功", SIX_BLADES.len()))
        }
    }
}
