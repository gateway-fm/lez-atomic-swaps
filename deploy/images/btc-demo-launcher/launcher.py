#!/usr/bin/env python3
"""Allowlisted local-demo launcher; the only component with Docker authority."""

from __future__ import annotations

import base64
import http.client
import io
import json
import os
import pathlib
import re
import socket
import socketserver
import tarfile
import time
import urllib.parse


SOCKET_PATH = pathlib.Path(os.environ.get(
    "LEZ_BTC_LAUNCHER_SOCKET", "/run/lez-btc-launcher/launcher.sock"))
DOCKER_SOCKET = os.environ.get("DOCKER_HOST_SOCKET", "/var/run/docker.sock")
DOCKER_API = os.environ.get("DOCKER_API_VERSION", "/v1.41")
RUNNER_NAME = os.environ.get("LEZ_M3_RUNNER_CONTAINER", "lez-runner-arm")
RUNNER_REPO = os.environ["LEZ_M3_RUNNER_REPO_IN_CONTAINER"]
RUNNER_EXPORT_SCRIPT = "/tmp/lez-export-btc-ui-evidence.sh"
CONTROLLER_UID = int(os.environ.get("LEZ_BTC_CONTROLLER_UID", "4713"))
CONTROLLER_GID = int(os.environ.get("LEZ_BTC_CONTROLLER_GID", "4713"))
MAXIMUM_REQUEST_BYTES = 4 * 1024 * 1024
MAXIMUM_FILE_BYTES = 1024 * 1024
MAXIMUM_RUN_SECONDS = 2 * 60 * 60

RUN_RE = re.compile(r"^m5arm-[0-9]{10}$")
EXEC_RE = re.compile(r"^[0-9a-f]{64}$")
IDENTIFIER_RE = re.compile(r"^[A-Za-z0-9._-]{1,128}$")
DIRECTIONS = {"taker_sells_foreign", "taker_sells_lez"}
ACTIONS = {"lock_btc", "fund_lez", "claim_lez", "claim_btc"}
TIMESTAMP_RE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$")


def compact(value: object) -> str:
    return json.dumps(value, sort_keys=True, separators=(",", ":"))


def require_exact(value: dict, fields: set[str]) -> None:
    if set(value) != fields:
        raise ValueError("launcher request fields do not match the versioned schema")


class DockerUnixConnection(http.client.HTTPConnection):
    def connect(self) -> None:
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(DOCKER_SOCKET)


def docker_request(method: str, route: str, payload: object | bytes | None = None,
                   content_type: str = "application/json") -> tuple[int, bytes]:
    if isinstance(payload, bytes):
        body = payload
    elif payload is None:
        body = None
    else:
        body = compact(payload).encode()
    headers = {"Content-Type": content_type}
    if body is not None:
        headers["Content-Length"] = str(len(body))
    connection = DockerUnixConnection("localhost", timeout=15)
    try:
        connection.request(method, DOCKER_API + route, body=body, headers=headers)
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def docker_json(method: str, route: str, payload: object | None = None,
                expected: tuple[int, ...] = (200, 201)) -> object:
    status, raw = docker_request(method, route, payload)
    if status not in expected:
        detail = raw.decode("utf-8", "replace")[:300]
        raise RuntimeError(f"Docker API operation returned {status}: {detail}")
    return json.loads(raw) if raw else {}


def runner_info() -> dict:
    encoded = urllib.parse.quote(RUNNER_NAME, safe="")
    status, raw = docker_request("GET", f"/containers/{encoded}/json")
    if status != 200:
        return {"ready": False, "busy": False, "reason": "runner container is unavailable"}
    running = json.loads(raw).get("State", {}).get("Running") is True
    busy = False
    if running:
        top_status, top_raw = docker_request("GET", f"/containers/{encoded}/top?ps_args=-eo%20args")
        if top_status == 200:
            processes = json.loads(top_raw).get("Processes", [])
            busy = any(
                "run-m3-actor-local-poc.sh" in " ".join(process)
                or "lez-interactive-m3-outer" in " ".join(process)
                or "lez-run-full-btc-ui" in " ".join(process)
                for process in processes
            )
    return {
        "ready": running,
        "busy": busy,
        "reason": "an M3 run is active" if busy else "ready" if running
        else "runner container is not running",
    }


