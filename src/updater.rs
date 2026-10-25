//! 自動更新與吃檔：呼叫既有 Python 更新腳本並回報結果。
//!
//! 更新腳本路徑寫在 servers.ini 的 updater=（選填）；未設定就跳過。
//! 吃檔＝把更新下載的資源套進客戶端（沿用既有 Python 工具，不重寫）。
//!
//! 設計原則：更新器本身不是我們的主要交付物，這裡只做「呼叫 + 收集輸出 + 錯誤進 error.log」，
//! 避免重複造輪子，也避免改壞既有可用流程。

use crate::logging::LogHandle;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Debug)]
pub struct UpdaterConfig {
    /// 更新腳本（.bat 或 .py）完整路徑；空字串 = 未設定
    pub updater: String,
    /// 吃檔腳本完整路徑；空字串 = 與更新器相同
    pub apply_patch: String,
    /// 是否在更新後自動吃檔
    pub auto_apply: bool,
}

impl Default for UpdaterConfig {
    fn default() -> Self {
        Self {
            updater: String::new(),
            apply_patch: String::new(),
            auto_apply: true,
        }
    }
}

impl UpdaterConfig {
    pub fn configured(&self) -> bool {
        !self.updater.trim().is_empty()
    }
}

/// 執行一次更新／吃檔，回傳是否成功。
pub fn run_script(script: &str, workdir: &str, log: &LogHandle, tag: &str) -> bool {
    let script = script.trim();
    if script.is_empty() {
        log.warn(format!("[{tag}] 未設定腳本，跳過"));
        return false;
    }
    let sp = Path::new(script);
    if !sp.exists() {
        log.error(format!("[{tag}] 找不到腳本：{script}"));
        return false;
    }
    let ext = sp
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut cmd = if ext == "py" {
        let mut c = Command::new("python");
        c.arg(sp);
        c
    } else {
        let mut c = Command::new("cmd");
        c.arg("/c").arg(sp);
        c
    };

    let wd = PathBuf::from(workdir);
    if wd.is_dir() {
        cmd.current_dir(wd);
    }

    log.info(format!("[{tag}] 執行 {script}"));
    let out = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags_no_window()
        .output();

    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            for l in stdout.lines().take(200) {
                log.info(format!("[{tag}] {l}"));
            }
            for l in stderr.lines().take(200) {
                log.warn(format!("[{tag}] {l}"));
            }
            if o.status.success() {
                log.info(format!("[{tag}] 完成（exit 0）"));
                true
            } else {
                log.error(format!(
                    "[{tag}] 失敗（exit {:?}）",
                    o.status.code().unwrap_or(-1)
                ));
                false
            }
        }
        Err(e) => {
            log.error(format!("[{tag}] 無法執行：{e}"));
            false
        }
    }
}

trait NoWindow {
    fn creation_flags_no_window(&mut self) -> &mut Self;
}

impl NoWindow for Command {
    fn creation_flags_no_window(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        CommandExt::creation_flags(self, CREATE_NO_WINDOW)
    }
}
