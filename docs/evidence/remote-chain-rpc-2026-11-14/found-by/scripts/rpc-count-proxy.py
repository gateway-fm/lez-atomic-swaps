#!/usr/bin/env python3
"""rpc-count-proxy.py <log> <listen_port:target_port> ... — a measuring TCP proxy on
127.0.0.1. Each listen port forwards to 127.0.0.1:<target_port> (the SSH tunnel's end,
moved off the standard chain ports). Every HTTP request a client sends is logged with its
wall-clock time, the chain port, the JSON-RPC method(s) in its body, and the time until
the first response bytes: `<unix time> <port> <method> <ms>`. Bytes are passed through
unchanged; nothing is parsed beyond finding "method" in each request."""
import re
import socket
import sys
import threading
import time

LOG = open(sys.argv[1], "a", buffering=1)
METHOD = re.compile(rb'"method"\s*:\s*"([^"]+)"')
lock = threading.Lock()


def pump(src, dst, port, pending, client_side):
    try:
        while True:
            data = src.recv(65536)
            if not data:
                break
            now = time.time()
            if client_side:
                methods = METHOD.findall(data)
                if data.startswith((b"POST", b"GET")) or methods:
                    pending.append((now, b",".join(methods).decode(errors="replace") or "-"))
            elif pending:
                start, method = pending.pop(0)
                with lock:
                    LOG.write(f"{start:.3f} {port} {method} {(now - start) * 1000:.0f}\n")
            dst.sendall(data)
    except OSError:
        pass
    finally:
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


def serve(listen, target):
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", listen))
    server.listen(128)
    while True:
        client, _ = server.accept()
        try:
            upstream = socket.create_connection(("127.0.0.1", target))
        except OSError:
            client.close()
            continue
        pending = []
        threading.Thread(target=pump, args=(client, upstream, listen, pending, True), daemon=True).start()
        threading.Thread(target=pump, args=(upstream, client, listen, pending, False), daemon=True).start()


for spec in sys.argv[2:]:
    a, b = spec.split(":")
    threading.Thread(target=serve, args=(int(a), int(b)), daemon=True).start()
while True:
    time.sleep(3600)
