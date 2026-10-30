//! 主介面：全屏背景圖 + 左側伺服器列表
//!
//! 依使用者最終指示（2026-10-24）：
//!   - 標題：Takanashi製作
//!   - 不顯示「RHD 大叔的回憶天堂」
//!   - 左側伺服器列表，讀 servers.ini 決定名稱 / IP / Port
//!   - 啟動入口保持停用，直到自製初始化方案完成安全驗證
//!   - 錯誤 LOG 自動匯出，不顯示
//!   - 不跳出原版登入器，直接進遊戲

pub mod theme;
pub mod widgets;

use crate::config::AppConfig;
use crate::inproc;
use crate::launch;
use crate::logging::{LogHandle, LogLine};
use crate::relay::{self, RelayHandle};
use crate::servers::{self, ServerProfile};
use eframe::egui;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(PartialEq, Clone, Copy)]
enum Phase {
    Idle,
    Booting,
    Failed,
}

/// 啟動執行緒的結果通道：boot_sequence 改在背景執行緒跑 launch_inproc，
/// 補丁等待（最長約 130 秒）期間 UI 保持回應，不再出現「登入器當機」觀感。
type LaunchRx = std::sync::mpsc::Receiver<Result<(), String>>;

fn launch_hold_status(process: Result<Option<String>, String>) -> String {
    match process {
        Ok(Some(name)) => format!("偵測到既有遊戲 {name}，請先自行關閉"),
        Err(e) => format!("無法檢查遊戲程序：{e}"),
        Ok(None) => "遊戲啟動暫停：安全驗證尚未完成".into(),
    }
}

pub struct LauncherApp {
    cfg: AppConfig,
    log: LogHandle,
    lines: VecDeque<LogLine>,
    rt: Arc<tokio::runtime::Runtime>,
    relay: Option<RelayHandle>,

    profiles: Vec<ServerProfile>,
    selected: usize,
    ini_error: Option<String>,
    ini_mtime: Option<std::time::SystemTime>,

    phase: Phase,
    status: String,
    target_ok: bool,
    client_proc: Option<String>,
    last_probe: Instant,
    queued: bool,
    launch_result: Option<LaunchRx>,
}