def upload_bundle(files: dict[str, bytes], directory: str) -> None:
    archive = io.BytesIO()
    with tarfile.open(fileobj=archive, mode="w") as bundle:
        for name, data in files.items():
            if "/" in name or len(data) > MAXIMUM_FILE_BYTES:
                raise ValueError("staged launcher file is invalid")
            item = tarfile.TarInfo(name=name)
            item.size = len(data)
            item.mode = 0o700 if name.endswith(".sh") else 0o600
            item.uid = 501
            item.gid = 20
            item.uname = "lez"
            item.gname = "dialout"
            item.mtime = int(time.time())
            bundle.addfile(item, io.BytesIO(data))
    encoded = urllib.parse.quote(RUNNER_NAME, safe="")
    status, raw = docker_request(
        "PUT", f"/containers/{encoded}/archive?path={urllib.parse.quote(directory)}",
        archive.getvalue(), "application/x-tar")
    if status != 200:
        raise RuntimeError(
            f"Docker API could not stage allowlisted demo files ({status}): "
            + raw.decode("utf-8", "replace")[:300])


def create_exec(command: list[str], environment: list[str]) -> str:
    encoded = urllib.parse.quote(RUNNER_NAME, safe="")
    result = docker_json("POST", f"/containers/{encoded}/exec", {
        "AttachStdout": False, "AttachStderr": False, "Tty": False,
        "Cmd": command, "Env": environment, "WorkingDir": RUNNER_REPO,
    })
    exec_id = str(result.get("Id", ""))
    if not EXEC_RE.fullmatch(exec_id):
        raise RuntimeError("Docker API returned an invalid execution identity")
    docker_json("POST", f"/exec/{exec_id}/start", {"Detach": True, "Tty": False})
    return exec_id


def wait_exec(exec_id: str) -> int:
    if not EXEC_RE.fullmatch(exec_id):
        raise ValueError("execution identity is invalid")
    deadline = time.monotonic() + MAXIMUM_RUN_SECONDS
    while time.monotonic() < deadline:
        inspection = dict(docker_json("GET", f"/exec/{exec_id}/json"))
        if inspection.get("Running") is not True:
            value = inspection.get("ExitCode")
            return int(value) if isinstance(value, int) else -1
        time.sleep(2)
    raise RuntimeError("allowlisted runner exceeded its bounded deadline")


def run_swap_job(request: dict) -> dict:
    require_exact(request, {
        "schema_version", "operation", "run_id", "direction", "offer_id",
        "reservation_id", "maker_wallet_id", "taker_wallet_id", "files",
    })
    run_id = request["run_id"]
    direction = request["direction"]
    if not isinstance(run_id, str) or not RUN_RE.fullmatch(run_id) or direction not in DIRECTIONS:
        raise ValueError("run identity or direction is invalid")
    for field in ("offer_id", "reservation_id", "maker_wallet_id", "taker_wallet_id"):
        if not isinstance(request[field], str) or not IDENTIFIER_RE.fullmatch(request[field]):
            raise ValueError(f"{field} is invalid")
    expected_names = {
        f"lez-run-full-btc-ui-{run_id}.sh",
        f"lez-interactive-m3-outer-{run_id}.sh",
        f"lez-interactive-m3-direction-{run_id}.sh",
        pathlib.Path(RUNNER_EXPORT_SCRIPT).name,
    }
    encoded_files = request["files"]
    if not isinstance(encoded_files, dict) or set(encoded_files) != expected_names:
        raise ValueError("run job files are not the exact allowlisted bundle")
    files = {}
    for name, encoded in encoded_files.items():
        if not isinstance(encoded, str):
            raise ValueError("run job file encoding is invalid")
        files[name] = base64.b64decode(encoded, validate=True)
    upload_bundle(files, "/tmp")
    environment = [
        f"LEZ_M3_RUN_ID={run_id}",
        "LEZ_M3_INTERACTIVE=1",
        "LEZ_M3_ATTACH=1",
        "LEZ_INTERACTIVE_UI_GATES=1",
        f"LEZ_INTERACTIVE_DIRECTION={direction}",
        f"LEZ_INTERACTIVE_REPO_ROOT={RUNNER_REPO}",
        f"LEZ_INTERACTIVE_OFFER_ID={request['offer_id']}",
        f"LEZ_INTERACTIVE_RESERVATION_ID={request['reservation_id']}",
        f"LEZ_INTERACTIVE_MAKER_WALLET={request['maker_wallet_id']}",
        f"LEZ_INTERACTIVE_TAKER_WALLET={request['taker_wallet_id']}",
    ]
    exec_id = create_exec(["bash", f"/tmp/lez-run-full-btc-ui-{run_id}.sh"], environment)
    return {"kind": "RunSwapResultV1", "run_id": run_id, "exec_id": exec_id}


