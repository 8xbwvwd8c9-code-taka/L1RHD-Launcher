//! TCP 中繼 (企畫書 §5.3)
//!
//!  1. 跳過自我迴圈（loopback 且 port == target_port）
//!  2. IPv4 綁定失敗 → SO_REUSEADDR 強制接管並記日誌
//!  3. 同時綁 ::1（先 IPV6_V6ONLY=1 再 bind）
//!  4. 雙向 pump，位元組原封不動，結束時 shutdown 對方
//!  5. 連目標失敗立即關下游並記日誌
//!  6. conn_seq 遞增，日誌格式 [#N] 客戶端連入 host:port（埠 P） 不改
//!  7. 停止時優雅關閉所有 listener 與既有連線

use crate::logging::{Lvl, LogHandle};
use std::net::{IpAddr, SocketAddr, TcpListener as StdTcpListener};
use std::os::windows::io::FromRawSocket;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

/// 封包採證開關（launcher.json `packet_capture`，預設關閉）。
/// 開啟時記錄**雙向**每個讀取區塊的完整 hex（上限 600B；合包時一個區塊可能含
/// 多個邏輯封包）。登入封包含帳密，日誌檔留在本機；診斷完應把開關關回。
static AUTH_CAPTURE: AtomicU8 = AtomicU8::new(0);

pub fn set_auth_capture(on: bool) {
    AUTH_CAPTURE.store(if on { 1 } else { 0 }, Ordering::SeqCst);
}

/// 條件成立時回傳區塊完整 hex（上限 600B）；否則 None。
fn auth_capture_prefix(buf: &[u8]) -> Option<String> {
    if AUTH_CAPTURE.load(Ordering::SeqCst) != 1 {
        return None;
    }
    if buf.is_empty() {
        return None;
    }
    let n = buf.len().min(600);
    let mut s = String::with_capacity(n * 3);
    for b in &buf[..n] {
        s.push_str(&format!("{:02X} ", b));
    }
    s.pop();
    Some(s)
}

/// 登入代理設定（launcher.json `proxy_login`）。None = 純透傳。
///
/// 背景：B2 以 `direct_launch` 直接啟動 `bin64\Lin.bin` 時，客戶端送出的第一個
/// opcode 199 不是伺服端 `C_AuthLogin` 認得的 `account\0password\0` 佈局
/// （2026-10-30 實測 288B 高熵 body + 明文尾端），伺服端必然回「帳號長度不合法」。
/// 原版流程是 `Login.exe` 自己發明文帳密包（2026-10-24 實錄 17B），再把會話交給
/// Lin.bin；本改寫器就是把這件事搬進中繼——只換 199 的 body，其餘位元組原封不動。
/// 不改客戶端、不改服務端，雙端仍全明文（符合 ADR-005）。
static PROXY_LOGIN: std::sync::Mutex<Option<(String, String)>> = std::sync::Mutex::new(None);

/// 補發 S_KEY（伺服端 opcode 15）開關（launcher.json `inject_s_key`，預設關閉）。
///
/// ADR-005 讓伺服端永久停發 KeyPacket，而客戶端沒收到這一幀就完全不進入登入
/// 流程：2026-10-30 多次實測，連上中繼後 36～168 秒上下行皆 0 bytes，最後自己
/// 關線並彈「目前無法使用此伺服器」。中繼代伺服端補上這一幀，客戶端才會送出
/// 161B（opcode 244）＋295B（opcode 199），後續登入代理才有東西可改寫。
///
/// 內容取自伺服端 `KeyPacket.java` 的固定種子；與 2026-10-24 實錄的 13 bytes
/// （`0D 00 0F 42 90 E1 7A A4 29 70 53 00 00`）逐位元組相同。
const S_KEY_FRAME: [u8; 13] = [
    0x0D, 0x00, 0x0F, 0x42, 0x90, 0xE1, 0x7A, 0xA4, 0x29, 0x70, 0x53, 0x00, 0x00,
];

static INJECT_S_KEY: AtomicU8 = AtomicU8::new(0);

pub fn set_inject_s_key(on: bool) {
    INJECT_S_KEY.store(if on { 1 } else { 0 }, Ordering::SeqCst);
}

fn inject_s_key_on() -> bool {
    INJECT_S_KEY.load(Ordering::SeqCst) == 1
}

