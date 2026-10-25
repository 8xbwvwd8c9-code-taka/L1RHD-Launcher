//! 遠端 hook：在客戶端行程內改寫 SocketMain connect 的 sockaddr_in
//!
//! 做法（不改客戶端檔案、不寫機器碼）：
//!   1. 在客戶端行程配置一塊 trampoline 記憶體
//!   2. 寫入一小段 shellcode：rdx 指向的 sockaddr_in 改寫為指定 IP/Port
//!   3. 用 WriteProcessMemory 把目標位址前 5 bytes 改成 jmp（inline hook）
//!   4. shellcode 結尾跳回原指令繼續執行
//!
//! 目標：ImageBase + 0xA7A8C4（lea rdx,[rsp+0x20] 的前 5 bytes）

use std::ffi::c_void;

/// 產生 x64 shellcode：
///   mov dword ptr [rdx+4], <ip>      ; sockaddr_in.sin_addr
///   mov word  ptr [rdx+2], <port&0xFFFF> ; sockaddr_in.sin_port (big-endian)
///   <原指令: lea rdx,[rsp+0x20] 等被覆蓋的 bytes 由 caller 補>
///   jmp back
///
/// 這裡只產生「改寫 sockaddr 並跳回」的最小片段。
pub fn build_redirect_stub(ip: [u8; 4], port: u16, patch_addr: u64, orig_bytes: &[u8]) -> Vec<u8> {
    let mut sc: Vec<u8> = Vec::new();

    // mov dword ptr [rdx+4], imm32     ; IP
    sc.extend_from_slice(&[0xC7, 0x42, 0x04]);
    sc.extend_from_slice(&ip);

    // mov word ptr [rdx+2], imm16      ; Port（big-endian 已由 caller 處理）
    let p_be = port.to_be_bytes();
    sc.extend_from_slice(&[0x66, 0xC7, 0x42, 0x02]);
    sc.extend_from_slice(&p_be);

    // 補回原指令（被 jmp 覆蓋的長度）
    sc.extend_from_slice(orig_bytes);

    // jmp back：E9 rel32（從 shellcode 結尾算到 patch_addr + orig_len）
    let jmp_placeholder_pos = sc.len();
    sc.push(0xE9);
    sc.extend_from_slice(&[0u8; 4]);

    // rel32 由實際擺放位址決定，回傳時先留 0，由 caller 修正
    let _ = (patch_addr, jmp_placeholder_pos);
    sc
}

/// 修正 shellcode 尾端 jmp 的 rel32
/// stub_addr = shellcode 實際所在位址
pub fn fix_jmp(sc: &mut [u8], stub_addr: u64, patch_addr: u64, orig_len: usize) {
    // jmp 指令位於 sc 最後 5 bytes
    let jmp_pos = sc.len() - 5;
    let next_insn = stub_addr + jmp_pos as u64 + 5;
    let target = patch_addr + orig_len as u64;
    let rel = (target as i64 - next_insn as i64) as i32;
    sc[jmp_pos + 1..jmp_pos + 5].copy_from_slice(&rel.to_le_bytes());
}

/// 在目標行程配置記憶體並寫入
pub fn alloc_and_write(hproc: windows::Win32::Foundation::HANDLE, bytes: &[u8]) -> Result<u64, String> {
    use windows::Win32::System::Memory::{
        VirtualAllocEx, VirtualProtectEx, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE,
        PAGE_PROTECTION_FLAGS,
    };

    let h = hproc;
    let size = bytes.len();
    unsafe {
        let p = VirtualAllocEx(h, None, size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
        if p.is_null() {
            return Err("VirtualAllocEx 失敗".into());
        }
        let mut written = 0usize;
        windows::Win32::System::Diagnostics::Debug::WriteProcessMemory(
            h,
            p,
            bytes.as_ptr() as *const c_void,
            size,
            Some(&mut written),
        )
        .map_err(|e| format!("WriteProcessMemory 失敗：{e}"))?;
        let _ = (VirtualProtectEx, PAGE_PROTECTION_FLAGS::default());
        Ok(p as u64)
    }
}

/// 安裝 inline hook：把 patch_addr 前 n bytes 改成 jmp stub_addr
pub fn install_jmp(hproc: windows::Win32::Foundation::HANDLE, patch_addr: u64, stub_addr: u64, n_bytes: usize) -> Result<(), String> {
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::Win32::System::Memory::{
        VirtualProtectEx, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS,
    };

    let h = hproc;
    let mut jmp: Vec<u8> = vec![0xE9];
    let next = patch_addr + n_bytes as u64;
    let rel = (stub_addr as i64 - next as i64) as i32;
    jmp.extend_from_slice(&rel.to_le_bytes());
    while jmp.len() < n_bytes {
        jmp.push(0x90); // nop padding
    }

    unsafe {
        let mut old = PAGE_PROTECTION_FLAGS::default();
        let _ = VirtualProtectEx(h, patch_addr as *const c_void, n_bytes, PAGE_EXECUTE_READWRITE, &mut old);
        let mut written = 0usize;
        WriteProcessMemory(
            h,
            patch_addr as *const c_void,
            jmp.as_ptr() as *const c_void,
            n_bytes,
            Some(&mut written),
        )
        .map_err(|e| format!("install_jmp 寫入失敗：{e}"))?;
        let _ = VirtualProtectEx(h, patch_addr as *const c_void, n_bytes, old, &mut old);
    }
    Ok(())
}