import unittest
import xml.etree.ElementTree as ET

from .util import REPO, load

stage = load("stage", REPO / "build/stage.py")

VERSIONS_H = """
#define ADDON_GLOBAL_VERSION_MAIN                     "2.0.4"
#define ADDON_GLOBAL_VERSION_MAIN_MIN                 "2.0.3"
#define ADDON_GLOBAL_VERSION_GUI                      "5.15.1"
#define ADDON_GLOBAL_VERSION_GUI_MIN                  "5.15.0"
#define ADDON_INSTANCE_VERSION_VFS                    "3.0.1"
#define ADDON_INSTANCE_VERSION_VFS_MIN                "3.0.1"
"""


class StageTests(unittest.TestCase):
    def render(self):
        return stage.render((REPO / "addon/addon.xml").read_text(), VERSIONS_H, "1.2.3")

    def test_imports_get_the_devkit_minimums(self):
        addon = ET.fromstring(self.render())
        imports = {i.get("addon"): i.get("version") for i in addon.find("requires")}
        self.assertEqual(imports, {
            "kodi.binary.global.main": "2.0.3",
            "kodi.binary.global.gui": "5.15.0",
            "kodi.binary.instance.vfs": "3.0.1",
        })
        self.assertEqual(addon.get("version"), "1.2.3")
        self.assertNotIn("@", self.render())

    def test_library_and_platform_match_the_build(self):
        addon = ET.fromstring(self.render())
        self.assertEqual(addon.get("id"), "vfs.oriel.display")
        vfs = addon.find('extension[@point="kodi.vfs"]')
        self.assertEqual(vfs.get("library_linux"), "vfs.oriel.display.so")
        self.assertIn("vfs.oriel.display.so", (REPO / "build/compile.sh").read_text())
        self.assertEqual(addon.find('extension[@point="xbmc.addon.metadata"]/platform').text, "linux-aarch64")

    def test_unknown_placeholder_fails(self):
        with self.assertRaises(SystemExit):
            stage.render("@ADDON_GLOBAL_VERSION_NOPE_MIN@", VERSIONS_H, "1.2.3")


if __name__ == "__main__":
    unittest.main()
