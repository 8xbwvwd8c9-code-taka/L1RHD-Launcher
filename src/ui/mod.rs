//! 主介面：全屏背景圖 + 左側伺服器列表
//!
//! 依使用者最終指示（2026-10-24）：
//!   - 標題：RHDLauncher
//!   - 不顯示個人品牌名稱
//!   - 左側伺服器列表，讀 servers.ini 決定名稱 / IP / Port
//!   - 沒有任何按鈕；背景自動更新 → 吃檔 → 封鎖 → 中繼 → 背景接管 Login.exe
//!   - 錯誤 LOG 自動匯出，不顯示
//!   - 不跳出原版登入器，直接進遊戲

pub mod theme;
pub mod widgets;

use crate::config::AppConfig;
use crate::hosts;
use crate::launch;
use crate::loginhost::HiddenLogin;
use crate::logging::{LogHandle, LogLine};
use crate::relay::{self, RelayHandle};
use crate::servers::{self, ServerProfile};
use crate::updater::UpdaterConfig;
use eframe::egui;
use std::collections::VecDeque;
use std::os::windows::process::CommandExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Idle,
    Booting,
    Updating,
    Launching,
    Running,
    Failed,
}

pub struct LauncherApp {
    cfg: AppConfig,
    log: LogHandle,
    lines: VecDeque<LogLine>,
    rt: Arc<tokio::runtime::Runtime>,
    relay: Option<RelayHandle>,
    login: Option<HiddenLogin>,
    client_pid: Option<u32>,

    profiles: Vec<ServerProfile>,
    selected: usize,
    ini_error: Option<String>,
    ini_mtime: Option<std::time::SystemTime>,
    upd: UpdaterConfig,

    phase: Phase,
    status: String,
    target_ok: bool,
    client_proc: Option<String>,
    last_probe: Instant,
    queued: bool,
    /// 交接成功後的自動退出計時（None = 不退出）
    exit_deadline: Option<Instant>,
}

