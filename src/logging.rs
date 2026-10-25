//! 日誌：環形緩衝 + 檔案落地 + UI 廣播
//!
//! 兩個檔：
//!   launcher.log   一般執行紀錄（append）
//!   error.log      只收 Warn / Error，供使用者一鍵匯出 TXT 排查

use eframe::egui;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lvl {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub ts: String,
    pub level: Lvl,
    pub msg: String,
}

impl std::fmt::Display for LogLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = match self.level {
            Lvl::Info => "INFO ",
            Lvl::Warn => "WARN ",
            Lvl::Error => "ERROR",
        };
        write!(f, "{} {} {}", self.ts, tag, self.msg)
    }
}

#[derive(Clone)]
pub struct LogHandle {
    pub watch: Arc<Mutex<Vec<LogLine>>>,
    pub buf: Arc<Mutex<VecDeque<LogLine>>>,
    pub file: Arc<Mutex<Option<PathBuf>>>,
    pub err_file: Arc<Mutex<Option<PathBuf>>>,
    pub cap: usize,
}

impl LogHandle {
    pub fn new() -> Self {
        Self {
            watch: Arc::new(Mutex::new(Vec::new())),
            buf: Arc::new(Mutex::new(VecDeque::with_capacity(512))),
            file: Arc::new(Mutex::new(None)),
            err_file: Arc::new(Mutex::new(None)),
            cap: 5000,
        }
    }

    pub fn set_dir(&self, dir: PathBuf) {
        if let Ok(mut g) = self.file.lock() {
            *g = Some(dir.join("launcher.log"));
        }
        if let Ok(mut g) = self.err_file.lock() {
            *g = Some(dir.join("error.log"));
        }
    }

    pub fn log(&self, level: Lvl, msg: impl Into<String>) {
        let line = LogLine {
            ts: now_ts(),
            level,
            msg: msg.into(),
        };
        if let Ok(mut b) = self.buf.lock() {
            if b.len() >= self.cap {
                b.pop_front();
            }
            b.push_back(line.clone());
        }
        self.write_line(&self.file, &line);
        if matches!(level, Lvl::Warn | Lvl::Error) {
            self.write_line(&self.err_file, &line);
        }
        if let Ok(mut w) = self.watch.lock() {
            w.push(line);
        }
        ctx_repaint();
    }

    fn write_line(&self, slot: &Arc<Mutex<Option<PathBuf>>>, line: &LogLine) {
        if let Ok(g) = slot.lock() {
            if let Some(p) = g.as_ref() {
                if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(p) {
                    let _ = writeln!(f, "{line}");
                }
            }
        }
    }

    pub fn info(&self, m: impl Into<String>) {
        self.log(Lvl::Info, m)
    }
    pub fn warn(&self, m: impl Into<String>) {
        self.log(Lvl::Warn, m)
    }
    pub fn error(&self, m: impl Into<String>) {
        self.log(Lvl::Error, m)
    }

    #[allow(dead_code)]
    pub fn export(&self) -> String {
        let mut s = String::new();
        if let Ok(b) = self.buf.lock() {
            for l in b.iter() {
                let _ = writeln!(s, "{l}");
            }
        }
        s
    }

    /// 只匯出錯誤（給「匯出錯誤 LOG」用）
    pub fn export_errors(&self) -> (String, usize) {
        let mut s = String::new();
        let mut n = 0usize;
        if let Ok(b) = self.buf.lock() {
            for l in b.iter() {
                if matches!(l.level, Lvl::Warn | Lvl::Error) {
                    let _ = writeln!(s, "{l}");
                    n += 1;
                }
            }
        }
        (s, n)
    }

    pub fn drain(&self) -> Vec<LogLine> {
        if let Ok(mut w) = self.watch.lock() {
            std::mem::take(&mut *w)
        } else {
            Vec::new()
        }
    }
}

static REPAINTER: Mutex<Option<egui::Context>> = Mutex::new(None);

pub fn set_repaint_ctx(ctx: egui::Context) {
    if let Ok(mut g) = REPAINTER.lock() {
        *g = Some(ctx);
    }
}

fn ctx_repaint() {
    if let Ok(g) = REPAINTER.lock() {
        if let Some(c) = g.as_ref() {
            c.request_repaint();
        }
    }
}

pub fn now_ts() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    unsafe {
        let st = GetLocalTime();
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond
        )
    }
}
