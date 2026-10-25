//! lineage.cfg TLV 讀寫 (企畫書 §5.5)
//!
//! 格式：
//!   [0..26]  "lineage configuration file"
//!   [26..30] 1a 00 00 00   (u32 = 26, magic 長度)
//!   [30..36] 00 00 04 00 00 00  (語意不明，勿碰)
//!   [36..40] "n-r\\0"
//!   [40..]   TLV: [tag:u32le][len:u32le][value:len]
//!
//! 雑坑：TLV 起點是 40，不是 30。不硬編 offset，用特徵搜尋。
//! tag 0x08 已證實不是連線埠（會漂移），rewrite_cfg 預設 false。

use std::path::{Path, PathBuf};

pub const MAGIC: &[u8] = b"lineage configuration file";
const N_R: [u8; 4] = [b'n', b'-', b'r', 0];

pub fn cfg_path(game_dir: &str) -> PathBuf {
    Path::new(game_dir).join("lineage.cfg")
}

/// 找到 TLV 起點：）magic + u32 + 6 bytes + "n-r\0" 之後
fn tlv_start(data: &[u8]) -> Option<usize> {
    let m = data.windows(MAGIC.len()).position(|w| w == MAGIC)?;
    // 優先尋找 "n-r\0" 作為 TLV 前導
    let from = m + MAGIC.len();
    if let Some(p) = data[from..].windows(4).position(|w| w == N_R) {
        return Some(from + p + 4);
    }
    // fallback: magic(26) + 4 + 6 + 4 = 40
    Some(m + 40)
}

#[derive(Debug, Clone)]
pub struct Field {
    pub tag: u32,
    #[allow(dead_code)]
    pub offset: usize,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct LineageCfg {
    pub fields: Vec<Field>,
}

impl LineageCfg {
    pub fn get4(&self, tag: u32) -> Option<[u8; 4]> {
        self.fields
            .iter()
            .find(|f| f.tag == tag && f.value.len() == 4)
            .map(|f| {
                let mut a = [0u8; 4];
                a.copy_from_slice(&f.value);
                a
            })
    }

    pub fn ip(&self) -> Option<String> {
        self.get4(0x07).map(|b| format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
    }

    /// tag 0x08。注意：不是連線埠，只作診斷顯示
    pub fn tag08(&self) -> Option<u32> {
        self.get4(0x08).map(|b| u32::from_le_bytes(b))
    }
}

pub fn read(game_dir: &str) -> Result<(LineageCfg, Vec<u8>), String> {
    let p = cfg_path(game_dir);
    let data = std::fs::read(&p).map_err(|e| format!("讀取 {} 失敗：{e}", p.display()))?;
    let start = tlv_start(&data).ok_or_else(|| "lineage.cfg 找不到特徵區段".to_string())?;

    let mut fields = Vec::new();
    let mut i = start;
    while i + 8 <= data.len() {
        let tag = u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let len = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
        if tag == 0xFFFF_FFFF {
            break;
        }
        // tag 0x2711 疑似沒有 len 欄位（實測接 Big5+NUL），對此特例容錯
        if tag == 0x2711 {
            let mut j = i + 4;
            while j < data.len() && data[j] != 0 {
                j += 1;
            }
            if j >= data.len() {
                break;
            }
            fields.push(Field {
                tag,
                offset: i,
                value: data[i + 4..j].to_vec(),
            });
            i = j + 1;
            continue;
        }
        if len > data.len() || i + 8 + len > data.len() {
            break;
        }
        fields.push(Field {
            tag,
            offset: i,
            value: data[i + 8..i + 8 + len].to_vec(),
        });
        i += 8 + len;
    }
    if fields.is_empty() {
        return Err("lineage.cfg TLV 解析結果為空".to_string());
    }
    Ok((LineageCfg { fields }, data))
}

#[allow(dead_code)]
pub fn ip_to_bytes(ip: &str) -> Result<[u8; 4], String> {
    let parts: Vec<&str> = ip.trim().split('.').collect();
    if parts.len() != 4 {
        return Err(format!("不合法的 IPv4：{ip}"));
    }
    let mut out = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        let n: u32 = p
            .parse()
            .map_err(|_| format!("不合法的 IPv4：{ip}"))?;
        if n > 255 {
            return Err(format!("不合法的 IPv4：{ip}"));
        }
        out[i] = n as u8;
    }
    Ok(out)
}

/// 就地覆蓋，不重建檔。回傳新值與 hex 供使用者核對。
#[allow(dead_code)]
pub fn write_tag4(game_dir: &str, tag: u32, value4: [u8; 4]) -> Result<String, String> {
    let p = cfg_path(game_dir);
    let mut data = std::fs::read(&p).map_err(|e| format!("讀取 {} 失敗：{e}", p.display()))?;
    let mut sig = [0u8; 8];
    sig[..4].copy_from_slice(&tag.to_le_bytes());
    sig[4] = 4;
    let pos = data
        .windows(8)
        .position(|w| w == sig)
        .ok_or_else(|| format!("lineage.cfg 找不到 tag 0x{tag:02X} 的 4-byte 欄位"))?;
    let old = [data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]];
    data[pos + 8..pos + 12].copy_from_slice(&value4);
    // 寫回前保留備份
    let bak = p.with_extension("cfg.rhd.bak");
    if !bak.exists() {
        let _ = std::fs::copy(&p, &bak);
    }
    std::fs::write(&p, &data).map_err(|e| format!("寫回 {} 失敗：{e}", p.display()))?;
    Ok(format!(
        "tag 0x{tag:02X}: {:02X?} -> {:02X?} (位置 0x{pos:X})",
        old, value4
    ))
}
