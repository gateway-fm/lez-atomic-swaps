#!/usr/bin/env python3
"""rpc-delay-proxy.py — a test tool: chain RPC as if it were on another host.

    rpc-delay-proxy.py --delay-ms 100 --log calls.tsv \\
        3040=sequencer:3040 8779=network:8779 18443=bitcoin-core:18443 18232=zebra:18232

Listens on each LISTEN port (all interfaces) and forwards every connection to
HOST:PORT, holding each chunk of a request for half the delay and each chunk
of a response for the other half, in order: every request/response round trip
takes about --delay-ms longer, as through an SSH tunnel to a remote RPC
provider. Each JSON-RPC method seen in a request is logged as one line of
<unix time>\\t<listen port>\\t<method> to --log (or stdout), so the calls a
Node makes can be counted per chain and per time window.

The devnet's Nodes reach the chains through the Compose service names; run
this as a container on the stack's network and point the Nodes' chain hosts
(LEZ_SEQUENCER_HOST, LEZ_INDEXER_HOST, LEZ_BITCOIN_CORE_HOST, LEZ_ZEBRA_HOST)
at it. It has no other use: nothing in a swap needs it.
"""
import argparse
import asyncio
import re
import sys
import time

METHOD = re.compile(rb'"method"\s*:\s*"([A-Za-z0-9_.]+)"')


async def pump(reader, writer, delay, on_chunk=None):
    """Copy reader to writer, each chunk `delay` seconds after it arrived."""
    queue = asyncio.Queue()

    async def receive():
        while True:
            chunk = await reader.read(65536)
            await queue.put((time.monotonic() + delay, chunk))
            if not chunk:
                return
            if on_chunk:
                on_chunk(chunk)

    async def send():
        while True:
            due, chunk = await queue.get()
            pause = due - time.monotonic()
            if pause > 0:
                await asyncio.sleep(pause)
            if not chunk:
                break
            writer.write(chunk)
            await writer.drain()
        if writer.can_write_eof():
            writer.write_eof()

    try:
        await asyncio.gather(receive(), send())
    except (ConnectionError, asyncio.IncompleteReadError):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--delay-ms', type=float, default=100.0)
    parser.add_argument('--log', default='-')
    parser.add_argument('routes', nargs='+', metavar='LISTEN=HOST:PORT')
    args = parser.parse_args()
    half = args.delay_ms / 2000.0
    log = sys.stdout if args.log == '-' else open(args.log, 'a', buffering=1, encoding='utf-8')

    def handler(listen, host, port):
        async def handle(client_reader, client_writer):
            try:
                upstream_reader, upstream_writer = await asyncio.open_connection(host, port)
            except OSError:
                client_writer.close()
                return

            def count(chunk):
                now = f'{time.time():.3f}'
                for method in METHOD.findall(chunk):
                    log.write(f'{now}\t{listen}\t{method.decode()}\n')

            await asyncio.gather(pump(client_reader, upstream_writer, half, count),
                                 pump(upstream_reader, client_writer, half))
            for writer in (upstream_writer, client_writer):
                writer.close()
        return handle

    async def serve():
        servers = []
        for route in args.routes:
            listen, _, target = route.partition('=')
            host, _, port = target.rpartition(':')
            servers.append(await asyncio.start_server(handler(int(listen), host, int(port)), '0.0.0.0', int(listen)))
        print(f'rpc-delay-proxy: +{args.delay_ms:g} ms per round trip on {", ".join(args.routes)}', flush=True)
        await asyncio.gather(*(server.serve_forever() for server in servers))

    asyncio.run(serve())


if __name__ == '__main__':
    main()
