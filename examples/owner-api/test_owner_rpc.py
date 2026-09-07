"""Run with: python3 -m unittest discover -s examples/owner-api -v."""

import json
import socket
import tempfile
import threading
import unittest

from owner_rpc import MAX_BODY_BYTES, OwnerRpc, RpcError


class OwnerRpcTests(unittest.TestCase):
    def exchange(self, response, *, parameter=None):
        # A real HTTP-over-UDS peer verifies the example's wire framing. No node,
        # chain, wallet, or private credentials are used.
        with tempfile.TemporaryDirectory(prefix="owner-api-") as directory:
            path = directory + "/node.sock"
            requests = []
            failures = []
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
                listener.bind(path)
                listener.listen(1)
                listener.settimeout(3)

                def serve():
                    try:
                        with listener.accept()[0] as stream:
                            stream.settimeout(3)
                            with stream.makefile("rb") as reader:
                                self.assertEqual(reader.readline(), b"POST / HTTP/1.1\r\n")
                                headers = {}
                                while (line := reader.readline()) not in (b"\r\n", b""):
                                    name, value = line.decode().split(":", 1)
                                    headers[name.lower()] = value.strip()
                                requests.append(json.loads(reader.read(int(headers["content-length"]))))
                            body = json.dumps(response).encode()
                            stream.sendall(f"HTTP/1.1 200 OK\r\nContent-Length: {len(body)}\r\nConnection: close\r\n\r\n".encode() + body)
                    except Exception as error:  # Propagate peer-thread assertions.
                        failures.append(error)

                peer = threading.Thread(target=serve)
                peer.start()
                try:
                    return OwnerRpc(path, timeout=2).call("taker_swap_initiate_v1", parameter or {})
                finally:
                    peer.join(4)
                    self.assertFalse(peer.is_alive())
                    if failures:
                        raise failures[0]
                    self.assertEqual(len(requests), 1)
                    self.assertEqual(requests[0], {
                        "jsonrpc": "2.0", "id": 1, "method": "taker_swap_initiate_v1",
                        "params": [parameter or {}]
                    })

    def test_exact_large_integers_and_null_results(self):
        amount = 2**100 + 17
        self.assertEqual(self.exchange({"jsonrpc": "2.0", "id": 1, "result": amount},
                                       parameter={"expected_lez_units": amount}), amount)
        self.assertIsNone(self.exchange({"jsonrpc": "2.0", "id": 1, "result": None}))

    def test_structured_remote_error_is_not_retried(self):
        with self.assertRaises(RpcError) as caught:
            self.exchange({"jsonrpc": "2.0", "id": 1, "error": {
                "code": -32009, "message": "conflict", "data": {"category": "conflict"}
            }})
        self.assertEqual(caught.exception.code, -32009)
        self.assertEqual(caught.exception.data, {"category": "conflict"})

    def test_malformed_and_oversized_responses_fail(self):
        for response in [
            {"jsonrpc": "2.0", "id": 2, "result": {}},
            {"jsonrpc": "2.0", "id": 1, "result": {}, "error": {}},
            {"jsonrpc": "2.0", "id": 1},
            {"jsonrpc": "2.0", "id": 1, "result": "x" * MAX_BODY_BYTES},
        ]:
            with self.subTest(response=str(response)[:100]), self.assertRaises(ValueError):
                self.exchange(response)

    def test_invalid_requests_fail_before_connecting(self):
        client = OwnerRpc("/does/not/exist")
        for method, parameter in [("", {}), ("maker_health", []), ("maker_health", {"x": "x" * MAX_BODY_BYTES})]:
            with self.assertRaises(ValueError):
                client.call(method, parameter)


if __name__ == "__main__":
    unittest.main()
