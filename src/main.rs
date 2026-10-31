//! RHD 大叔的回憶天堂 —— 自製登入器
//!
//! 入口：提權檢查 → 讀設定 → 建 runtime → 開視窗

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[allow(dead_code)]
mod autologin;
mod cfgfile;
mod config;
mod elevate;
mod hosts;
mod inproc;
mod launch;
mod logging;
mod relay;
mod servers;
mod ui;
mod updater;

use logging::LogHandle;

/// 建置標記：寫進啟動日誌，用於區分部署的 exe 版本（凍結取證對照用）。
pub const BUILD_TAG: &str = "20261031-heartbeat1";

fn main() {
    // 自檢模式：不開視窗，驗證 relay / hosts / cfg 讀寫
    if std::env::args().any(|a| a == "--selftest") {
        selftest();
        return;
    }

    // 常駐中繼模式：由 UI 版啟動後獨立存活，讓 UI 可以退出但中繼不斷線
    if std::env::args().any(|a| a == "--relay-daemon") {
        relay_daemon_mode();
        return;
    }

    // 舊啟動入口已停用，避免既有腳本誤啟動客戶端。
    if std::env::args().any(|a| a == "--login-host") {
        eprintln!("舊登入入口已停用，遊戲啟動等待安全驗證。");
        std::process::exit(1);
    }

    let log = LogHandle::new();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    log.set_dir(exe_dir.clone());
    log.info("═══ RHD Launcher 啟動 ═══");
    log.info(format!("工作目錄 = {}", exe_dir.display()));
    log.info(format!("管理員權限 = {}", elevate::is_admin()));
    log.info(format!("build = {BUILD_TAG}"));
    log_freeze_risk(&log);

    let cfg = config::load();
    relay::apply_config(
        cfg.packet_capture,
        cfg.proxy_login,
        &cfg.login_account,
        &cfg.login_password,
        cfg.inject_s_key,
    );

    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([680.0, 480.0])
            .with_title("Takanashi製作"),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "Takanashi製作",
        opts,
        Box::new(move |cc| Ok(Box::new(ui::LauncherApp::new(cc, cfg, log)))),
    ) {
        eprintln!("開啟視窗失敗：{e}");
        std::process::exit(1);
    }
}





/// 常駐中繼模式：只做 TCP 中繼，不開視窗。
/// 由 UI 版以 --relay-daemon 啟動，UI 退出後此進程仍存活，
/// 確保客戶端的長連線不會因為登入器關閉而斷線。
fn relay_daemon_mode() {
    let log = LogHandle::new();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    log.set_dir(exe_dir);
    log.info("── 常駐中繼模式啟動 ──");
    log.info(format!("build = {BUILD_TAG}"));
    log_freeze_risk(&log);

    let cfg = config::load();
    relay::apply_config(
        cfg.packet_capture,
        cfg.proxy_login,
        &cfg.login_account,
        &cfg.login_password,
        cfg.inject_s_key,
    );

    let profiles = match servers::load() {
        Ok(v) => v,
        Err(e) => {
            log.error(format!("servers.ini: {e}"));
            return;
        }
    };
    let p = match profiles.first() {
        Some(p) => p.clone(),
        None => {
            log.error("沒有伺服器設定");
            return;
        }
    };

    // 若埠已被既有 daemon 佔用，直接退出（避免兩個 daemon 搶埠）
    let mut all_busy = true;
    for port in &p.listen_ports {
        if std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok() {
            all_busy = false;
        }
    }
    if all_busy {
        log.info("偵測到既有常駐中繼，本次不啟動");
        return;
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log.error(format!("建立 runtime 失敗：{e}"));
            return;
        }
    };

    match relay::start(
        "127.0.0.1",
        &p.listen_ports,
        &p.host,
        p.port,
        log.clone(),
        rt.handle(),
    ) {
        Ok(_h) => {
            log.info(format!("常駐中繼已就緒 -> {}", p.host_port()));
            // 心跳（凍結取證）：每 10 秒記錄一次存活，凍結後重建時可把
            // 系統凍結窗口收斂到 10 秒級（2026-10-31 採證筆記 §3.1-4）
            log.info("心跳啟用：每 10 秒記錄常駐中繼存活");
            let started = std::time::Instant::now();
            let mut next_beat = 10u64;
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let el = started.elapsed().as_secs();
                if el >= next_beat {
                    log.info(format!("心跳：常駐中繼存活 t={el}s"));
                    next_beat = el + 10;
                }
                // 目標伺服器若消失也不退出，等它回來
            }
        }
        Err(e) => log.error(format!("常駐中繼啟動失敗：{e}")),
    }
}

