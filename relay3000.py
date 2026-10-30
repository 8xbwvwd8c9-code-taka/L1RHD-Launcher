# -*- coding: utf-8 -*-
"""天R 客戶端 -> 服務端 獨立中繼（不依賴 GUI、不改 hosts）。

客戶端連 127.0.0.1 的 2000 / 29467（歷史紀錄也出現過 11702），
服務端 gs.jar 實際只聽 3000，由本腳本把兩邊接起來。

用法： python relay3000.py
停止： Ctrl+C
"""
import socket
import threading
import time
import sys

TARGET = ("127.0.0.1", 3000)
LISTEN_PORTS = [2000, 29467, 11702]
BUFSIZE = 16384
_seq = 0
_lock = threading.Lock()


def ts():
    return time.strftime("%H:%M:%S")


def pipe(src, dst, tag):
    try:
        while True:
            data = src.recv(BUFSIZE)
            if not data:
                break
            dst.sendall(data)
    except Exception:
        pass
    finally:
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except Exception:
                pass
            try:
                s.close()
            except Exception:
                pass


def handle(client_sock, addr, port, idx):
    print("[%s] [#%d] 客戶端連入 %s:%d（埠 %d）"
          % (ts(), idx, addr[0], addr[1], port), flush=True)
    try:
        server_sock = socket.create_connection(TARGET, timeout=5)
        server_sock.settimeout(None)  # 清除 timeout，避免長連線被切斷
        server_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        print("[%s] [#%d] 已接上目標 %s:%d" % (ts(), idx, TARGET[0], TARGET[1]), flush=True)
    except Exception as e:
        print("[%s] [#%d] 目標連不上：%s" % (ts(), idx, e), flush=True)
        client_sock.close()
        return
    threading.Thread(target=pipe, args=(client_sock, server_sock, "C>S"), daemon=True).start()
    threading.Thread(target=pipe, args=(server_sock, client_sock, "S>C"), daemon=True).start()


def serve(port):
    global _seq
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        srv.bind(("127.0.0.1", port))
        srv.listen(32)
    except Exception as e:
        print("[%s] 埠 %d 綁定失敗：%s" % (ts(), port, e), flush=True)
        return
    print("[%s] 監聽 127.0.0.1:%d -> %s:%d" % (ts(), port, TARGET[0], TARGET[1]), flush=True)
    while True:
        try:
            cs, addr = srv.accept()
        except Exception:
            break
        with _lock:
            _seq += 1
            idx = _seq
        cs.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        threading.Thread(target=handle, args=(cs, addr, port, idx), daemon=True).start()


def main():
    # 先驗證目標活著
    try:
        t = socket.create_connection(TARGET, timeout=3)
        t.close()
        print("[%s] 目標 %s:%d 可連線 = True" % (ts(), TARGET[0], TARGET[1]), flush=True)
    except Exception as e:
        print("[%s] 警告：目標 %s:%d 連不上（%s），仍會啟動監聽"
              % (ts(), TARGET[0], TARGET[1], e), flush=True)

    for p in LISTEN_PORTS:
        threading.Thread(target=serve, args=(p,), daemon=True).start()

    try:
        while True:
            time.sleep(1)
    except KeyboardInterrupt:
        print("[%s] 中繼結束" % ts(), flush=True)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    main()