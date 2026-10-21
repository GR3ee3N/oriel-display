import re
import unittest

from .util import REPO

SIZES = {"u32": 4, "i32": 4, "u64": 8}


def crate():
    return (REPO / "crates/protocol/src/lib.rs").read_text()


def types_in_crate():
    kinds = re.search(r"pub mod kind \{(.*?)\n\}", crate(), re.S).group(1)
    return {name.lower(): int(value) for name, value in re.findall(r"pub const ([A-Z]+): u32 = (\d+);", kinds)}


def fields_in_crate():
    body = re.search(r"pub struct Msg \{(.*?)\n\}", crate(), re.S).group(1)
    fields = []
    for name, array_kind, count, kind in re.findall(r"pub (\w+): (?:\[(\w+); (\d+)\]|(\w+))", body):
        fields.append((name, SIZES[array_kind or kind] * int(count or 1)))
    return fields


def spec():
    return (REPO / "docs/protocol.md").read_text()


class ProtocolTests(unittest.TestCase):
    def test_message_is_64_bytes_without_padding(self):
        offset = 0
        for name, size in fields_in_crate():
            self.assertEqual(offset % min(size, 8), 0, f"{name} would be padded")
            offset += size
        self.assertEqual(offset, 64)

    def test_spec_lists_the_crate_layout(self):
        rows = re.findall(r"^\| (\d+) \| `(\w+)` \| [ui]\d+", spec(), re.M)
        offset = 0
        expected = []
        for name, size in fields_in_crate():
            expected.append((str(offset), name))
            offset += size
        self.assertEqual(rows, expected)

    def test_spec_lists_the_crate_types(self):
        rows = {name: int(value) for value, name in re.findall(r"^\| (\d+) \| `(\w+)` \| (?:producer|add-on) \|", spec(), re.M)}
        self.assertEqual(rows, types_in_crate())

    def test_socket_names_and_versions_match_the_spec(self):
        for constant in ("SOCKET", "CONTROL_SOCKET"):
            name = re.search(rf'pub const {constant}: &\[u8\] = b"([^"]+)";', crate()).group(1)
            self.assertIn(f"`{name}`", spec())
        for constant, label in (("VERSION", "Producer protocol version"), ("CONTROL_VERSION", "Control protocol version")):
            version = re.search(rf"pub const {constant}: u32 = (\d+);", crate()).group(1)
            self.assertIn(f"{label}: {version}", spec())


if __name__ == "__main__":
    unittest.main()
