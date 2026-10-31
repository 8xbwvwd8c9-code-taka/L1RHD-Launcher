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

// ── manifest 吃檔引擎（2026-10-31，依歐霸天堂官方更新架構簡化實作） ──
//
// 官方架構（RE 證據，見 vault 筆記 20261031 §2.5）：`Launcher/NCTinyUpdater.dll`
// （native，未加殼）以 protobuf 對帳（PlayncLauncherToUpdateServer /
// UpdateServerToPlayncLauncher）、libcurl HTTP 下載、DownloadTask→DecompressTask
// 管線套用；客戶端側完整性契約＝`Login.local`（167 條 `[data\路徑]`＋`md5=<hex>`）。
//
// 本實作以 Login.local 同格式純文字 manifest 取代 protobuf 對帳，流程同構：
// 拉遠端 manifest → 逐檔 MD5 比對 → 下載差異 → 驗 MD5 → 就地套用。
// 不改寫 `Login.local` 本體（那是 Login.exe 自己的契約）。

use md5::{Digest, Md5};
use std::io::Read;

/// manifest 條目上限（歐霸天堂 Login.local 實測 167 條；留 30 倍餘裕）
pub const MANIFEST_MAX_ENTRIES: usize = 5000;
/// 單檔下載上限（吃檔目標是 data\*.tbl/xml 等鬆散檔；500 MiB 已遠超實際）
pub const DOWNLOAD_FILE_CAP: u64 = 512 * 1024 * 1024;
/// 整輪下載累積上限
pub const DOWNLOAD_TOTAL_CAP: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct ManifestEntry {
    /// 相對於客戶端根目錄的路徑（`data\string-c.tbl`）
    pub path: String,
    /// 小寫 MD5 hex（32 字元）
    pub md5: String,
}

/// 解析 Login.local 同格式 manifest：`[相對路徑]` 行後接 `md5=<hex>` 行。
pub fn parse_manifest(text: &str) -> Result<Vec<ManifestEntry>, String> {
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() {
            continue;
        }
        if let Some(p) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            if cur.is_some() {
                return Err(format!("manifest 第 {} 行：上一個 [path] 區段缺 md5", idx + 1));
            }
            cur = Some(p.trim().to_string());
        } else if let Some(h) = line.strip_prefix("md5=") {
            let path = cur.take().ok_or_else(|| {
                format!("manifest 第 {} 行：md5= 前沒有 [path] 區段", idx + 1)
            })?;
            if path.is_empty() {
                return Err(format!("manifest 第 {} 行：空路徑", idx + 1));
            }
            let md5 = h.trim().to_ascii_lowercase();
            if md5.len() != 32 || !md5.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("manifest 第 {} 行：md5 格式不合法", idx + 1));
            }
            out.push(ManifestEntry { path, md5 });
            if out.len() > MANIFEST_MAX_ENTRIES {
                return Err(format!("manifest 超過 {} 條上限", MANIFEST_MAX_ENTRIES));
            }
        } else {
            return Err(format!("manifest 第 {} 行：無法辨識的行（{:?}）", idx + 1, line));
        }
    }
    if cur.is_some() {
        return Err("manifest 結尾有 [path] 區段缺 md5".into());
    }
    Ok(out)
}

/// 防路徑穿越：只接受相對、不含 `..`、無磁碟號、無開頭斜線的 manifest 路徑。
fn safe_join(game_dir: &Path, rel: &str) -> Option<PathBuf> {
    if rel.is_empty()
        || rel.contains("..")
        || rel.contains(':')
        || rel.starts_with('/')
        || rel.starts_with('\\')
    {
        return None;
    }
    Some(game_dir.join(rel.replace('\\', "/")))
}

