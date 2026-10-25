//! RHDLauncher — 自製登入器
//!
//! 入口：提權檢查 → 讀設定 → 建 runtime → 開視窗

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[allow(dead_code)]
mod autologin;
mod cfgfile;
mod config;
mod elevate;
mod hosts;
mod inject;
mod launch;
mod loginhost;
mod logging;
mod patches;
mod relay;
mod remotehook;
mod sockhook;
mod servers;
mod ui;
mod updater;

use logging::LogHandle;

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

    // LoginHost 模式：本程式被複製改名為 Login.exe 後，只負責「以 Login.exe 身分」啟動客戶端
    // 目的：客戶端會檢查父進程，歷史上成功時父進程就是 Login.exe（見 lin.log parentFile 14540）
    if std::env::args().any(|a| a == "--login-host") {
        login_host_mode();
        return;
    }

    // 管理員不再是必要條件（asInvoker，雙擊即開不再彈 UAC）：
    // - B1 直啟模式：hosts 封鎖可略過、補丁寫入與客戶端同權限等級即可
    // - 路線 A：缺管理員只影響 hosts 封鎖（降級為警告，功能仍可用）
    if !elevate::is_admin() {
        // 不退出；讓 UI 起來後於日誌顯示降級說明
    }

    let log = LogHandle::new();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    log.set_dir(exe_dir.clone());
    log.info("═══ RHD Launcher 啟動 ═══");
    log.info(format!("工作目錄 = {}", exe_dir.display()));
    if elevate::is_admin() {
        log.info("權限 = 管理員（hosts 封鎖可用）");
    } else {
        log.info("權限 = 一般使用者（hosts 封鎖將略過；B1 直啟不受影響）");
    }

    let cfg = config::load();

    let opts = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([760.0, 560.0])
            .with_min_inner_size([680.0, 480.0])
            .with_title("RHDLauncher"),
        ..Default::default()
    };

    if let Err(e) = eframe::run_native(
        "RHDLauncher",
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
            // 保持存活；_h 持有 listener，drop 會停止中繼
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3));
                // 目標伺服器若消失也不退出，等它回來
            }
        }
        Err(e) => log.error(format!("常駐中繼啟動失敗：{e}")),
    }
}

/// LoginHost 模式：扮演原版 Login.exe 的父進程角色
/// 用法：把 RHDLauncher.exe 複製成 Login.exe，再由真正的 launcher 以 --login-host 喚起
fn login_host_mode() {
    let log = LogHandle::new();
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    log.set_dir(exe_dir.clone());
    log.info("── LoginHost 模式啟動 ──");

    let cfg = config::load();
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

    // 啟動客戶端 + 注入 LinLogin.dll + 呼叫匯出函式
    // 關鍵：版本號 2002122605 vs 2002122601 證明是 LinLogin.dll 覆寫的
    match inject::launch_and_inject_ex(&p.game_dir, log.clone(), None) {
        Ok(pid) => log.info(format!("客戶端已啟動 pid {pid}")),
        Err(e) => {
            log.error(format!("啟動失敗：{e}"));
            return;
        }
    }

    // 保持存活，讓客戶端認得父進程
    loop {
        std::thread::sleep(std::time::Duration::from_secs(2));
        if launch::client_process_running().is_none() {
            log.info("客戶端已結束，LoginHost 退出");
            break;
        }
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
        }
    }
    log.info(format!("client_proc={:?}", launch::client_process_running()));

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
    // §10.18 connect hook 位址驗證（僅在客戶端已載入時有效）
    match sockhook::module_base_for_self() {
        Ok(base) => {
            log.info(format!("client image base = 0x{base:X}"));
            match sockhook::verify_connect_site(base) {
                Ok(addr) => log.info(format!("connect hook site 驗證通過 @ 0x{addr:X}")),
                Err(e) => log.error(format!("connect hook site 驗證失敗：{e}")),
            }
        }
        Err(e) => log.warn(format!("客戶端未載入，略過 hook 驗證：{e}")),
    }

    log.info("=== SELFTEST END ===");
}