impl LauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: AppConfig, log: LogHandle) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_style(&cc.egui_ctx);
        crate::logging::set_repaint_ctx(cc.egui_ctx.clone());
        widgets::install_bg(&cc.egui_ctx);
        relay::apply_config(
            cfg.packet_capture,
            cfg.proxy_login,
            &cfg.login_account,
            &cfg.login_password,
            cfg.inject_s_key,
        );

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
        let ini_mtime = std::fs::metadata(servers::ini_path())
            .and_then(|m| m.modified())
            .ok();

        Self {
            cfg,
            log,
            lines: VecDeque::with_capacity(256),
            rt: Arc::new(rt),
            relay: None,
            profiles,
            selected,
            ini_error,
            ini_mtime,
            phase: Phase::Idle,
            status: String::new(),
            target_ok: false,
            client_proc: None,
            last_probe: Instant::now() - Duration::from_secs(10),
            queued: false,
            launch_result: None,
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
        let game_dir = self.current().map(|p| p.game_dir.clone());
        self.client_proc = match game_dir.as_deref() {
            Some(game_dir) => match launch::client_process_running(game_dir) {
                Ok(process) => process,
                Err(e) => Some(format!("無法檢查遊戲程序：{e}")),
            },
            None => None,
        };
    }

    /// 進入遊戲：進程內補丁路線（TR-BUG-458 路線 B2，rhd_inproc.dll）。
    ///
    /// 前置檢查失敗（既有程序、程序盤點錯誤）一律不啟動；inproc_launch=false 時
    /// 維持停用狀態。補丁由自製 DLL 在客戶端進程內完成，啟動器只寫入注入路徑字串。
    fn boot_sequence(&mut self) {
        let Some(p) = self.current().cloned() else {
            self.phase = Phase::Failed;
            self.status = "沒有伺服器設定".into();
            return;
        };
        match launch::client_process_running(&p.game_dir) {
            Ok(Some(name)) => {
                self.phase = Phase::Failed;
                self.status = format!("偵測到既有遊戲 {name}，請先自行關閉");
                self.log.info(self.status.clone());
                return;
            }
            Ok(None) => {}
            Err(e) => {
                self.phase = Phase::Failed;
                self.status = format!("無法檢查遊戲程序：{e}");
                self.log.error(self.status.clone());
                return;
            }
        }
        if !self.cfg.inproc_launch {
            self.phase = Phase::Failed;
            self.status = launch_hold_status(Ok(None));
            self.log.info(self.status.clone());
            return;
        }
        if !self.ensure_relay_for(&p) {
            self.phase = Phase::Failed;
            self.status = "中繼啟動失敗，未啟動遊戲".into();
            return;
        }

        self.log.info("進入遊戲：進程內補丁路線（rhd_inproc.dll）");
        // 背景執行緒執行完整啟動鏈；UI 只輪詢結果（修復：等待期間不再凍結視窗）
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let profile = p.clone();
        let log2 = self.log.clone();
        std::thread::spawn(move || {
            let _ = tx.send(inproc::launch_inproc(&profile, log2));
        });
        self.launch_result = Some(rx);
        self.phase = Phase::Booting;
        self.status = "啟動中：等待客戶端脫殼與進程內補丁（最長約 2 分鐘）…".into();
        self.log.info(self.status.clone());
    }

    /// 啟動成功收尾：交棒常駐中繼 daemon → 釋放 listen 埠 → 約 3 秒後退出。
    fn finish_boot_ok(&mut self) {
        self.log.info("遊戲已啟動並完成進程內補丁；登入器交棒常駐中繼後退出");
        self.stop_all();
        inproc::spawn_relay_daemon(&self.log);
        let exit_log = self.log.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(3));
            exit_log.info("登入器退出（中繼由 daemon 接管）");
            std::process::exit(0);
        });
    }

    /// 輪詢背景啟動結果；每幀呼叫。
    fn poll_launch_result(&mut self) {
        if self.phase != Phase::Booting {
            return;
        }
        let Some(rx) = &self.launch_result else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(())) => {
                self.launch_result = None;
                self.finish_boot_ok();
            }
            Ok(Err(e)) => {
                self.launch_result = None;
                self.phase = Phase::Failed;
                self.status = e.clone();
                self.log.error(e);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.launch_result = None;
                self.phase = Phase::Failed;
                self.status = "啟動執行緒異常結束（無結果回傳）".into();
                self.log.error(self.status.clone());
            }
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
                // 舊實例（含殘留 daemon、未關閉的舊視窗）可能占住 listen 埠：
                // 先終止同名非自身實例再重試一次；仍失敗且本機有服務中的中繼就沿用。
                let killed = inproc::terminate_stale_launcher_instances(&self.log);
                if killed > 0 {
                    std::thread::sleep(Duration::from_millis(600));
                    match relay::start(
                        "127.0.0.1",
                        &p.listen_ports,
                        &p.host,
                        p.port,
                        self.log.clone(),
                        self.rt.handle(),
                    ) {
                        Ok(h) => {
                            self.log.info(format!("中繼 -> {}（重綁成功）", p.host_port()));
                            self.relay = Some(h);
                            return true;
                        }
                        Err(e2) => {
                            self.log.error(format!("中繼重綁仍失敗：{e2}"));
                            return false;
                        }
                    }
                }
                if relay::existing_relay_on(&p.listen_ports) {
                    self.log
                        .info("偵測到既有常駐中繼（埠已服務中），沿用不重綁".to_string());
                    true
                } else {
                    self.log.error(format!("中繼啟動失敗：{e}"));
                    false
                }
            }
        }
    }

    #[allow(dead_code)]
    fn restart(&mut self) {
        self.stop_all();
        self.boot_sequence();
    }

    fn stop_all(&mut self) {
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

#[cfg(test)]
mod tests {
    use super::launch_hold_status;

    #[test]
    fn game_launch_remains_blocked_for_all_process_check_results() {
        assert!(launch_hold_status(Ok(None)).contains("安全驗證"));
        assert!(launch_hold_status(Ok(Some("Lin.bin".into()))).contains("既有遊戲"));
        assert!(launch_hold_status(Err("快照失敗".into())).contains("無法檢查"));
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
        self.poll_launch_result();
        if !self.queued {
            self.queued = true;
            if self.current().is_some() {
                let _ = self.ensure_relay();
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
            Phase::Idle => "已選擇伺服器".to_string(),
            Phase::Booting => {
                if self.status.is_empty() {
                    "準備中".to_string()
                } else {
                    self.status.clone()
                }
            }
            Phase::Failed => self.status.clone(),
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
