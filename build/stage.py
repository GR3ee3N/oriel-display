"""Writes the add-on's addon.xml to stdout, filled in for a release.

    build/stage.py TEMPLATE VERSIONS_H VERSION

VERSIONS_H is the dev-kit's kodi/versions.h. Each @NAME@ is a version from it
(the API minimums Kodi checks against the imports), and @VERSION@ is ours.
"""
import re
import sys
from pathlib import Path


def devkit_versions(header):
    return dict(re.findall(r'^#define\s+(ADDON_[A-Z_]+)\s+"([0-9.]+)"', header, re.M))


def render(template, header, version):
    known = devkit_versions(header)

    def value(match):
        name = match.group(1)
        if name == "VERSION":
            return version
        if name not in known:
            raise SystemExit(f"addon.xml asks for {name}, which the dev-kit does not define")
        return known[name]

    return re.sub(r"@([A-Z_]+)@", value, template)


def main():
    template, header, version = sys.argv[1:]
    sys.stdout.write(render(Path(template).read_text(), Path(header).read_text(), version))


if __name__ == "__main__":
    main()