pub fn set_proxy_login(creds: Option<(String, String)>) {
    *PROXY_LOGIN.lock().unwrap_or_else(|e| e.into_inner()) = creds;
}

fn proxy_login_creds() -> Option<(String, String)> {
    PROXY_LOGIN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

/// 依 launcher.json 一次套用中繼行為（封包採證＋登入代理＋補發 S_KEY）。
/// UI 主路徑、常駐 daemon 與視窗建構三處共用，避免各自漏設。
pub fn apply_config(
    capture: bool,
    proxy: bool,
    account: &str,
    password: &str,
    inject_s_key: bool,
) {
    set_auth_capture(capture);
    set_inject_s_key(inject_s_key);
    if proxy && !account.is_empty() && !password.is_empty() {
        set_proxy_login(Some((account.to_string(), password.to_string())));
    } else {
        set_proxy_login(None);
    }
}

/// 幀格式：`[u16 LE total（含前綴）][4B 序號][opcode][body]`。
/// 只改第一個 opcode 199（`C_AuthLogin`），序號沿用客戶端原值。
struct LoginRewrite {
    account: Vec<u8>,
    password: Vec<u8>,
    pending: Vec<u8>,
    done: bool,
}

impl LoginRewrite {
    /// 帳號 2–14、密碼 4–16（伺服端 `C_AuthLogin` 閘門），且限 ASCII。
    fn new(account: &str, password: &str) -> Option<Self> {
        if !account.is_ascii() || !password.is_ascii() {
            return None;
        }
        if !(2..=14).contains(&account.len()) || !(4..=16).contains(&password.len()) {
            return None;
        }
        Some(Self {
            account: account.as_bytes().to_vec(),
            password: password.as_bytes().to_vec(),
            pending: Vec::new(),
            done: false,
        })
    }

    fn login_frame(&self, seq: &[u8], total_len: &mut u64) -> Vec<u8> {
        let body = self.account.len() + 1 + self.password.len() + 1;
        let total = 7 + body;
        let mut f = Vec::with_capacity(total);
        f.extend_from_slice(&(total as u16).to_le_bytes());
        f.extend_from_slice(seq);
        f.push(0xC7);
        f.extend_from_slice(&self.account);
        f.push(0);
        f.extend_from_slice(&self.password);
        f.push(0);
        *total_len += total as u64;
        f
    }

    /// 吃任意切片，吐出要送往上游的位元組（可為空；跨讀塊的殘幀留在 pending）。
    fn push(&mut self, input: &[u8], rewritten: &mut u64) -> Vec<u8> {
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        loop {
            if self.pending.len() < 2 {
                break;
            }
            let total = u16::from_le_bytes([self.pending[0], self.pending[1]]) as usize;
            // 幀長不合理（<7 或 >16KiB）就不猜，整段原樣放行，避免卡死連線。
            if !(7..=16384).contains(&total) {
                out.extend_from_slice(&self.pending);
                self.pending.clear();
                break;
            }
            if self.pending.len() < total {
                break;
            }
            let frame: Vec<u8> = self.pending.drain(..total).collect();
            if !self.done && frame[6] == 0xC7 {
                out.extend_from_slice(&self.login_frame(&frame[2..6], rewritten));
                self.done = true;
            } else {
                out.extend_from_slice(&frame);
            }
        }
        out
    }
}

pub struct RelayHandle {
    pub bound: Vec<(u16, SocketAddr)>,
    #[allow(dead_code)]
    pub conn_seq: Arc<AtomicU64>,
    stop_tx: watch::Sender<bool>,
    pub running: bool,
}

impl RelayHandle {
    pub fn stop(&mut self) {
        let _ = self.stop_tx.send(true);
        self.running = false;
    }
    #[allow(dead_code)]
    pub fn conns(&self) -> u64 {
        self.conn_seq.load(Ordering::Relaxed)
    }
}

fn is_loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host.parse::<IpAddr>().map(|i| i.is_loopback()).unwrap_or(false)
}

