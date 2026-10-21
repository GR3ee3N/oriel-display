"""Wait for Kodi to register an add-on, then verify that it is enabled.

    enable.py ADDON [--reload]

--reload disables an enabled add-on first, so Kodi loads its new library.
"""
import codecs
import json
import socket
import sys
import time


class Kodi:
    def __init__(self):
        self.sock = socket.create_connection(("127.0.0.1", 9090), timeout=10)
        self.buf = ""
        self.decoder = json.JSONDecoder()
        self.utf8 = codecs.getincrementaldecoder("utf-8")()
        self.next_id = 1

    def call(self, method, params):
        rid = self.next_id
        self.next_id += 1
        request = {"jsonrpc": "2.0", "id": rid, "method": method, "params": params}
        self.sock.sendall(json.dumps(request).encode())
        while True:
            text = self.buf.lstrip()
            if text:
                try:
                    message, end = self.decoder.raw_decode(text)
                except json.JSONDecodeError:
                    pass
                else:
                    self.buf = text[end:]
                    if message.get("id") == rid:
                        if "error" in message:
                            raise RuntimeError(f"{method}: {message['error']}")
                        return message.get("result")
                    continue
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("Kodi closed the connection")
            self.buf += self.utf8.decode(chunk)


def enable(addon, reload=False):
    kodi = Kodi()
    try:
        if reload:
            try:
                kodi.call("Addons.SetAddonEnabled", {"addonid": addon, "enabled": False})
            except RuntimeError:
                pass  # Not registered yet: a first install.
        deadline = time.monotonic() + 10
        while True:
            try:
                details = kodi.call("Addons.GetAddonDetails", {"addonid": addon, "properties": ["enabled"]})
                if details["addon"]["enabled"]:
                    return
                kodi.call("Addons.SetAddonEnabled", {"addonid": addon, "enabled": True})
            except RuntimeError:
                pass  # UpdateLocalAddons may not have registered the add-on yet.
            if time.monotonic() >= deadline:
                raise RuntimeError(f"Kodi did not enable {addon}")
            time.sleep(0.25)
    finally:
        kodi.sock.close()


if __name__ == "__main__":
    enable(sys.argv[1], "--reload" in sys.argv[2:])
