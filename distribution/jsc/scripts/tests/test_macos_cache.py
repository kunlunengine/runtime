"""Portable regression tests for the Xcode cache/independent-rebuild boundary."""

from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / "macos-cache-settings.sh"


class MacOSCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="kunlun-macos-cache-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()

    def settings(self, *arguments):
        result = subprocess.run(
            ["bash", str(SCRIPT), str(self.root), *map(str, arguments)],
            check=True,
            capture_output=True,
            text=True,
        )
        return dict(line.split("=", 1) for line in result.stdout.splitlines())

    def test_independent_builds_always_get_distinct_empty_caches(self):
        first = Path(self.settings()["COMPILATION_CACHE_CAS_PATH"])
        (first / "sentinel").touch()
        second = Path(self.settings()["COMPILATION_CACHE_CAS_PATH"])
        self.assertNotEqual(first, second)
        self.assertEqual(second.parent, self.root)
        self.assertEqual(list(second.iterdir()), [])

    def test_shared_cache_is_reused_without_deleting_entries(self):
        shared = self.root / "persistent"
        first = self.settings(shared)
        (shared / "sentinel").touch()
        second = self.settings(shared)
        self.assertEqual(first, second)
        self.assertEqual(Path(second["COMPILATION_CACHE_CAS_PATH"]), shared)
        self.assertTrue((shared / "sentinel").exists())

    def test_settings_select_bounded_local_native_cache(self):
        settings = self.settings()
        self.assertEqual(settings["WK_USE_CCACHE"], "NO")
        self.assertEqual(settings["COMPILATION_CACHE_ENABLE_CACHING"], "YES")
        self.assertEqual(settings["COMPILATION_CACHE_ENABLE_DIAGNOSTIC_REMARKS"], "YES")
        self.assertEqual(settings["COMPILATION_CACHE_KEEP_CAS_DIRECTORY"], "YES")
        self.assertEqual(settings["COMPILATION_CACHE_LIMIT_SIZE"], "2G")
        self.assertEqual(settings["COMPILATION_CACHE_ENABLE_PLUGIN"], "NO")
        self.assertEqual(settings["COMPILATION_CACHE_REMOTE_SERVICE_PATH"], "")

    def test_path_metacharacters_fail_before_emitting_build_settings(self):
        for name in ["with space", "quote'", "dollar$", "line\nbreak"]:
            with self.subTest(name=name):
                result = subprocess.run(
                    ["bash", str(SCRIPT), str(self.root), str(self.root / name)],
                    capture_output=True,
                    text=True,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("whitespace or shell metacharacters", result.stderr)
                self.assertEqual(result.stdout, "")

    def test_missing_output_directory_is_rejected(self):
        result = subprocess.run(
            ["bash", str(SCRIPT), str(self.root / "absent")],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()
