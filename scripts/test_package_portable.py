"""Portable archive contract tests; synthetic binaries are not native acceptance."""
import io
import json
import os
from pathlib import Path
import stat
import struct
import tarfile
import tempfile
import unittest
import zipfile

from package_portable import (LICENSE_FILES, MARKER, TARGETS, archive_entries, check_binary,
                              marker_for, package, read_archive_file, validate_archive)


class PortableTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="nickel-portable-test-")
        self.root = Path(self.temporary.name)
        (self.root / "package.json").write_text(json.dumps({"version": "1.0.2", "repository": {"url": "https://github.com/Nickel-Studio/Ni-Launcher.git"}}), encoding="utf-8")
        for name in (*LICENSE_FILES, "LICENSES/dependencies-NOTICES.txt", "LICENSES/Apache-2.0.txt"):
            source = self.root / name
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_text(f"license fixture: {name}\n", encoding="utf-8")

    def tearDown(self):
        self.temporary.cleanup()

    def binary(self, target):
        data = bytearray(256)
        if "windows" in target:
            source = self.root / "ni-launcher.exe"
            data[:2] = b"MZ"
            struct.pack_into("<I", data, 60, 128)
            data[128:132] = b"PE\0\0"
            struct.pack_into("<H", data, 132, 0x8664)
        elif "linux" in target:
            source = self.root / "source.AppImage"
            data[:6] = b"\x7fELF\x02\x01"
            data[8:11] = b"AI\x02"
            struct.pack_into("<H", data, 18, 62)
        else:
            source = self.root / TARGETS[target][0] / "Nickel.app/Contents/MacOS/ni-launcher"
            source.parent.mkdir(parents=True)
            (source.parent.parent / "Info.plist").write_text("plist fixture", encoding="utf-8")
            data[:4] = b"\xcf\xfa\xed\xfe"
            struct.pack_into("<I", data, 4, 0x0100000C if "aarch64" in target else 0x01000007)
        source.write_bytes(data)
        source.chmod(0o755)
        return source.parent.parent.parent if "apple" in target else source

    def test_all_four_targets_preserve_contents_markers_notices_and_permissions(self):
        for target in TARGETS:
            if os.name == "nt" and target != "x86_64-pc-windows-msvc":
                continue  # Windows does not represent POSIX executable modes; Linux CI tests these targets.
            with self.subTest(target=target):
                source = self.binary(target)
                report = package(target, source, self.root / "outputs", self.root)
                archive = self.root / "outputs" / report["archive"]
                self.assertTrue(validate_archive(archive, self.root)["portableArchiveVerified"])
                self.assertTrue(report["runtimeDataEmpty"])
                self.assertTrue(report["modpackBootstrapAbsent"])
                self.assertIn(MARKER, report["entries"])
                self.assertNotIn("Nickel/settings.json", report["entries"])
                prefix, _entries = archive_entries(archive)
                usage = read_archive_file(archive, f"{prefix}/README-portable.txt").decode("utf-8")
                self.assertIn("No user accounts, credential key files, secrets.local.json", usage)
                self.assertIn("may embed authorized public Microsoft OAuth configuration", usage)
                self.assertIn("a CurseForge API key", usage)
                self.assertIn("not client-side secrecy", usage)
                self.assertNotIn("No accounts, keys", usage)
                with self.assertRaises(ValueError):
                    package(target, source, self.root / "outputs", self.root)

    @unittest.skipUnless(os.name == "posix", "macOS symlink layout requires POSIX")
    def test_macos_bundle_internal_links_are_preserved_and_external_links_rejected(self):
        target = "aarch64-apple-darwin"
        source = self.binary(target)
        resources = source / "Contents/Resources"
        resources.mkdir()
        (resources / "Current").write_bytes(b"fixture")
        (resources / "Alias").symlink_to("Current")
        report = package(target, source, self.root / "outputs", self.root)
        self.assertEqual(report["entries"]["Nickel.app/Contents/Resources/Alias"]["kind"], "symlink")
        (resources / "Bad").symlink_to(self.root / "package.json")
        with self.assertRaises(ValueError):
            package(target, source, self.root / "bad-outputs", self.root)

    def test_wrong_architecture_and_non_appimage_inputs_are_rejected(self):
        source = self.binary("x86_64-unknown-linux-gnu")
        with self.assertRaises(ValueError):
            check_binary(source, "x86_64-pc-windows-msvc")
        source.write_bytes(b"not an appimage" * 10)
        with self.assertRaises(ValueError):
            package("x86_64-unknown-linux-gnu", source, self.root / "outputs", self.root)
        marker = marker_for("x86_64-pc-windows-msvc", "1.0.2")
        self.assertNotIn("payload", marker)
        with self.assertRaises(ValueError):
            marker_for("unknown", "1.0.2")
        with self.assertRaises(ValueError):
            marker_for("x86_64-pc-windows-msvc", "../private")

    def test_archive_traversal_duplicate_paths_and_private_runtime_files_are_rejected(self):
        for names in [["../private"], ["root/Nickel/accounts.json"], ["root/image", "root/image"], ["/absolute"]]:
            destination = self.root / "bad.tar.gz"
            with tarfile.open(destination, "w:gz") as stream:
                for name in names:
                    entry = tarfile.TarInfo(name)
                    entry.size = 7
                    stream.addfile(entry, io.BytesIO(b"fixture"))
            with self.assertRaises(ValueError):
                archive_entries(destination)
        destination = self.root / "linked.zip"
        with zipfile.ZipFile(destination, "w") as stream:
            entry = zipfile.ZipInfo("root/link")
            entry.create_system = 3
            entry.external_attr = (stat.S_IFLNK | 0o777) << 16
            stream.writestr(entry, "../../private")
        with self.assertRaises(ValueError):
            archive_entries(destination)

    def test_missing_license_and_changed_license_are_rejected(self):
        target = "x86_64-pc-windows-msvc"
        source = self.binary(target)
        report = package(target, source, self.root / "outputs", self.root)
        (self.root / "LICENSE").write_text("changed", encoding="utf-8")
        with self.assertRaises(ValueError):
            validate_archive(self.root / "outputs" / report["archive"], self.root)
        (self.root / "LICENSE").unlink()
        with self.assertRaises(ValueError):
            package(target, source, self.root / "bad-outputs", self.root)


if __name__ == "__main__":
    unittest.main()
