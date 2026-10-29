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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

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
                                    pump(inbound, outbound, seq, lg, rx).await;
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
        w.write_all(&buf[..n]).await?;
        w.flush().await?;
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
) {
    let _ = inbound.set_nodelay(true);
    let _ = outbound.set_nodelay(true);
    let (ir, iw) = inbound.split();
    let (or_, ow) = outbound.split();

    // 兩個方向獨立進行，任一方向 EOF 只關閉該方向，不強制中斷另一方向
    // （比照 relay_debug.py 的 settimeout(None) 行為，避免遊戲長連線被切斷）
    let up = pipe_with_log(ir, ow, "C>S", seq, log.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