fn bind_addrs(listen_host: &str, port: u16) -> Vec<SocketAddr> {
    let mut v = Vec::new();
    match listen_host.parse::<IpAddr>() {
        Ok(ip) => v.push(SocketAddr::new(ip, port)),
        Err(_) => v.push(SocketAddr::new(IpAddr::from([127, 0, 0, 1]), port)),
    }
    if v.iter().any(|a| a.ip().is_ipv4()) {
        if let Ok(v6) = "::1".parse::<IpAddr>() {
            v.push(SocketAddr::new(v6, port));
        }
    }
    v
}

/// 原生 winsock 綁定，可指定 SO_REUSEADDR / IPV6_V6ONLY。回傳 std TcpListener（阻塞、非 non-blocking）。
fn raw_bind(addr: SocketAddr, reuse: bool) -> std::io::Result<StdTcpListener> {
    use windows::Win32::Networking::WinSock::*;

    unsafe {
        let mut data = WSADATA::default();
        let _ = WSAStartup(0x0202, &mut data);
    }

    let is_v6 = addr.is_ipv6();
    let family = if is_v6 { AF_INET6 } else { AF_INET };

    unsafe {
        let s = socket(family.0 as i32, SOCK_STREAM, IPPROTO_TCP.0)
            .map_err(|e| std::io::Error::other(format!("socket 建立失敗: {e}")))?;

        if reuse {
            let one: i32 = 1;
            let _ = setsockopt(
                s,
                SOL_SOCKET,
                SO_REUSEADDR,
                Some(std::slice::from_raw_parts(&one as *const i32 as *const u8, 4)),
            );
        }
        if is_v6 {
            let one: i32 = 1;
            let _ = setsockopt(
                s,
                IPPROTO_IPV6.0,
                IPV6_V6ONLY,
                Some(std::slice::from_raw_parts(&one as *const i32 as *const u8, 4)),
            );
        }

        let mut storage: SOCKADDR_STORAGE = std::mem::zeroed();
        let len = if !is_v6 {
            let v4 = match addr {
                SocketAddr::V4(v) => v,
                _ => unreachable!(),
            };
            let p = &mut storage as *mut _ as *mut SOCKADDR_IN;
            (*p).sin_family = AF_INET;
            (*p).sin_port = v4.port().to_be();
            (*p).sin_addr.S_un.S_addr = u32::from_ne_bytes(v4.ip().octets());
            std::mem::size_of::<SOCKADDR_IN>() as i32
        } else {
            let v6 = match addr {
                SocketAddr::V6(v) => v,
                _ => unreachable!(),
            };
            let p = &mut storage as *mut _ as *mut SOCKADDR_IN6;
            (*p).sin6_family = AF_INET6;
            (*p).sin6_port = v6.port().to_be();
            let bytes = &mut (*p).sin6_addr.u.Byte;
            bytes[..16].copy_from_slice(&v6.ip().octets());
            std::mem::size_of::<SOCKADDR_IN6>() as i32
        };

        let rc = bind(s, &storage as *const _ as *const SOCKADDR, len);
        if rc != 0 {
            let e = std::io::Error::last_os_error();
            let _ = closesocket(s);
            return Err(e);
        }
        if listen(s, SOMAXCONN as i32) != 0 {
            let e = std::io::Error::last_os_error();
            let _ = closesocket(s);
            return Err(e);
        }
        Ok(StdTcpListener::from_raw_socket(s.0 as u64))
    }
}

