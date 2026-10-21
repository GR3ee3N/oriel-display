"""Packages the add-on with its licenses and sources, and the protocol crate.

    build/package.py COMMIT SOURCE_TAR
"""
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import xml.etree.ElementTree as ET
import zipfile

REPO = Path(__file__).resolve().parent.parent
GITHUB = "GR3ee3N/oriel-display"
ID = "vfs.oriel.display"
LICENSE_FILE = re.compile(r"^(?:licen[cs]e|copying|copyright|notice|unlicense)(?:[-._].*)?$", re.I)


def require(condition, message):
    if not condition:
        raise SystemExit(message)


def digest(file):
    with Path(file).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def checksum(file):
    file.with_name(file.name + ".sha256").write_text(f"{digest(file)}  {file.name}\n")


def zip_tree(directory, destination):
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        for file in sorted(directory.rglob("*")):
            require(not file.is_symlink(), f"ZIP must not contain Unix links: {file}")
            archive.write(file, f"{directory.name}/{file.relative_to(directory).as_posix()}")


def package(runtime, licenses, source, destination, version, devkit_ref):
    url = f"https://github.com/{GITHUB}/releases/download/v{version}/{source.name}.tar.xz"
    files = destination / "files"
    files.mkdir(parents=True)
    addon = destination / "packages" / ID
    shutil.copytree(runtime / ID, addon)
    require((addon / f"{ID}.so").is_file(), "run build/compile.sh first")
    shutil.copy(REPO / "LICENSE", addon / "LICENSE.txt")
    shutil.copytree(licenses, addon / "LICENSES")
    (addon / "THIRD-PARTY-NOTICES.txt").write_text(
        "Third-party components keep their own licenses; see LICENSES/. Oriel's\n"
        "own code is GPL-3.0-or-later (LICENSE.txt).\n"
        "\n"
        "The library links Rust's standard library and the Cargo crates in\n"
        "LICENSES/cargo, and its bindings come from Kodi's add-on dev-kit headers\n"
        f"(Team Kodi, GPL-2.0-or-later) in CoreELEC's Kodi at {devkit_ref}.\n"
        f"Complete source and build scripts: {url}\n"
    )
    (addon / "SOURCE.txt").write_text(url + "\n")
    zip_name = f"{ID}-{version}.zip"
    zip_tree(addon, files / zip_name)
    # The framework's shell builds against this.
    with tarfile.open(files / f"oriel-display-{version}-protocol.tar.xz", "w:xz") as archive:
        archive.add(runtime / "protocol", arcname="protocol")
    shutil.copytree(licenses, source / "licenses")
    with tarfile.open(files / f"{source.name}.tar.xz", "w:xz") as archive:
        archive.add(source, arcname=source.name)
    # Kodi's index is served from the latest release alongside the zip it names.
    addons = ET.Element("addons")
    element = ET.fromstring((addon / "addon.xml").read_bytes())
    ET.SubElement(element.find('extension[@point="xbmc.addon.metadata"]'), "path").text = zip_name
    addons.append(element)
    ET.indent(addons)
    ET.ElementTree(addons).write(files / "addons.xml", encoding="UTF-8", xml_declaration=True)
    for file in sorted(files.iterdir()):
        if not file.name.endswith(".sha256"):
            checksum(file)


def main():
    commit, snapshot = sys.argv[1:]
    version, devkit_ref = os.environ["VERSION"], os.environ["KODI_DEVKIT_REF"]
    destination = REPO / ".cache/release.next"
    if destination.exists():
        shutil.rmtree(destination)
    source = destination / f"oriel-display-{version}-sources"
    source.mkdir(parents=True)
    with tarfile.open(snapshot) as archive:
        archive.extractall(source, filter="data")
    project = source / "oriel-display"
    vendor = project / "vendor"
    cargo_config = subprocess.check_output(["cargo", "vendor", "--locked", "--versioned-dirs", str(vendor)], text=True, cwd=project)
    (project / ".cargo").mkdir(exist_ok=True)
    (project / ".cargo/config.toml").write_text(cargo_config.replace(str(vendor), "vendor"))
    (source / "COMMIT").write_text(commit + "\n")
    (source / "README.txt").write_text(
        "Build instructions: oriel-display/README.md. Cargo dependencies are\n"
        "vendored; .cargo/config.toml selects them. The build downloads Kodi's\n"
        f"add-on dev-kit headers from CoreELEC's Kodi at {devkit_ref}, the Rust\n"
        "toolchain and its build tools.\n"
    )

    licenses = destination / "licenses"
    licenses.mkdir()
    shutil.copy(REPO / ".cache/devkit" / devkit_ref / "LICENSES/GPL-2.0-or-later", licenses / "kodi-dev-kit-GPL-2.0-or-later.txt")
    # The standard library is linked into the add-on, with its own third-party code.
    rust = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True, cwd=REPO).strip())
    shutil.copytree(rust / "share/doc/rust", licenses / "rust", ignore=shutil.ignore_patterns("README.md"))
    for crate in sorted(vendor.iterdir()):
        found = [file for file in crate.iterdir() if file.is_file() and LICENSE_FILE.match(file.name)]
        require(found, f"no license file in Cargo dependency {crate.name}")
        (licenses / "cargo" / crate.name).mkdir(parents=True)
        for file in found:
            shutil.copy(file, licenses / "cargo" / crate.name / file.name)

    package(REPO / ".cache/runtime", licenses, source, destination, version, devkit_ref)
    # tools/release.sh publishes only packages of the commit it releases.
    (destination / "COMMIT").write_text(commit + "\n")
    for leftover in (destination / "packages", source, licenses):
        shutil.rmtree(leftover)
    release = REPO / ".cache/release"
    if release.exists():
        shutil.rmtree(release)
    destination.rename(release)
    print(f"Release packages, protocol crate and source: {release / 'files'}")


if __name__ == "__main__":
    main()
