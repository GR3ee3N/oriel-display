"""Render release versions into addon.xml and write it to stdout.

    build/stage.py TEMPLATE VERSIONS_H VERSION

Read Kodi API versions from the dev-kit's kodi/versions.h. Replace @NAME@
with the matching header version and @VERSION@ with the add-on release
version. Kodi checks imported API minimums against its supported versions.
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