pub fn start(
    listen_host: &str,
    listen_ports: &[u16],
    target_host: &str,
    target_port: u16,
    log: LogHandle,
    rt: &tokio::runtime::Handle,
) -> Result<RelayHandle, String> {
    let (stop_tx, stop_rx) = watch::channel(false);
    let conn_seq = Arc::new(AtomicU64::new(0));
    let mut bound: Vec<(u16, SocketAddr)> = Vec::new();
    let target = format!("{target_host}:{target_port}");
    let loopback_target = is_loopback(target_host);

    for &port in listen_ports {
        if loopback_target && port == target_port {
            log.warn(format!("埠 {port} 與目標相同，略過以避免自己轉給自己"));
            continue;
        }

        let mut bound_any = false;
        for addr in bind_addrs(listen_host, port) {
            let attempt = raw_bind(addr, false).or_else(|first| match raw_bind(addr, true) {
                Ok(l) => {
                    log.warn(format!(
                        "綁定 {addr} 原本被其他程式佔用，已用 SO_REUSEADDR 強制接管（原因：{first}）"
                    ));
                    Ok(l)
                }
                Err(e2) => Err(e2),
            });

            match attempt {
                Ok(std_l) => {
                    let actual = std_l.local_addr().unwrap_or(addr);
                    let _ = std_l.set_nonblocking(true);
                    // 必須在 runtime context 內建立 tokio listener
                    let joined = rt.block_on(async { TcpListener::from_std(std_l) });
                    match joined {
                        Ok(l) => {
                            bound.push((port, actual));
                            bound_any = true;
                            log.info(format!("中繼監聽 {actual} -> {target}"));
                            let t = target.clone();
                            let lg = log.clone();
                            let rx = stop_rx.clone();
                            let seq = conn_seq.clone();
                            rt.spawn(async move {
                                accept_loop(l, t, port, lg, rx, seq).await;
                            });
                        }
                        Err(e) => log.error(format!("轉換 listener 失敗 {actual}: {e}")),
                    }
                }
                Err(e) => {
                    let hint = if port == 2000 {
                        "（常見：VMware NAT vmnat.exe 佔用 2000）"
                    } else {
                        ""
                    };
                    log.error(format!("綁定 {addr} 失敗：{e}{hint}"));
                }
            }
        }
        if !bound_any {
            log.error(format!("埠 {port} 完全綁不上，中繼將無法提供該埠"));
        }
    }

    if bound.is_empty() {
        return Err("沒有任何監聽埠成功綁定".to_string());
    }

    Ok(RelayHandle {
        bound,
        conn_seq,
        stop_tx,
        running: true,
    })
}

async fn accept_loop(
    listener: TcpListener,
    target: String,
    port: u16,
    log: LogHandle,
    mut stop_rx: watch::Receiver<bool>,
    conn_seq: Arc<AtomicU64>,
) {
    loop {
        tokio::select! {
            changed = stop_rx.changed() => {
                let _ = changed;
                if *stop_rx.borrow() {
                    log.info(format!("監聽埠 {port} 已停止"));
                    break;
                }
            }
            acc = listener.accept() => {
                match acc {
                    Ok((inbound, peer)) => {
                        let seq = conn_seq.fetch_add(1, Ordering::SeqCst) + 1;
                        let t = target.clone();
                        let lg = log.clone();
                        let rx = stop_rx.clone();
                        let rewrite = proxy_login_creds()
                            .and_then(|(a, p)| LoginRewrite::new(&a, &p));
                        tokio::spawn(async move {
                            let peer_ip = if peer.is_ipv6() {
                                format!("[{}]", peer.ip())
                            } else {
                                peer.ip().to_string()
                            };
                            lg.info(format!(
                                "[#{seq}] 客戶端連入 {peer_ip}:{}（埠 {port}）",
                                peer.port()
                            ));
                            match TcpStream::connect(&t).await {
                                Ok(outbound) => {
                                    lg.info(format!("[#{seq}] 已接上 {t}"));
                                    if rewrite.is_some() {
                                        lg.info(format!(
                                            "[#{seq}] 登入代理啟用：第一個 opcode 199 將改寫為明文帳密包"
                                        ));
                                    }
                                    let inject = inject_s_key_on();
                                    pump(inbound, outbound, seq, lg, rx, rewrite, inject).await;
                                }
                                Err(e) => {
                                    lg.error(format!(
                                        "[#{seq}] 連不上目標 {t}：{e}（已關閉客戶端）"
                                    ));
                                    drop(inbound);
                                }
                            }
                        });
                    }
                    Err(e) => {
                        log.warn(format!("accept 失敗：{e}"));
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        }
    }
}

/// 雙向 pump：位元組原封不動；任一方向結束就 shutdown 對方，等另一方向也收尾。
/// TR-BUG-458 [已修復但未實機測試]：日誌只記方向與長度，避免記錄登入憑證。
async fn pipe_with_log<R, W>(
    mut r: R,
    mut w: W,
    tag: &'static str,
    seq: u64,
    log: LogHandle,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 16384];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total += n as u64;
        log.info(format!("[#{seq}] {tag} {n} bytes"));
        if let Some(hex) = auth_capture_prefix(&buf[..n]) {
            log.info(format!("[#{seq}] PKT-{tag} {hex}"));
        }
        w.write_all(&buf[..n]).await?;
        w.flush().await?;
    }
    let _ = w.shutdown().await;
    Ok(total)
}

/// C>S 方向：可選的登入代理改寫。日誌只記方向、長度與「已改寫」，不記帳密。
async fn pipe_client_to_server<R, W>(
    mut r: R,
    mut w: W,
    seq: u64,
    log: LogHandle,
    mut rewrite: Option<LoginRewrite>,
) -> std::io::Result<u64>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 16384];
    let mut total = 0u64;
    loop {
        let n = r.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        total += n as u64;
        log.info(format!("[#{seq}] C>S {n} bytes"));
        if let Some(hex) = auth_capture_prefix(&buf[..n]) {
            log.info(format!("[#{seq}] PKT-C>S {hex}"));
        }
        let out = match rewrite.as_mut() {
            Some(rw) => {
                let mut rewritten = 0u64;
                let out = rw.push(&buf[..n], &mut rewritten);
                if rewritten > 0 {
                    log.info(format!(
                        "[#{seq}] 登入代理：opcode 199 已改寫為明文帳密包（{rewritten} bytes；客戶端原送 {n} bytes）"
                    ));
                }
                out
            }
            None => buf[..n].to_vec(),
        };
        if !out.is_empty() {
            w.write_all(&out).await?;
            w.flush().await?;
        }
    }
    let _ = w.shutdown().await;
    Ok(total)
}

