#!/usr/bin/env python3
"""chat-bridge.py tcp-to-unix <port> <socket> | unix-to-tcp <socket> <port>

The devnet's "local" transport hands the Taker the Maker's Chat socket
through a volume both role containers mount; a container cannot connect to a
Unix socket of the macOS host (or the host to one in the Docker VM), so with
one role native and the other in its container the socket crosses the
boundary as TCP on this host's loopback:

  tcp-to-unix: listen on 127.0.0.1:<port>, connect each client to <socket>
               (the native Maker's chat.sock; a forwarder container in the
               Taker's maker_socket volume dials host.docker.internal:<port>)
  unix-to-tcp: listen on <socket> (0600), connect each client to
               127.0.0.1:<port> (a forwarder container publishing the Docker
               Maker's chat.sock there), for a native Taker.

Bytes are copied unchanged in both directions; nothing is parsed or logged
but connection counts."""
import asyncio
import os
import sys
import time


async def pipe(reader, writer):
    try:
        while data := await reader.read(65536):
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


count = 0


async def main():
    mode = sys.argv[1]

    async def handle(reader, writer):
        global count
        count += 1
        try:
            if mode == "tcp-to-unix":
                r2, w2 = await asyncio.open_unix_connection(sys.argv[3])
            else:
                r2, w2 = await asyncio.open_connection("127.0.0.1", int(sys.argv[3]))
        except OSError as error:
            print(f"{time.strftime('%H:%M:%S', time.gmtime())} connect failed: {error}", flush=True)
            writer.close()
            return
        await asyncio.gather(pipe(reader, w2), pipe(r2, writer))

    if mode == "tcp-to-unix":
        server = await asyncio.start_server(handle, "127.0.0.1", int(sys.argv[2]))
    elif mode == "unix-to-tcp":
        path = sys.argv[2]
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
        old = os.umask(0o077)
        server = await asyncio.start_unix_server(handle, path)
        os.umask(old)
    else:
        sys.exit(__doc__)
    print(f"{time.strftime('%H:%M:%S', time.gmtime())} {mode} up", flush=True)

    async def report():
        while True:
            await asyncio.sleep(300)
            print(f"{time.strftime('%H:%M:%S', time.gmtime())} {count} connections so far", flush=True)
    asyncio.ensure_future(report())
    async with server:
        await server.serve_forever()


asyncio.run(main())