fn file_md5_hex(p: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Md5::new();
    std::io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

fn http_get(url: &str, cap: u64) -> Result<Vec<u8>, String> {
    let resp = ureq::get(url)
        .timeout(std::time::Duration::from_secs(60))
        .call()
        .map_err(|e| format!("HTTP GET {url}：{e}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(cap)
        .read_to_end(&mut buf)
        .map_err(|e| format!("讀取 {url} 回應失敗：{e}"))?;
    Ok(buf)
}

fn base_url_of(manifest_url: &str) -> String {
    match manifest_url.rfind('/') {
        Some(i) => manifest_url[..i].to_string(),
        None => String::new(),
    }
}

#[derive(Default, Debug)]
pub struct UpdateReport {
    pub total: usize,
    pub unchanged: usize,
    /// 本地缺檔
    pub missing: usize,
    /// 本地存在但 MD5 不同
    pub changed: usize,
    /// 實際下載並套用成功
    pub downloaded: usize,
    /// 失敗清單（path＋原因）
    pub failed: Vec<String>,
}

/// 執行一輪 manifest 吃檔：拉 manifest → 比對 → 下載差異 → 驗 MD5 → 就地套用。
/// 同步（ureq blocking）；呼叫端在背景執行緒內使用。
pub fn run_manifest_update(
    game_dir: &Path,
    manifest_url: &str,
    log: &LogHandle,
) -> Result<UpdateReport, String> {
    let manifest_url = manifest_url.trim();
    if manifest_url.is_empty() {
        return Err("update_manifest_url 未設定".into());
    }
    if !manifest_url.starts_with("http://") {
        // ureq 無 TLS feature：只支援 http（私服內網情境；https 需另行加 feature）
        return Err("update_manifest_url 僅支援 http://（本建置未含 TLS）".into());
    }
    let root = game_dir.to_path_buf();
    if !root.is_dir() {
        return Err(format!("遊戲目錄不存在：{}", root.display()));
    }

    log.info(format!("吃檔：拉取 manifest {manifest_url}"));
    let text = String::from_utf8_lossy(&http_get(manifest_url, 32 * 1024 * 1024)?).into_owned();
    let entries = parse_manifest(&text)?;
    log.info(format!("吃檔：manifest 共 {} 條", entries.len()));

    let mut report = UpdateReport {
        total: entries.len(),
        ..Default::default()
    };
    let base = base_url_of(manifest_url);
    let mut downloaded_bytes: u64 = 0;

    for e in &entries {
        let Some(local) = safe_join(&root, &e.path) else {
            report
                .failed
                .push(format!("{}（路徑不合法，已跳過）", e.path));
            continue;
        };
        let need = match file_md5_hex(&local) {
            Ok(h) if h == e.md5 => {
                report.unchanged += 1;
                continue;
            }
            Ok(_) => {
                report.changed += 1;
                "變更"
            }
            Err(_) => {
                report.missing += 1;
                "新增"
            }
        };
        if base.is_empty() {
            report.failed.push(format!("{}（無法推導資源 URL）", e.path));
            continue;
        }
        let url = format!("{}/{}", base.trim_end_matches('/'), e.path.replace('\\', "/"));
        match http_get(&url, DOWNLOAD_FILE_CAP) {
            Ok(bytes) => {
                let mut h = Md5::new();
                h.update(&bytes);
                let got = format!("{:x}", h.finalize());
                if got != e.md5 {
                    report
                        .failed
                        .push(format!("{}（下載後 MD5 不符）", e.path));
                    log.warn(format!("吃檔：{} 下載後 MD5 不符，已丟棄", e.path));
                    continue;
                }
                if let Some(parent) = local.parent() {
                    if let Err(er) = std::fs::create_dir_all(parent) {
                        report
                            .failed
                            .push(format!("{}（建目錄失敗：{er}）", e.path));
                        continue;
                    }
                }
                let tmp = local.with_extension("rhd-dl-tmp");
                if let Err(er) = std::fs::write(&tmp, &bytes) {
                    report.failed.push(format!("{}（暫存寫入失敗：{er}）", e.path));
                    continue;
                }
                if let Err(er) = std::fs::rename(&tmp, &local) {
                    let _ = std::fs::remove_file(&tmp);
                    report.failed.push(format!("{}（套用失敗：{er}）", e.path));
                    continue;
                }
                downloaded_bytes += bytes.len() as u64;
                report.downloaded += 1;
                log.info(format!(
                    "吃檔：{}（{need}）→ 已下載驗證套用（{} bytes）",
                    e.path,
                    bytes.len()
                ));
                if downloaded_bytes > DOWNLOAD_TOTAL_CAP {
                    report
                        .failed
                        .push("（累積下載超過 4 GiB 上限，本輪中止）".into());
                    break;
                }
            }
            Err(er) => {
                report.failed.push(format!("{}（{er}）", e.path));
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    #[test]
    fn parse_manifest_happy_path_with_crlf() {
        let text = "[data\\a.tbl]\r\nmd5=D41D8CD98F00B204E9800998ECF8427E\r\n\r\n[data\\b.xml]\r\nmd5=0123456789abcdef0123456789abcdef\r\n";
        let v = parse_manifest(text).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].path, r"data\a.tbl");
        assert_eq!(v[0].md5, "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(v[1].path, r"data\b.xml");
    }

    #[test]
    fn parse_manifest_rejects_missing_md5_and_bad_hash() {
        assert!(parse_manifest("[data\\a.tbl]\r\n[data\\b.tbl]\r\nmd5=0123456789abcdef0123456789abcdef\r\n").is_err());
        assert!(parse_manifest("[data\\a.tbl]\r\nmd5=short\r\n").is_err());
        assert!(parse_manifest("md5=0123456789abcdef0123456789abcdef\r\n").is_err());
        assert!(parse_manifest("[data\\a.tbl]\r\nmd5=zz23456789abcdef0123456789abcdef\r\n").is_err());
    }

    #[test]
    fn safe_join_blocks_traversal_and_absolute() {
        let root = Path::new("G:\\L1RHD");
        assert!(safe_join(root, "data\\a.tbl").is_some());
        assert!(safe_join(root, "..\\evil").is_none());
        assert!(safe_join(root, "a\\..\\..\\evil").is_none());
        assert!(safe_join(root, "\\abs").is_none());
        assert!(safe_join(root, "C:\\evil").is_none());
        assert!(safe_join(root, "").is_none());
    }

    #[test]
    fn file_md5_hex_matches_empty_vector() {
        let dir = std::env::temp_dir().join(format!("rhd-md5-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("empty.bin");
        std::fs::write(&p, b"").unwrap();
        assert_eq!(
            file_md5_hex(&p).unwrap(),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn base_url_of_strips_last_segment() {
        assert_eq!(
            base_url_of("http://192.168.1.10:8080/patch/manifest.local"),
            "http://192.168.1.10:8080/patch"
        );
        assert_eq!(base_url_of("nofullslashes"), "");
    }

    /// 端到端：本地 HTTP 伺服器提供 manifest＋資源，驗證
    /// 拉 manifest → MD5 比對（不動/變更/缺檔）→ 下載 → 驗 MD5 → 就地套用。
    #[test]
    fn manifest_update_end_to_end_over_local_http() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("rhd-e2e-{}", std::process::id()));
        let gd = root.join("game");
        std::fs::create_dir_all(gd.join("data")).unwrap();
        std::fs::write(gd.join("data/a.tbl"), b"AAA").unwrap();
        std::fs::write(gd.join("data/b.xml"), b"OLD").unwrap();
        let md5 = |b: &[u8]| {
            let mut h = Md5::new();
            h.update(b);
            format!("{:x}", h.finalize())
        };
        let manifest = format!(
            "[data\\a.tbl]\r\nmd5={}\r\n[data\\b.xml]\r\nmd5={}\r\n[data\\c.ini]\r\nmd5={}\r\n",
            md5(b"AAA"),
            md5(b"NEW-B"),
            md5(b"NEW-C")
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let manifest_bytes = manifest.into_bytes();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut s, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let n = s.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req
                    .lines()
                    .next()
                    .unwrap()
                    .split(' ')
                    .nth(1)
                    .unwrap()
                    .to_string();
                let body: &[u8] = if path.ends_with("manifest.local") {
                    &manifest_bytes
                } else if path.ends_with("b.xml") {
                    b"NEW-B"
                } else {
                    b"NEW-C"
                };
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                s.write_all(hdr.as_bytes()).unwrap();
                s.write_all(body).unwrap();
            }
        });
        let log = crate::logging::LogHandle::new();
        let r = run_manifest_update(
            &gd,
            &format!("http://127.0.0.1:{port}/patch/manifest.local"),
            &log,
        )
        .unwrap();
        server.join().unwrap();
        assert_eq!(r.total, 3);
        assert_eq!(r.unchanged, 1);
        assert_eq!(r.changed, 1);
        assert_eq!(r.missing, 1);
        assert_eq!(r.downloaded, 2);
        assert!(r.failed.is_empty(), "failed={:?}", r.failed);
        assert_eq!(std::fs::read(gd.join("data/b.xml")).unwrap(), b"NEW-B");
        assert_eq!(std::fs::read(gd.join("data/c.ini")).unwrap(), b"NEW-C");
        assert_eq!(std::fs::read(gd.join("data/a.tbl")).unwrap(), b"AAA");
        let _ = std::fs::remove_dir_all(&root);
    }
}
