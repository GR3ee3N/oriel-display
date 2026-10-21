import unittest

from .util import REPO, load

checker = load("check_library", REPO / "build/check-library.py")


def dynamic(*names):
    return "".join(f" 0x1 (NEEDED) Shared library: [{name}]\n" for name in names)


def needs(*versions):
    return "Version symbols section\n GLIBC_9.99\nVersion needs section '.gnu.version_r'\n" + "\n".join(versions)


def symbols(*names):
    return "".join(f"    {i}: 0000000000001234    40 FUNC    GLOBAL DEFAULT   12 {name}\n" for i, name in enumerate(names))


EXPORTS = symbols("ADDON_Create", "ADDON_GetTypeVersion", "ADDON_GetTypeMinVersion")


class CheckLibraryTests(unittest.TestCase):
    env = REPO / "build/box.env"

    def test_box_libraries_and_versions_pass(self):
        found = checker.problems(dynamic("libEGL.so.1", "libGLESv2.so.2", "libstdc++.so.6", "libc.so.6"),
                                 needs("GLIBC_2.34", "GLIBCXX_3.4.32", "GCC_3.0"), EXPORTS, self.env)
        self.assertEqual(found, [])

    def test_foreign_library_and_new_symbols_fail(self):
        found = checker.problems(dynamic("libfoo.so.1"), needs("GLIBC_9.0", "GLIBCXX_3.4.99"), EXPORTS, self.env)
        self.assertEqual(len(found), 3)
        self.assertIn("libfoo.so.1", found[0])

    def test_missing_entry_points_fail(self):
        found = checker.problems(dynamic(), needs(), symbols("ADDON_Create"), self.env)
        self.assertEqual(found, ["does not export ADDON_GetTypeVersion", "does not export ADDON_GetTypeMinVersion"])


if __name__ == "__main__":
    unittest.main()
