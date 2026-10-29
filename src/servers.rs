//! 伺服器清單：IP / Port / 名稱全部由外部 INI 決定
//!
//! servers.ini 每一段 = 一個伺服器，段名就是左側清單顯示的名稱。
//! 使用者只要改 INI，UI 清單、IP、Port 全部跟著變，程式內不寫死任何位址。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct ServerProfile {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub listen_ports: Vec<u16>,
    pub game_dir: String,
}

impl ServerProfile {
    pub fn host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub fn ini_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent()
        .unwrap_or_else(|| Path::new("."))
        .join("servers.ini")
}

/// 登入器自身所在目錄（= 預設遊戲根目錄）
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// game_dir 正規化，所有路徑以登入器所在目錄為基準：
///   1. 空 / 未寫            → exe 所在目錄（登入器直接放遊戲根目錄即可）
///   2. 相對路徑（如 `..`）   → 以 exe 所在目錄為基準解析
///   3. 舊設定的絕對路徑    → 退回登入器所在目錄
fn normalize_game_dir(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return exe_dir().to_string_lossy().to_string();
    }
    let p = Path::new(raw);
    if p.components().any(|part| {
        matches!(
            part,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        )
    }) {
        return exe_dir().to_string_lossy().to_string();
    }
    exe_dir()
        .join(p)
        .canonicalize()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|_| exe_dir().join(p).to_string_lossy().to_string())
}

pub fn example_ini() -> String {
    [
        "; Takanashi製作 —— 伺服器清單（UTF-8）",
        "; 每段 = 一個伺服器；段名就是左側清單顯示的名稱",
        "; 改這個檔就能換 IP / Port / 名稱，登入器不用改",
        "; game_dir：留空＝登入器所在目錄；相對路徑以登入器所在目錄為基準",
        "",
        "[黑皮天堂]",
        "host=127.0.0.1",
        "port=3000",
        "listen=2000,29467",
        "game_dir=",
        "",
    ]
    .join("\r\n")
}

pub fn load() -> Result<Vec<ServerProfile>, String> {
    let path = ini_path();
    if !path.exists() {
        let _ = std::fs::write(&path, example_ini());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("讀取 {} 失敗：{e}", path.display()))?;

    let mut out: Vec<ServerProfile> = Vec::new();
    let mut cur: Option<ServerProfile> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(s) = cur.take() {
                out.push(s);
            }
            cur = Some(ServerProfile {
                name: line[1..line.len() - 1].trim().to_string(),
                host: "127.0.0.1".into(),
                port: 3000,
                listen_ports: vec![2000, 29467],
                // 預設＝登入器所在目錄（normalize_game_dir 會再正規化）
                game_dir: String::new(),
            });
            continue;
        }
        let Some(s) = cur.as_mut() else { continue };
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim();
        match k.as_str() {
            "host" | "ip" => s.host = v.to_string(),
            "port" => {
                if let Ok(n) = v.parse::<u16>() {
                    s.port = n;
                }
            }
            "listen" | "listen_ports" => s.listen_ports = crate::config::parse_ports(v),
            "game_dir" | "gamedir" => s.game_dir = v.to_string(),
            _ => {}
        }
    }
    if let Some(s) = cur.take() {
        out.push(s);
    }

    // 收尾統一正規化 game_dir（不鎖死絕對路徑）
    for s in out.iter_mut() {
        s.game_dir = normalize_game_dir(&s.game_dir);
    }

    if out.is_empty() {
        return Err(format!("{} 裡沒有任何 [伺服器名稱] 段落", path.display()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_dir_is_anchored_to_launcher_directory() {
        let base = exe_dir();
        assert_eq!(PathBuf::from(normalize_game_dir("")), base);
        assert_eq!(PathBuf::from(normalize_game_dir(".")), base.canonicalize().unwrap());
        assert_eq!(
            PathBuf::from(normalize_game_dir(&base.to_string_lossy())),
            base
        );
        let rooted = format!("{}other", std::path::MAIN_SEPARATOR);
        assert_eq!(PathBuf::from(normalize_game_dir(&rooted)), base);
        if let Some(std::path::Component::Prefix(prefix)) = base.components().next() {
            let drive_relative = format!("{}other", prefix.as_os_str().to_string_lossy());
            assert_eq!(PathBuf::from(normalize_game_dir(&drive_relative)), base);
        }
    }
}