/// 啟動期凍結風險掃描（只讀，不改任何狀態）。
/// 三嫌疑採證見 vault 筆記 20261031-天R登入器B4當機採證與歐霸天堂Login.exe反編譯取證 §1.3：
/// ①IObit AscFileFilter 篩選驅動 ②SecureEngine/GG × 行程內補丁並行 ③D: 滿盤（僅限有 NTFS 140 時）。
fn log_freeze_risk(log: &LogHandle) {
    log.info(format!(
        "凍結風險掃描：AscFileFilter={} D_free={}（嫌疑1=AscFileFilter 2=SE/GG×補丁 3=D:滿盤[僅限NTFS140]）",
        ascfilefilter_state(),
        d_free_gb()
            .map(|g| format!("{g} GB"))
            .unwrap_or_else(|| "?".to_string()),
    ));
}

/// 查 IObit Advanced SystemCare 的 AscFileFilter 服務狀態。
/// sc.exe 的 STATE 名稱各語系皆為英文（RUNNING/STOPPED）。
fn ascfilefilter_state() -> &'static str {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("sc.exe")
        .args(["query", "AscFileFilter"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            if s.contains("RUNNING") {
                "RUNNING"
            } else if s.contains("STOPPED") {
                "STOPPED"
            } else {
                "absent"
            }
        }
        Err(_) => "unknown",
    }
}

/// 查 D: 可用空間（GB）。槽不存在或查詢失敗回 None。
fn d_free_gb() -> Option<u64> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let root: Vec<u16> = "D:\\\0".encode_utf16().collect();
    let mut free: u64 = 0;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), None, None, Some(&mut free))
            .ok()
            .map(|_| free / 1024 / 1024 / 1024)
    }
}

/// 自檢：驗證 relay 綁定與轉發、hosts 狀態、servers.ini、lineage.cfg 讀取。
fn selftest() {
    let log = LogHandle::new();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    log.set_dir(exe_dir.clone());
    log.info("=== SELFTEST START ===");
    log.info(format!("admin={}", elevate::is_admin()));
    log.info(format!("hosts_blocked={}", hosts::is_blocked()));

    match servers::load() {
        Ok(v) => {
            log.info(format!("servers.ini 讀到 {} 個伺服器", v.len()));
            for s in &v {
                log.info(format!(
                    "  [{}] {} listen={:?} game_dir={}",
                    s.name,
                    s.host_port(),
                    s.listen_ports,
                    s.game_dir
                ));
            }
        }
        Err(e) => log.error(format!("servers.ini: {e}")),
    }

    let cfg = config::load();
    log.info(format!("updater={:?}", cfg.updater));
    if let Ok(v) = servers::load() {
        if let Some(s) = v.first() {
            match cfgfile::read(&s.game_dir) {
                Ok((c, _)) => log.info(format!(
                    "lineage.cfg ip={:?} tag08={:?} fields={}",
                    c.ip(),
                    c.tag08(),
                    c.fields.len()
                )),
                Err(e) => log.error(format!("lineage.cfg: {e}")),
            }
            log.info(format!(
                "target {} alive={}",
                s.host_port(),
                launch::target_alive(&s.host, s.port, std::time::Duration::from_secs(2))
            ));
            log.info(format!(
                "client_proc={:?}",
                launch::client_process_running(&s.game_dir)
            ));
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    let echo_port = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::AsyncWriteExt;
                    let (mut r, mut w) = s.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                    let _ = w.shutdown().await;
                });
            }
        });
        p
    });

    match relay::start(
        "127.0.0.1",
        &[41888],
        "127.0.0.1",
        echo_port,
        log.clone(),
        rt.handle(),
    ) {
        Ok(mut h) => {
            log.info(format!("relay bound={:?}", h.bound));
            let r = rt.block_on(async {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut c = tokio::net::TcpStream::connect("127.0.0.1:41888").await?;
                c.write_all(b"PING-RELAY").await?;
                let mut buf = [0u8; 16];
                let n = c.read(&mut buf).await?;
                Ok::<_, std::io::Error>(String::from_utf8_lossy(&buf[..n]).to_string())
            });
            log.info(format!("relay echo result = {:?}", r));
            h.stop();
        }
        Err(e) => log.error(format!("relay start failed: {e}")),
    }
    log.info("=== SELFTEST END ===");
}
