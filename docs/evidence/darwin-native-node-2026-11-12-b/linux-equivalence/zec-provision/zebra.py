# A fake Zebra JSON-RPC for the Taker-payment bookkeeping zec-provision.sh does in Python.
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        result = {"getrawtransaction": {"height": 150, "vout": [{"n": 1, "scriptPubKey": {"addresses": ["tmTaker"]}}]},
                  "gettxout": None}.get(body["method"])
        data = json.dumps({"jsonrpc": "2.0", "id": 1, "result": result}).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.end_headers(); self.wfile.write(data)
    def log_message(self, *a): pass
HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
