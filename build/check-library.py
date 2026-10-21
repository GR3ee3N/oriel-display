"""Checks the add-on's libraries: architecture, requirements, entry points.

    build/check-library.py BOX_ENV LIBRARY...

Kodi's process on the box must provide every library and symbol version it
needs (BOX_ENV says what CoreELEC has), and Kodi finds a binary add-on by its
ADDON_* exports.
"""
from pathlib import Path
import re
import subprocess
import sys

EXPORTS = ("ADDON_Create", "ADDON_GetTypeVersion", "ADDON_GetTypeMinVersion")


def box(env):
    return dict(line.split("=", 1) for line in env.read_text().splitlines() if line and not line.startswith("#"))


def version(text):
    return tuple(map(int, text.split(".")))


def problems(dynamic, versions, symbols, env):
    target = box(env)
    system = re.compile(target["LIBRARIES"])
    found = [f"needs {name}, which the box does not provide"
             for name in re.findall(r"\(NEEDED\).*\[(.*?)\]", dynamic) if not system.fullmatch(name)]
    # Only requirements, not the versions a library itself exports.
    needs = versions.partition("Version needs section")[2]
    for name, required in re.findall(r"\b(GLIBC|GLIBCXX|CXXABI|GCC)_([0-9]+(?:\.[0-9]+)*)\b", needs):
        if version(required) > version(target[name]):
            found.append(f"requires {name}_{required}; the box provides {name}_{target[name]}")
    exported = set(re.findall(r"\bFUNC\s+GLOBAL\s+DEFAULT\s+\d+\s+(\w+)", symbols))
    found += [f"does not export {name}" for name in EXPORTS if name not in exported]
    return found


def readelf(*args):
    return subprocess.check_output(["aarch64-linux-gnu-readelf", *args], text=True)


def main():
    env, *libraries = map(Path, sys.argv[1:])
    found = []
    for library in libraries:
        problems_here = [] if re.search(r"Machine:\s+AArch64", readelf("-h", str(library))) else ["is not an ARM64 binary"]
        problems_here += problems(readelf("-d", str(library)), readelf("-V", str(library)), readelf("--dyn-syms", "-W", str(library)), env)
        found += [f"{library.name} {problem}" for problem in problems_here]
    if found:
        raise SystemExit("\n".join(found))


if __name__ == "__main__":
    main()
