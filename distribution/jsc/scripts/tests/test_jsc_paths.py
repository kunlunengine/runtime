"""Portable path and extraction tests; no native engine or compressor required."""

import importlib.util
import io
from pathlib import Path
import stat
import tarfile
import tempfile
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "jsc_artifact_paths", Path(__file__).resolve().parents[1] / "jsc_artifact.py"
)
assert SPEC and SPEC.loader
jsc_artifact = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(jsc_artifact)


class PortablePathTests(unittest.TestCase):
    def test_canonical_names(self):
        for value in (
            "include/kunlun_jsc.h", "lib/JavaScriptCore.dll",
            "licenses/01-WebKit.txt", "metadata/build.json",
            "目录/模块.mjs", "lib/com10.dll",
        ):
            with self.subTest(path=value):
                self.assertEqual(
                    jsc_artifact.checked_relative_path(value, "fixture").as_posix(), value
                )

    def test_host_dependent_names_are_rejected_on_every_host(self):
        # Keep these cases aligned with distribution/jsc/paths.rs.
        for value in (
            "", "/lib/jsc.dll", "C:/lib/jsc.dll", "C:lib/jsc.dll",
            r"lib\jsc.dll", r"\\server\share", "lib/../escape",
            "lib/./jsc.dll", "lib//jsc.dll", "lib/",
            "lib/jsc.dll:payload", "lib/jsc.dll.", "lib/jsc.dll ",
            "lib/NUL.dll", "lib/con", "lib/COM1.dll", "lib/lpt9.txt",
            "lib/COM¹.dll", "lib/CONOUT$", "lib/a?b", "lib/a\0b", "lib/a\nb",
        ):
            with self.subTest(path=value):
                with self.assertRaisesRegex(jsc_artifact.ArtifactError, "safe relative path"):
                    jsc_artifact.checked_relative_path(value, "fixture")

    def extract(self, root, names):
        archive = root / "fixture.tar"
        with tarfile.open(archive, "w", format=tarfile.USTAR_FORMAT) as output:
            for name in names:
                member = tarfile.TarInfo(name)
                member.size = 7
                output.addfile(member, io.BytesIO(b"fixture"))
        destination = root / "extracted"
        destination.mkdir()

        def decompress(command, *, stdout, **kwargs):
            self.assertEqual(command[-1], str(archive))
            stdout.write(archive.read_bytes())

        # Only compression is substituted. The real validator/extractor runs.
        with mock.patch.object(jsc_artifact.subprocess, "run", side_effect=decompress):
            return jsc_artifact.decompress_archive(archive, "fixture-zstd", destination)

    def test_portable_archive_extracts(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.extract(root, ["include/kunlun_jsc.h", "metadata/build.json"])
            self.assertEqual(
                (root / "extracted/root/include/kunlun_jsc.h").read_bytes(), b"fixture"
            )

    def test_unsafe_archives_install_no_files(self):
        for names in (
            [r"include\..\escape"],
            ["include/C:escape"],
            ["include/NUL.h"],
            ["include/header.h:payload"],
            ["include/header.h."],
            ["include//header.h"],
            ["include/Header.h", "include/header.h"],
            ["include/header.h", "include/header.h"],
        ):
            with self.subTest(names=names), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                with self.assertRaisesRegex(jsc_artifact.ArtifactError, "unsafe or duplicate"):
                    self.extract(root, names)
                self.assertEqual(list((root / "extracted/root").rglob("*")), [])

    def test_reparse_attributes_are_rejected_independent_of_link_tag(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            file = root / "file.txt"
            file.write_bytes(b"fixture")
            with mock.patch.object(
                Path, "lstat",
                return_value=mock.Mock(st_file_attributes=0x400, st_mode=stat.S_IFREG | 0o644),
            ):
                self.assertTrue(jsc_artifact.is_reparse_point(file))
                with self.assertRaisesRegex(jsc_artifact.ArtifactError, "reparse points"):
                    list(jsc_artifact.iter_regular_files(root))


if __name__ == "__main__":
    unittest.main()