impl LauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: AppConfig, log: LogHandle) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_style(&cc.egui_ctx);
        crate::logging::set_repaint_ctx(cc.egui_ctx.clone());
        widgets::install_bg(&cc.egui_ctx);

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("rhd-relay")
            .build()
            .expect("建立 tokio runtime 失敗");

        let (profiles, ini_error) = match servers::load() {
            Ok(v) => (v, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        let selected = profiles
            .iter()
            .position(|p| p.name == cfg.last_server)
            .unwrap_or(0);
        let upd = UpdaterConfig {
            updater: cfg.updater.clone(),
            apply_patch: cfg.apply_patch.clone(),
            auto_apply: cfg.auto_apply_after_update,
        };
        let ini_mtime = std::fs::metadata(servers::ini_path())
            .and_then(|m| m.modified())
            .ok();

        Self {
            cfg,
            log,
            lines: VecDeque::with_capacity(256),
            rt: Arc::new(rt),
            relay: None,
            login: None,
            client_pid: None,
            profiles,
            selected,
            ini_error,
            ini_mtime,
            upd,
            phase: Phase::Idle,
            status: String::new(),
            target_ok: false,
            client_proc: None,
            last_probe: Instant::now() - Duration::from_secs(10),
            queued: false,
            exit_deadline: None,
        }
    }

    fn current(&self) -> Option<&ServerProfile> {
        self.profiles.get(self.selected)
    }

    fn reload_if_changed(&mut self) {
        let m = std::fs::metadata(servers::ini_path())
            .and_then(|x| x.modified())
            .ok();
        if m == self.ini_mtime {
            return;
        }
        self.ini_mtime = m;
        match servers::load() {
            Ok(v) => {
                let keep = self.current().map(|p| p.name.clone());
                self.profiles = v;
                self.selected = keep
                    .and_then(|n| self.profiles.iter().position(|p| p.name == n))
                    .unwrap_or(0);
                self.ini_error = None;
            }
            Err(e) => self.ini_error = Some(e),
        }
    }

    fn probe(&mut self) {
        if self.last_probe.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_probe = Instant::now();
        if let Some(p) = self.current() {
            let (h, port) = (p.host.clone(), p.port);
            self.target_ok = launch::target_alive(&h, port, Duration::from_millis(500));
        }
        self.client_proc = launch::client_process_running();
    }

    /// 背景流程：封鎖 → 更新 → 吃檔 → 中繼 → 背景接管 Login.exe
    fn boot_sequence(&mut self) {
        let Some(p) = self.current().cloned() else {
            self.phase = Phase::Failed;
            self.status = "servers.ini 沒有可用伺服器".into();
            return;
        };
        self.phase = Phase::Launching;
        self.status = "啟動中".into();
        self.cfg.last_server = p.name.clone();
        let _ = crate::config::save(&self.cfg);

        // 1. 內建封鎖官方清單（無管理員時會失敗——僅路線 A 需要，B1 降級為警告）
        if self.cfg.block_official_list && !hosts::is_blocked() {
            let domains: Vec<String> =
                hosts::BUILTIN_BLOCKED.iter().map(|s| s.to_string()).collect();
            if let Err(e) = hosts::block(&domains) {
                self.log.warn(format!(
                    "hosts 封鎖失敗（{e}）——非管理員執行時屬預期；B1 直啟模式不需要此封鎖，路線 A 建議右鍵以管理員執行"
                ));
            }
        }

        // 2. 自動更新 + 吃檔（強制，無按鈕）
        if self.upd.configured() {
            self.phase = Phase::Updating;
            self.status = "更新中".into();
            let ok = crate::updater::run_script(&self.upd.updater, &p.game_dir, &self.log, "更新");
            if ok && self.upd.auto_apply {
                let patch = if self.upd.apply_patch.trim().is_empty() {
                    self.upd.updater.clone()
                } else {
                    self.upd.apply_patch.clone()
                };
                crate::updater::run_script(&patch, &p.game_dir, &self.log, "吃檔");
            }
        }

        // 3. 中繼：改由獨立 daemon 提供（UI 退出後仍存活）
        self.spawn_relay_daemon();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        if true {
            // 4. 啟動原版 Login.exe（已實測唯一能過 GameGuard 並完成交接的路徑）
            //    依據：WORKING_SETUP.md 第一節必要條件 #4
            //    防重複：若客戶端已在執行，先終止舊的再啟動，避免兩份 Login.exe 搶連線
        if let Some(name) = crate::launch::client_process_running() {
            self.log.warn(format!("偵測到既有客戶端 {name}，先行終止以免衝突"));
            // 安全性：不用 /T 樹殺——SecureEngine 保護的行程被強制樹狀終止
            // 有內核層風險（2026-10-25 當機排查，見筆記 §14）；僅終止主行程
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "LogonX64.bin"])
                .creation_flags(0x0800_0000)
                .status();
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
            // B1 直啟分流（direct_launch=true，實驗）：零黑盒 = 直接 Lin.bin + 解殼後六刀補丁
            // 詳見 patches.rs 與黑曜石《20261025-天R登入器執行期採證與LinLogin六刀補丁破解》§10.5
            if self.cfg.direct_launch {
                match crate::inject::launch_b1(&p.game_dir, self.log.clone()) {
                    Ok(pid) => {
                        self.log.info(format!("客戶端已啟動 pid {pid}（B1 直啟+六刀補丁）"));
                        self.client_pid = Some(pid);
                        self.phase = Phase::Running;
                        self.status = "已進入遊戲（B1）".into();
                        // B1 不自我退出：補丁執行緒在本進程內，退出會殺掉它
                        // （實測 2026-10-25：自我退出曾把補丁執行緒帶走 → 六刀未寫 → 380）
                    }
                    Err(e) => {
                        self.log.error(format!("B1 啟動失敗：{e}"));
                        self.phase = Phase::Failed;
                        self.status = "啟動失敗".into();
                    }
                }
                return;
            }
            let login_exe = std::path::Path::new(&p.game_dir).join(&p.login_exe);
            let child = std::process::Command::new(&login_exe)
                .current_dir(&p.game_dir)
                .creation_flags(0x0000_0000)
                .spawn();
            match child.map(|c| c.id()).map_err(|e| e.to_string()) {
                Ok(pid) => {
                    self.log.info(format!("客戶端已啟動 pid {pid}（含 connect hook）"));
                    self.client_pid = Some(pid);
                    self.phase = Phase::Running;
                    self.status = "已進入遊戲".into();
                    // 交接成功後自我退出，避免登入器留在背景佔用資源
                    // 中繼改由獨立的 relay 進程接管（spawn_relay_daemon）
                    self.exit_deadline = Some(std::time::Instant::now());
                }
                Err(e) => {
                    self.log.error(format!("啟動失敗：{e}"));
                    self.phase = Phase::Failed;
                    self.status = "啟動失敗".into();
                }
            }
        } else {
            self.phase = Phase::Failed;
            self.status = "連線失敗".into();
        }
    }

    /// 啟動獨立 relay 進程（--relay-daemon），UI 退出後中繼由它持續提供
    ///
    /// 會先檢查埠是否已被既有 daemon 佔用：
    /// - 已佔用 → 直接沿用，不重複啟動
    /// - 未佔用 → 啟動新 daemon
    fn spawn_relay_daemon(&mut self) {
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(_) => return,
        };

        // 檢查監聽埠是否已被既有 daemon 佔用
        let ports = match self.current() {
            Some(p) => p.listen_ports.clone(),
            None => return,
        };
        let mut all_busy = true;
        for port in &ports {
            let occupied = std::net::TcpListener::bind(("127.0.0.1", *port)).is_err();
            if !occupied {
                all_busy = false;
            }
        }
        if all_busy {
            self.log.info("常駐中繼已在執行，直接沿用");
            return;
        }

        match std::process::Command::new(&exe)
            .arg("--relay-daemon")
            .creation_flags(0x0000_0008) // DETACHED_PROCESS，UI 關閉不影響
            .spawn()
        {
            Ok(c) => self.log.info(format!("常駐中繼已啟動 pid {}", c.id())),
            Err(e) => self.log.warn(format!("常駐中繼啟動失敗：{e}")),
        }
    }

    fn ensure_relay_for(&mut self, p: &ServerProfile) -> bool {
        if self.relay.is_some() {
            return true;
        }
        match relay::start(
            "127.0.0.1",
            &p.listen_ports,
            &p.host,
            p.port,
            self.log.clone(),
            self.rt.handle(),
        ) {
            Ok(h) => {
                self.log.info(format!("中繼 -> {}", p.host_port()));
                self.relay = Some(h);
                true
            }
            Err(e) => {
                self.log.error(format!("中繼啟動失敗：{e}"));
                false
            }
        }
    }

    #[allow(dead_code)]
    fn restart(&mut self) {
        self.stop_all();
        self.boot_sequence();
    }

    fn stop_all(&mut self) {
        if let Some(l) = self.login.as_ref() {
            l.stop_hiding();
        }
        // 不終止 Login.exe：它與客戶端是使用者的遊戲行程，登入器關閉不應影響遊戲
        self.login = None;
        if let Some(mut h) = self.relay.take() {
            h.stop();
        }
    }

    /// 自動匯出錯誤 LOG（不顯示、不詢問）
    fn auto_export_errors(&self) {
        let (body, n) = self.log.export_errors();
        if n == 0 {
            return;
        }
        let dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|x| x.to_path_buf()))
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let name = format!(
            "error-{}.log",
            crate::logging::now_ts().replace([':', ' '], "-")
        );
        let path = dir.join(name);
        let _ = std::fs::write(&path, body.as_bytes());
    }

    /// 只選擇伺服器，不自動進遊戲（等使用者按「進入遊戲」）
    fn click_server(&mut self, idx: usize) {
        if idx == self.selected {
            return;
        }
        // 切換伺服器時，先停掉舊的連線與進程
        self.stop_all();
        self.selected = idx;
        self.phase = Phase::Idle;
        self.target_ok = false;
        self.last_probe = Instant::now() - Duration::from_secs(30);
        self.cfg.last_server = self
            .profiles
            .get(idx)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let _ = crate::config::save(&self.cfg);
        let _ = self.ensure_relay();
    }

    fn ensure_relay(&mut self) -> bool {
        let Some(p) = self.current().cloned() else {
            return false;
        };
        self.ensure_relay_for(&p)
    }
}

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for l in self.log.drain() {
            self.lines.push_back(l);
            if self.lines.len() > 3000 {
                self.lines.pop_front();
            }
        }
        self.reload_if_changed();
        self.probe();

        // 啟動時只掛上中繼，不自動進遊戲（等使用者按「進入遊戲」）
        if !self.queued {
            self.queued = true;
            if self.current().is_some() {
                let _ = self.ensure_relay();
            }
        }

        // 交接成功後自我退出（等 3 秒讓客戶端站穩）
        if let Some(t0) = self.exit_deadline {
            if t0.elapsed() > std::time::Duration::from_secs(3) {
                self.log.info("客戶端已交接，登入器自動退出");
                self.auto_export_errors();
                // 不停止中繼：常駐 daemon 已接管
                self.relay = None;
                std::process::exit(0);
            }
        }

        // 檢查 Login.exe 是否已交接退出
        if let Some(l) = self.login.as_mut() {
            if let Ok(Some(st)) = l.child.try_wait() {
                let code = st.code().unwrap_or(-1);
                self.log.info(format!("Login.exe 已交接退出（代碼 {code}）"));
                self.login = None;
            }
        }
        if self.phase == Phase::Failed {
            self.auto_export_errors();
        }

        // ---- 背景圖全屏 ----
        widgets::draw_background(ui);

        // ---- 左側伺服器列表（連線狀態燈在按鈕上） ----
        egui::Area::new(egui::Id::new("servers_area"))
            .fixed_pos(egui::pos2(18.0, 16.0))
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgba_unmultiplied(10, 12, 16, 175))
                    .corner_radius(9)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(178.0);
                        ui.label(
                            egui::RichText::new("伺服器列表")
                                .size(12.0)
                                .color(egui::Color32::from_rgb(190, 186, 178)),
                        );
                        ui.add_space(6.0);

                        if let Some(err) = self.ini_error.clone() {
                            ui.colored_label(
                                egui::Color32::from_rgb(235, 120, 120),
                                "servers.ini 讀取失敗",
                            );
                            ui.label(egui::RichText::new(err).size(9.5));
                            ui.add_space(4.0);
                        }

                        let items: Vec<String> =
                            self.profiles.iter().map(|p| p.name.clone()).collect();
                        for (i, name) in items.iter().enumerate() {
                            let sel = i == self.selected;
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(178.0, 32.0),
                                egui::Sense::click(),
                            );
                            if resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let painter = ui.painter();
                            let (bg, border, txt_color) = if sel {
                                (
                                    egui::Color32::from_rgb(228, 182, 106),
                                    egui::Color32::from_rgb(255, 228, 176),
                                    egui::Color32::from_rgb(28, 22, 14),
                                )
                            } else if resp.hovered() {
                                (
                                    egui::Color32::from_rgb(86, 88, 96),
                                    egui::Color32::from_rgb(150, 152, 160),
                                    egui::Color32::from_rgb(255, 255, 255),
                                )
                            } else {
                                (
                                    egui::Color32::from_rgb(58, 60, 68),
                                    egui::Color32::from_rgb(110, 112, 120),
                                    egui::Color32::from_rgb(228, 226, 220),
                                )
                            };
                            // 立體陰影
                            painter.rect_filled(
                                rect.translate(egui::vec2(0.0, 2.0)),
                                7.0,
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 90),
                            );
                            painter.rect_filled(rect, 7.0, bg);
                            painter.rect_stroke(
                                rect,
                                7.0,
                                egui::Stroke::new(1.2, border),
                                egui::StrokeKind::Inside,
                            );

                            // 連線狀態燈（按鈕上）
                            let dot_color = if self.target_ok {
                                egui::Color32::from_rgb(46, 204, 113)
                            } else {
                                egui::Color32::from_rgb(190, 90, 90)
                            };
                            let dot = egui::pos2(rect.left() + 15.0, rect.center().y);
                            painter.circle_filled(dot, 4.5, dot_color);

                            painter.text(
                                egui::pos2(rect.left() + 28.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                name,
                                egui::FontId::proportional(14.0),
                                txt_color,
                            );

                            if resp.clicked() {
                                self.click_server(i);
                            }
                            ui.add_space(5.0);
                        }

                        if self.profiles.is_empty() {
                            ui.label(
                                egui::RichText::new("（servers.ini 沒有項目）")
                                    .size(10.0)
                                    .color(egui::Color32::from_rgb(150, 145, 138)),
                            );
                        }
                    });
            });

        // ---- 右下角：進入遊戲按鈕（固定座標，避開 Area 錨點裁切問題） ----
        // 用中央面板可視區域計算，確保按鈕一定落在視窗內
        let view = ui.available_rect_before_wrap();
        let win_w = if view.width() > 100.0 { view.width() } else { 760.0 };
        let win_h = if view.height() > 100.0 { view.height() } else { 560.0 };
        let show_btn = self.current().is_some()
            && matches!(self.phase, Phase::Idle | Phase::Failed);

        if show_btn {
            let bw = 148.0;
            let bh = 48.0;
            let bx = win_w - bw - 24.0;
            let by = win_h - bh - 24.0;
            let child = egui::Area::new(egui::Id::new("enter_btn"))
                .fixed_pos(egui::pos2(bx, by))
                .order(egui::Order::Foreground)
                .constrain(false)
                .show(ui.ctx(), |ui| {
                    let b = egui::Button::new(
                        egui::RichText::new("進入遊戲")
                            .size(17.0)
                            .strong()
                            .color(egui::Color32::from_rgb(28, 22, 14)),
                    )
                    .fill(egui::Color32::from_rgb(228, 182, 106))
                    .corner_radius(9)
                    .min_size(egui::vec2(bw, bh));
                    ui.add(b)
                });
            if child.inner.clicked() {
                self.phase = Phase::Booting;
                self.boot_sequence();
            }
        }

        // 狀態文字
        let status_text = match self.phase {
            Phase::Idle => "已選擇伺服器",
            Phase::Booting => "準備中",
            Phase::Updating => "更新中",
            Phase::Launching => "啟動中",
            Phase::Running => "已進入遊戲",
            Phase::Failed => "失敗，詳見 error log",
        };
        let sy = if show_btn {
            win_h - 86.0
        } else {
            win_h - 28.0
        };
        egui::Area::new(egui::Id::new("status_txt"))
            .fixed_pos(egui::pos2(win_w - 200.0, sy))
            .order(egui::Order::Foreground)
            .constrain(false)
            .show(ui.ctx(), |ui| {
                ui.set_width(170.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(status_text)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(235, 230, 220)),
                    );
                });
            });
    }

    fn on_exit(&mut self) {
        self.auto_export_errors();
        self.stop_all();
    }
}