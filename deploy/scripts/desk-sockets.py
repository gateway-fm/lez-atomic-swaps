#!/usr/bin/env python3
"""Owner-only host sockets for native Basecamp desks, bridged to the Compose Nodes.

The desks' backends accept one thing: an absolute path to a Unix socket that
the current user owns with mode 0600. The Nodes' owner sockets live inside
their containers, and Docker Desktop cannot bind-mount a container's socket
onto a macOS host. This bridge listens on host sockets with exactly that
ownership and mode and relays each connection into the Node's container
(`docker exec ... socat`), so a native desk reaches its Node as if it ran
next to it. Every connection is authorised by the user's Docker access, the
same authority that owns the containers.

    deploy/scripts/desk-sockets.py [--dir DIR] [--prefix lez] [maker] [taker]

Prints the two environment lines a desk process needs, then serves until
interrupted. Python 3.8+ and Docker only.
"""
import argparse
import asyncio
import os
import signal
import stat
import sys

ROLES = ("maker", "taker")


async def pump(reader, writer, finish):
    try:
        while True:
            chunk = await reader.read(65536)
            if not chunk:
                break
            writer.write(chunk)
            await writer.drain()
    except (ConnectionError, BrokenPipeError, asyncio.CancelledError):
        pass
    finally:
        finish()


async def relay(role, container, node_socket, reader, writer):
    process = await asyncio.create_subprocess_exec(
        "docker", "exec", "-i", container, "socat", "STDIO", f"UNIX-CONNECT:{node_socket}",
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
    )

    def close_stdin():
        if process.stdin and not process.stdin.is_closing():
            process.stdin.close()

    def close_client():
        if not writer.is_closing():
            writer.close()

    await asyncio.gather(
        pump(reader, process.stdin, close_stdin),
        pump(process.stdout, writer, close_client),
    )
    await process.wait()
    print(f"{role}: relayed one connection (socat exit {process.returncode})", flush=True)


def owner_only(path):
    info = os.lstat(path)
    return stat.S_ISSOCK(info.st_mode) and info.st_uid == os.geteuid() and stat.S_IMODE(info.st_mode) == 0o600


async def serve(directory, prefix, roles):
    os.makedirs(directory, mode=0o700, exist_ok=True)
    os.chmod(directory, 0o700)
    servers = []
    for role in roles:
        path = os.path.join(directory, f"{role}.sock")
        if os.path.lexists(path):
            os.unlink(path)
        container = f"{prefix}-{role}-node"
        node_socket = f"/run/lez/{role}/node.sock"
        server = await asyncio.start_unix_server(
            lambda reader, writer, role=role, container=container, node_socket=node_socket:
                relay(role, container, node_socket, reader, writer),
            path=path,
        )
        os.chmod(path, 0o600)
        if not owner_only(path):
            raise SystemExit(f"{path} is not an owner-only socket")
        servers.append(server)
        print(f"export LEZ_{role.upper()}_RPC_SOCKET={path}", flush=True)
    print(f"serving; the desks' backends may connect (Ctrl-C stops the bridge)", file=sys.stderr, flush=True)
    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for signum in (signal.SIGINT, signal.SIGTERM):
        loop.add_signal_handler(signum, stop.set)
    await stop.wait()
    for server in servers:
        server.close()
        await server.wait_closed()


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dir", default=os.path.join(os.path.expanduser("~"), ".lez", "desks"),
                        help="directory for the host sockets (default: ~/.lez/desks; keep it short, "
                             "macOS limits a socket path to 104 bytes)")
    parser.add_argument("--prefix", default=os.environ.get("LEZ_CONTAINER_PREFIX", "lez"),
                        help="container name prefix of the Compose stack (default: lez)")
    # No `choices=`: with `nargs="*"` argparse checks the list default against
    # them as one value, so the documented bare command exited 2 on every Python
    # before 3.14. The roles are validated below instead, one by one.
    parser.add_argument("roles", nargs="*", metavar="ROLE",
                        help=f"roles to serve (default: {' '.join(ROLES)})")
    arguments = parser.parse_args()
    roles = arguments.roles or list(ROLES)
    unknown = [role for role in roles if role not in ROLES]
    if unknown:
        parser.error(f"invalid role(s): {' '.join(unknown)} (choose from {', '.join(ROLES)})")
    directory = os.path.abspath(arguments.dir)
    for role in roles:
        if len(os.path.join(directory, f"{role}.sock").encode()) > 100:
            raise SystemExit(f"{directory} is too long for a macOS socket path; pass a shorter --dir")
    asyncio.run(serve(directory, arguments.prefix, roles))


if __name__ == "__main__":
    main()
