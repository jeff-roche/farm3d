#!/usr/bin/env python3
"""A TCP proxy that injects network faults between farm3d and a Moonraker.

A0.1 (#9) uses it to test disconnect and reconnect without touching the
printer. Point farm3d (or the live harness) at the proxy's listen address
instead of the printer. Then change the mode while it runs:

    netfault.py --listen 127.0.0.1:17125 --target 192.0.2.10:7125 --control /tmp/netfault.mode
    echo drop   > /tmp/netfault.mode   # close every connection, refuse new ones
    echo freeze > /tmp/netfault.mode   # keep sockets open, relay nothing (a pulled cable)
    echo pass   > /tmp/netfault.mode   # relay normally again

`drop` is what a Moonraker restart or a host reboot looks like to a client.
`freeze` is what a Wi-Fi drop or a pulled cable looks like: no FIN, no RST,
just silence. Every mode change is printed with a UTC timestamp so it can be
lined up with the harness logs. Standard library only.
"""

import argparse
import asyncio
import datetime
import pathlib


def stamp(message):
    now = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="milliseconds")
    print(f"{now} netfault: {message}", flush=True)


class Proxy:
    def __init__(self, target_host, target_port, control):
        self.target_host = target_host
        self.target_port = target_port
        self.control = pathlib.Path(control)
        self.mode = "pass"
        self.connections = set()

    def read_mode(self):
        try:
            mode = self.control.read_text().strip() or "pass"
        except FileNotFoundError:
            mode = "pass"
        if mode not in ("pass", "drop", "freeze"):
            mode = "pass"
        return mode

    async def watch_control(self):
        while True:
            mode = self.read_mode()
            if mode != self.mode:
                stamp(f"mode {self.mode} -> {mode} ({len(self.connections)} open connections)")
                self.mode = mode
                if mode == "drop":
                    for writers in list(self.connections):
                        for writer in writers:
                            writer.transport.abort()
                    self.connections.clear()
            await asyncio.sleep(0.2)

    async def relay(self, reader, writer):
        try:
            while True:
                data = await reader.read(65536)
                if not data:
                    break
                while self.mode == "freeze":
                    await asyncio.sleep(0.2)
                if self.mode == "drop":
                    break
                writer.write(data)
                await writer.drain()
        except (ConnectionError, OSError):
            pass
        finally:
            writer.transport.abort()

    async def handle(self, client_reader, client_writer):
        if self.mode == "drop":
            stamp("refused a connection")
            client_writer.transport.abort()
            return
        try:
            upstream_reader, upstream_writer = await asyncio.open_connection(
                self.target_host, self.target_port
            )
        except OSError as error:
            stamp(f"upstream connect failed: {error}")
            client_writer.transport.abort()
            return
        pair = (client_writer, upstream_writer)
        self.connections.add(pair)
        stamp(f"connection opened ({len(self.connections)} open)")
        await asyncio.gather(
            self.relay(client_reader, upstream_writer),
            self.relay(upstream_reader, client_writer),
        )
        self.connections.discard(pair)
        stamp(f"connection closed ({len(self.connections)} open)")


async def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--listen", default="127.0.0.1:17125")
    parser.add_argument("--target", required=True, help="Moonraker host:port")
    parser.add_argument("--control", default="/tmp/netfault.mode")
    args = parser.parse_args()
    listen_host, listen_port = args.listen.rsplit(":", 1)
    target_host, target_port = args.target.rsplit(":", 1)
    proxy = Proxy(target_host, int(target_port), args.control)
    proxy.mode = proxy.read_mode()
    server = await asyncio.start_server(proxy.handle, listen_host, int(listen_port))
    stamp(f"listening on {args.listen}, forwarding to {args.target}, mode {proxy.mode}, control {args.control}")
    async with server:
        await asyncio.gather(server.serve_forever(), proxy.watch_control())


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
