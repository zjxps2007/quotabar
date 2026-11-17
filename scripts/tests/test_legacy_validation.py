import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "legacy_validation", Path(__file__).resolve().parents[1] / "prepare_legacy_validation.py")
legacy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(legacy)


def snapshot(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        for name, content, link in entries:
            member = tarfile.TarInfo(name)
            if link is not None:
                member.type = tarfile.SYMTYPE
                member.linkname = link
                archive.addfile(member)
            else:
                member.size = len(content)
                member.mode = 0o644
                archive.addfile(member, io.BytesIO(content))
    return output.getvalue()


class LegacyValidationTests(unittest.TestCase):
    def test_only_current_production_files_are_overlaid(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            names = ["native/lib.rs", "build.rs", "windows/assets/QuotaBar.manifest",
                     "macos/assets/AppIcon-4.icns", "macos/Sources/Old.swift",
                     "windows/src/Old.cs", "windows/tests/Old.cs", "native/deleted.rs"]
            for name in names[:-1]:
                file = root / name
                file.parent.mkdir(parents=True, exist_ok=True)
                file.write_bytes(b"fixture")
            with patch.object(legacy, "ROOT", root), patch.object(
                    legacy, "git", return_value=("\0".join(names) + "\0").encode()):
                self.assertEqual(legacy.production_files(), sorted(names[:4]))

    def test_resource_link_resolves_to_the_preserved_assets(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            legacy.extract_snapshot(snapshot([
                ("shared/assets/icon.png", b"icon bytes", None),
                ("macos/Sources/QuotaBar/Resources", b"", "../../../shared/assets"),
            ]), root)
            self.assertEqual((root / "macos/Sources/QuotaBar/Resources/icon.png").read_bytes(), b"icon bytes")

    def test_archive_path_cannot_write_outside_workspace(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "workspace"
            root.mkdir()
            with self.assertRaises(ValueError):
                legacy.extract_snapshot(snapshot([("../outside.txt", b"must not write", None)]), root)
            self.assertFalse((root.parent / "outside.txt").exists())

    def test_resource_link_cannot_escape_workspace(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder) / "workspace"
            root.mkdir()
            with self.assertRaises(ValueError):
                legacy.extract_snapshot(snapshot([("resource", b"", "../outside")]), root)


if __name__ == "__main__":
    unittest.main()