def approve_action(request: dict) -> dict:
    require_exact(request, {
        "schema_version", "operation", "run_id", "direction", "role", "action",
        "expected_revision", "approved_at",
    })
    run_id = request["run_id"]
    direction = request["direction"]
    action = request["action"]
    role = request["role"]
    revision = request["expected_revision"]
    if not isinstance(run_id, str) or not RUN_RE.fullmatch(run_id) or direction not in DIRECTIONS:
        raise ValueError("approval run identity or direction is invalid")
    if action not in ACTIONS or role not in {"maker", "taker"} or revision not in range(4):
        raise ValueError("approval role, action, or revision is invalid")
    if not isinstance(request["approved_at"], str) \
            or not TIMESTAMP_RE.fullmatch(request["approved_at"]):
        raise ValueError("approval timestamp is invalid")
    permit = compact({
        "schema_version": 1, "run_id": run_id, "role": role, "action": action,
        "expected_revision": revision, "approved_at": request["approved_at"],
    }).encode() + b"\n"
    directory = (
        f"{RUNNER_REPO}/.e2e/{run_id}/m3-actor-poc/private/directions/"
        f"{direction}/interactive-gates"
    )
    upload_bundle({f"{action}.permit.json": permit}, directory)
    return {"kind": "ApproveSwapActionResultV1", "approved": True}


def collect_result(request: dict) -> dict:
    require_exact(request, {"schema_version", "operation", "run_id", "direction"})
    run_id = request["run_id"]
    direction = request["direction"]
    if not isinstance(run_id, str) or not RUN_RE.fullmatch(run_id) or direction not in DIRECTIONS:
        raise ValueError("result run identity or direction is invalid")
    source = f"{RUNNER_REPO}/.e2e/{run_id}/m3-actor-poc/evidence"
    generated = f"{source}/m3-btc-ui-evidence.json"
    exec_id = create_exec(
        ["bash", RUNNER_EXPORT_SCRIPT, source, generated],
        [f"LEZ_UI_EVIDENCE_DIRECTION={direction}"],
    )
    return {
        "kind": "CollectSwapResultV1", "run_id": run_id,
        "exec_id": exec_id, "exit_code": wait_exec(exec_id),
    }


def dispatch(request: object) -> dict:
    if not isinstance(request, dict) or request.get("schema_version") != 1:
        raise ValueError("unsupported launcher request schema")
    operation = request.get("operation")
    if operation == "runner_status":
        require_exact(request, {"schema_version", "operation"})
        return {"kind": "RunnerStatusResultV1", **runner_info()}
    if operation == "run_swap":
        return run_swap_job(request)
    if operation == "wait_swap":
        require_exact(request, {"schema_version", "operation", "exec_id"})
        return {"kind": "WaitSwapResultV1", "exit_code": wait_exec(request["exec_id"])}
    if operation == "approve_action":
        return approve_action(request)
    if operation == "collect_result":
        return collect_result(request)
    raise ValueError("launcher operation is not allowlisted")


class Handler(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        try:
            raw = self.rfile.readline(MAXIMUM_REQUEST_BYTES + 1)
            if not raw or len(raw) > MAXIMUM_REQUEST_BYTES or not raw.endswith(b"\n"):
                raise ValueError("launcher request is empty, oversized, or unterminated")
            result = dispatch(json.loads(raw))
            response = {"schema_version": 1, "ok": True, "result": result}
        except Exception as error:
            response = {"schema_version": 1, "ok": False, "error": str(error)[:300]}
        self.wfile.write(compact(response).encode() + b"\n")


class Server(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True


def call_healthcheck() -> None:
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    client.settimeout(3)
    client.connect(str(SOCKET_PATH))
    client.sendall(b'{"operation":"runner_status","schema_version":1}\n')
    response = json.loads(client.makefile("rb").readline(65537))
    if response.get("ok") is not True:
        raise RuntimeError("launcher health request failed")


def hand_to_controller(path: pathlib.Path, mode: int) -> None:
    """Keeps root ownership of `path` and opens it to the controller's group.

    The launcher runs as root with every capability but CAP_CHOWN dropped, so
    it must stay the owner to unlink and bind the socket on restart; the
    controller reaches the socket through group 4713 (directory 0710, socket
    0660). Mode and group change only when they differ, so restarts are
    idempotent.
    """
    current = os.stat(path)
    if current.st_mode & 0o7777 != mode:
        os.chmod(path, mode)
    if current.st_gid != CONTROLLER_GID:
        os.chown(path, 0, CONTROLLER_GID)


def main() -> None:
    if len(os.sys.argv) == 2 and os.sys.argv[1] == "--healthcheck":
        call_healthcheck()
        return
    if len(os.sys.argv) != 1:
        raise SystemExit("launcher accepts only --healthcheck")
    SOCKET_PATH.parent.mkdir(parents=True, exist_ok=True)
    hand_to_controller(SOCKET_PATH.parent, 0o710)
    try:
        SOCKET_PATH.unlink()
    except FileNotFoundError:
        pass
    server = Server(str(SOCKET_PATH), Handler)
    hand_to_controller(SOCKET_PATH, 0o660)
    server.serve_forever()


if __name__ == "__main__":
    main()