async fn pump(
    mut inbound: TcpStream,
    mut outbound: TcpStream,
    seq: u64,
    log: LogHandle,
    mut stop_rx: watch::Receiver<bool>,
    rewrite: Option<LoginRewrite>,
    inject_s_key: bool,
) {
    let _ = inbound.set_nodelay(true);
    let _ = outbound.set_nodelay(true);
    let (ir, mut iw) = inbound.split();
    let (or_, ow) = outbound.split();

    // 伺服端停發 S_KEY；客戶端沒收到就不送任何封包。要在雙向 pump 之前補上。
    if inject_s_key {
        match iw.write_all(&S_KEY_FRAME).await {
            Ok(()) => log.info(format!(
                "[#{seq}] 補發 S_KEY（opcode 15，{} bytes；伺服端依 ADR-005 停發）",
                S_KEY_FRAME.len()
            )),
            Err(e) => log.warn(format!("[#{seq}] 補發 S_KEY 失敗：{e}")),
        }
    }

    // 兩個方向獨立進行，任一方向 EOF 只關閉該方向，不強制中斷另一方向
    // （比照 relay_debug.py 的 settimeout(None) 行為，避免遊戲長連線被切斷）
    let up = pipe_client_to_server(ir, ow, seq, log.clone(), rewrite);
    let down = pipe_with_log(or_, iw, "S>C", seq, log.clone());

    tokio::select! {
        _ = stop_rx.changed() => {
            log.info(format!("[#{seq}] 因停止而關閉"));
        }
        (a, b) = async { tokio::join!(up, down) } => {
            match (a, b) {
                (Ok(x), Ok(y)) => log.log(
                    Lvl::Info,
                    format!("[#{seq}] 結束 上傳 {x} bytes，下載 {y} bytes"),
                ),
                (Err(e), _) | (_, Err(e)) => log.warn(format!("[#{seq}] 中斷：{e}")),
            }
        }
    }
}

