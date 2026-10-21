from pathlib import Path
import tempfile
import tomllib
import unittest

from .util import REPO, load

protocol = load("protocol_package", REPO / "build/protocol.py")


class ProtocolPackageTests(unittest.TestCase):
    def test_crate_is_independent_of_the_workspace(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "protocol"
            protocol.stage(target)
            package = tomllib.loads((target / "Cargo.toml").read_text())["package"]
            self.assertEqual(package["name"], "oriel-display-protocol")
            self.assertTrue(all(not isinstance(value, dict) for value in package.values()))
            self.assertEqual((target / "src/lib.rs").read_bytes(), (REPO / "crates/protocol/src/lib.rs").read_bytes())
            self.assertEqual((target / "LICENSE").read_bytes(), (REPO / "LICENSE").read_bytes())

