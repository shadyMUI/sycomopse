import re
import unittest
from pathlib import Path


class DesktopBinarySelectionTests(unittest.TestCase):
    def test_tauri_selects_the_desktop_binary_not_either_headless_helper(self):
        manifest = (Path(__file__).resolve().parents[2] / "src-tauri/Cargo.toml").read_text()
        package = re.search(r"(?ms)^\[package\]\n(.*?)(?=^\[)", manifest).group(1)
        self.assertRegex(package, r'(?m)^default-run\s*=\s*"peripheral-hub"\s*$')
        binaries = re.findall(r'(?ms)^\[\[bin\]\]\n(.*?)(?=^\[|\Z)', manifest)
        desktop = next(block for block in binaries if re.search(r'(?m)^name\s*=\s*"peripheral-hub"\s*$', block))
        self.assertRegex(desktop, r'(?m)^required-features\s*=\s*\["desktop"\]\s*$')
        self.assertTrue(any('"peripheral-hub-remapping-agent"' in block for block in binaries))
        self.assertTrue(any('"peripheral-hub-audio-worker"' in block for block in binaries))


if __name__ == "__main__":
    unittest.main()