/// 偵測本機是否已有中繼在服務這些 listen 埠（daemon 常駐設計：登入器重開時沿用，不重綁）。
pub fn existing_relay_on(ports: &[u16]) -> bool {
    use std::net::TcpStream;
    use std::time::Duration;
    !ports.is_empty()
        && ports.iter().all(|&p| {
            match format!("127.0.0.1:{p}").parse::<SocketAddr>() {
                Ok(a) => TcpStream::connect_timeout(&a, Duration::from_millis(300)).is_ok(),
                Err(_) => false,
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn frame244() -> Vec<u8> {
        let mut f = vec![0xA1, 0x00, 0x01, 0x00, 0x00, 0x00, 0xF4];
        f.extend(std::iter::repeat(0x11).take(154));
        f
    }

    fn frame199() -> Vec<u8> {
        let mut f = vec![0x27, 0x01, 0x02, 0x00, 0x00, 0x00, 0xC7];
        f.extend(std::iter::repeat(0x22).take(288));
        f
    }

    #[test]
    fn login_rewrite_replaces_first_199_with_plain_credentials() {
        let mut rw = LoginRewrite::new("5566", "5566").unwrap();
        let mut rewritten = 0u64;
        let out = rw.push(&frame199(), &mut rewritten);
        assert_eq!(out.len(), 17);
        assert_eq!(&out[..7], &[0x11, 0x00, 0x02, 0x00, 0x00, 0x00, 0xC7]);
        assert_eq!(
            &out[7..],
            &[b'5', b'5', b'6', b'6', 0, b'5', b'5', b'6', b'6', 0]
        );
        assert_eq!(rewritten, 17);
    }

    #[test]
    fn login_rewrite_leaves_handshake_frame_untouched() {
        let mut rw = LoginRewrite::new("5566", "5566").unwrap();
        let f = frame244();
        let mut rewritten = 0u64;
        let out = rw.push(&f, &mut rewritten);
        assert_eq!(out, f);
        assert_eq!(rewritten, 0);
    }

    #[test]
    fn login_rewrite_handles_split_and_coalesced_frames() {
        let mut rw = LoginRewrite::new("5566", "5566").unwrap();
        let f244 = frame244();
        let mut stream = f244.clone();
        stream.extend_from_slice(&frame199());
        let (a, b) = stream.split_at(3);
        let mut rewritten = 0u64;
        assert!(rw.push(a, &mut rewritten).is_empty());
        let out = rw.push(b, &mut rewritten);
        assert_eq!(out.len(), f244.len() + 17);
        assert_eq!(&out[..f244.len()], &f244[..]);
        assert_eq!(rewritten, 17);
    }

    #[test]
    fn login_rewrite_only_first_199() {
        let mut rw = LoginRewrite::new("5566", "5566").unwrap();
        let mut rewritten = 0u64;
        let f = frame199();
        let _ = rw.push(&f, &mut rewritten);
        let second = rw.push(&f, &mut rewritten);
        assert_eq!(second, f);
        assert_eq!(rewritten, 17);
    }

    #[test]
    fn login_rewrite_rejects_out_of_range_credentials() {
        assert!(LoginRewrite::new("5", "5566").is_none());
        assert!(LoginRewrite::new("5566", "556").is_none());
        assert!(LoginRewrite::new("帳號", "5566").is_none());
        assert!(LoginRewrite::new(&"a".repeat(15), "5566").is_none());
    }

    #[test]
    fn existing_relay_detects_live_local_listener() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(existing_relay_on(&[port]));
        // 埠 1 不可連（保留埠），應判定為無既有中繼
        assert!(!existing_relay_on(&[1]));
        assert!(!existing_relay_on(&[]));
        drop(l);
    }

    #[test]
    fn auth_capture_gated_and_masked() {
        let payload = [0xC7u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        set_auth_capture(false);
        assert!(auth_capture_prefix(&payload).is_none());
        set_auth_capture(true);
        let hex = auth_capture_prefix(&payload).unwrap();
        assert!(hex.starts_with("C7 01 02"));
        set_auth_capture(false);
    }

    #[tokio::test]
    async fn forwards_payload_without_recording_its_contents() {
        let (mut producer, input) = tokio::io::duplex(128);
        let (output, mut consumer) = tokio::io::duplex(128);
        let log = LogHandle::new();
        let payload = b"SENSITIVE_TEST_VALUE";
        let send = async {
            producer.write_all(payload).await.unwrap();
            producer.shutdown().await.unwrap();
        };
        let receive = async {
            let mut received = Vec::new();
            consumer.read_to_end(&mut received).await.unwrap();
            received
        };
        let ((), result, received) = tokio::join!(
            send,
            pipe_with_log(input, output, "C>S", 1, log.clone()),
            receive
        );
        assert_eq!(result.unwrap(), payload.len() as u64);
        assert_eq!(received, payload);
        let exported = log.export();
        assert!(exported.contains("C>S 20 bytes"));
        assert!(!exported.contains("SENSITIVE_TEST_VALUE"));
        assert!(!exported.contains("53 45 4E 53 49 54 49 56 45"));
    }
}
