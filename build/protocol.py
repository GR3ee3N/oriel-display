"""Stage a standalone protocol crate with workspace package fields resolved."""
import json
from pathlib import Path
import shutil
import sys
import tomllib

REPO = Path(__file__).resolve().parent.parent


def stage(destination):
    package = tomllib.loads((REPO / "crates/protocol/Cargo.toml").read_text())["package"]
    workspace = tomllib.loads((REPO / "Cargo.toml").read_text())["workspace"]["package"]
    if destination.exists():
        shutil.rmtree(destination)
    shutil.copytree(REPO / "crates/protocol/src", destination / "src")
    shutil.copy(REPO / "LICENSE", destination / "LICENSE")
    fields = {key: workspace[key] if isinstance(value, dict) else value for key, value in package.items()}
    (destination / "Cargo.toml").write_text(
        "[package]\n" + "".join(f"{key} = {json.dumps(value)}\n" for key, value in fields.items()))


if __name__ == "__main__":
    stage(Path(sys.argv[1]))
