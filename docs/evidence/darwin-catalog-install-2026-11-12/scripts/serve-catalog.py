"""serve-catalog.py <dir> <cert> <key> [port] — the local test catalog over HTTPS on 127.0.0.1."""
import functools, http.server, ssl, sys
d, cert, key = sys.argv[1:4]
port = int(sys.argv[4]) if len(sys.argv) > 4 else 8443
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(cert, key)
srv = http.server.ThreadingHTTPServer(("127.0.0.1", port), functools.partial(http.server.SimpleHTTPRequestHandler, directory=d))
srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
print(f"serving {d} on https://localhost:{port}/", flush=True)
srv.serve_forever()
