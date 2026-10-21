#!/usr/bin/env python3
"""Drives vfs.oriel.display's control socket on the box, for testing; prints
what the add-on sends. Steps run in order:

    control.py open-browse[=MARK] | open-player | close-browse | close-player
               | started | draw-on | draw-off | wait=SECONDS ...

Example: control.py open-browse wait=5 close-browse wait=1
"""
import json
import select
import socket
import sys
import time

SOCKET = b"\0oriel-display-control"
VERSION = 1


def main():
    steps = sys.argv[1:]
    if not steps:
        sys.exit(__doc__)
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
    sock.connect(SOCKET)
    send(sock, {"hello": VERSION})
    print_events(sock, 1)
    for step in steps:
        name, _, value = step.partition("=")
        if name == "wait":
            print_events(sock, float(value))
            continue
        message = {
            "open-browse": {"open": "browse", "mark": value or None},
            "open-player": {"open": "player"},
            "close-browse": {"close": "browse"},
            "close-player": {"close": "player"},
            "started": {"started": True},
            "draw-on": {"draw": True},
            "draw-off": {"draw": False},
        }.get(name)
        if message is None:
            sys.exit(f"unknown step {step}")
        send(sock, message)
    print_events(sock, 1)


def send(sock, message):
    print(f"{time.monotonic():.3f} > {json.dumps(message)}", flush=True)
    sock.send(json.dumps(message).encode())


def print_events(sock, seconds):
    end = time.monotonic() + seconds
    while (left := end - time.monotonic()) > 0:
        if not select.select([sock], [], [], left)[0]:
            continue
        data = sock.recv(4096)
        if not data:
            print("add-on closed the connection", flush=True)
            sys.exit(1)
        print(f"{time.monotonic():.3f} < {data.decode()}", flush=True)


if __name__ == "__main__":
    main()
