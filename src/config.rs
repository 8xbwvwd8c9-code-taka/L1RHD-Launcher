//! launcher.json 讀寫 (企畫書 §5.2 / 附錄 C)
//!
//! 只放「程式行為」設定；IP / Port 一律在 servers.ini，帳密在遊戲內輸入。
//! 與舊版 Python 的欄位名相容，可直接沿用既有 launcher.json。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    /// 選填：更新腳本路徑（.bat/.py）；servers.ini 有 updater= 時以 ini 為準
    #[serde(default)]
    pub updater: String,
    /// 選填：吃檔腳本路徑；留空則與 updater 相同
    #[serde(default)]
    pub apply_patch: String,
    #[serde(default = "def_true")]
    pub auto_apply_after_update: bool,
    /// 內建封鎖官方清單（不對外顯示開關，正常永遠 true）
    #[serde(default = "def_true")]
    pub block_official_list: bool,
    #[serde(default = "def_true")]
    pub minimize_to_tray: bool,
    /// 上次選到的伺服器名稱（servers.ini 的段名）
    #[serde(default)]
    pub last_server: String,
    /// 進程內補丁路線（rhd_inproc.dll；TR-BUG-458 路線 B2）。
    /// false = 維持「安全驗證未完成」的停用狀態，不啟動遊戲。
    #[serde(default = "def_true")]
    pub inproc_launch: bool,
}

fn def_true() -> bool {
    true
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            updater: String::new(),
            apply_patch: String::new(),
            auto_apply_after_update: true,
            block_official_list: true,
            minimize_to_tray: true,
            last_server: String::new(),
            inproc_launch: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inproc_launch_defaults_to_enabled() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.inproc_launch);
        let off: AppConfig = serde_json::from_str(r#"{"inproc_launch": false}"#).unwrap();
        assert!(!off.inproc_launch);
    }
}

pub fn parse_ports(s: &str) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    for tok in s.split(|c| c == ',' || c == ';' || c == ' ' || c == '\t') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        if let Ok(p) = tok.parse::<u32>() {
            if (1..=65535).contains(&p) {
                let p = p as u16;
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

pub fn config_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("launcher.json")
}

pub fn load() -> AppConfig {
    match std::fs::read_to_string(config_path()) {
        Ok(s) => serde_json::from_str::<AppConfig>(&s).unwrap_or_default(),
        Err(_) => {
            let d = AppConfig::default();
            let _ = save(&d);
            d
        }
    }
}

pub fn save(cfg: &AppConfig) -> std::io::Result<()> {
    let s = serde_json::to_string_pretty(cfg).unwrap_or_else(|_| "{}".into());
    std::fs::write(config_path(), s.as_bytes())
}
