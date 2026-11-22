"""Inert ELF/SquashFS fixtures verify rejection without executing test binaries."""
import os
from pathlib import Path
import struct
import tempfile
import unittest
from unittest.mock import patch

import optimize_appimage as image


def fixture(signed=False, digest_bytes=16):
    names = b"\0.shstrtab\0.sha256_sig\0.sig_key\0.digest_md5\0"
    count, table = 5, 64
    strings = table + 64 * count
    signature, key, md5 = strings + len(names), strings + len(names) + 32, strings + len(names) + 64
    offset = md5 + digest_bytes
    data = bytearray(offset + 96)
    data[:6] = b"\x7fELF\x02\x01"
    data[8:11] = b"AI\x02"
    struct.pack_into("<Q", data, 40, table)
    struct.pack_into("<HHH", data, 58, 64, count, 1)
    sections = [(0, 0, 0, 0)]
    for name, start, length in [(b".shstrtab", strings, len(names)), (b".sha256_sig", signature, 32),
                               (b".sig_key", key, 32), (b".digest_md5", md5, digest_bytes)]:
        sections.append((names.index(name), 1, start, length))
    for index, (name, kind, start, length) in enumerate(sections):
        struct.pack_into("<IIQQQQIIQQ", data, table + 64 * index, name, kind, 0, 0, start, length, 0, 0, 1, 0)
    data[strings:strings + len(names)] = names
    if signed:
        data[signature] = 1
    data[offset:offset + 4] = b"hsqs"
    struct.pack_into("<I", data, offset + 12, 131072)
    struct.pack_into("<H", data, offset + 20, 6)
    struct.pack_into("<HH", data, offset + 28, 4, 0)
    struct.pack_into("<Q", data, offset + 40, 96)
    return data, offset


class AppImageTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="nickel-appimage-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "fixture.AppImage"
        self.data, self.offset = fixture()
        self.source.write_bytes(self.data)

    def test_unsigned_layout_extracts_runtime_boundary_and_digest_section(self):
        offset, runtime, md5, format = image.layout(self.source)
        self.assertEqual(offset, self.offset)
        self.assertEqual(runtime, self.data[:offset])
        self.assertEqual(md5[1], 16)
        self.assertEqual(format, {"compressorId": 6, "blockBytes": 131072})

    def test_signatures_sidecar_and_unsupported_digest_are_rejected_without_source_changes(self):
        for data in [fixture(signed=True)[0], fixture(digest_bytes=8)[0]]:
            self.source.write_bytes(data)
            before = image.digest_file(self.source)
            with self.assertRaises(ValueError):
                image.layout(self.source)
            self.assertEqual(image.digest_file(self.source), before)
        self.source.write_bytes(self.data)
        self.source.with_suffix(".AppImage.sig").write_bytes(b"signed-fixture")
        with self.assertRaises(ValueError):
            image.layout(self.source)

    def test_truncation_wrong_format_and_corrupt_section_layout_fail_closed(self):
        cases = [self.data[:length] for length in [0, 32, 63, self.offset, self.offset + 95]]
        for location, content in [(0, b"BAD!"), (8, b"BAD"), (self.offset, b"BAD!"), (self.offset + 28, b"\x03\0\0\0")]:
            data = bytearray(self.data)
            data[location:location + len(content)] = content
            cases.append(data)
        for data in cases:
            self.source.write_bytes(data)
            with self.assertRaises((ValueError, struct.error)):
                image.layout(self.source)

    @unittest.skipUnless(os.name == "posix", "Filesystem modes and xattrs require POSIX")
    def test_inventory_records_content_permissions_links_and_xattrs(self):
        directory = self.root / "tree"
        directory.mkdir()
        binary = directory / "ni-launcher"
        binary.write_bytes(b"inert-fixture")
        binary.chmod(0o755)
        (directory / "alias").symlink_to("ni-launcher")
        expected = image.inventory(directory)
        self.assertEqual(expected["ni-launcher"]["sha256"], image.digest_file(binary).hex())
        self.assertTrue(expected["ni-launcher"]["mode"] & 0o111)
        self.assertEqual(expected["alias"]["link"], "ni-launcher")
        self.assertIn("xattrs", expected["ni-launcher"])
        binary.write_bytes(b"changed-fixture")
        self.assertNotEqual(image.inventory(directory), expected)

    @unittest.skipUnless(os.name == "posix", "Optimizer is Linux-only")
    def test_occupied_outputs_and_extraction_failure_preserve_source(self):
        output, evidence = self.root / "output.AppImage", self.root / "evidence.json"
        before = image.digest_file(self.source)
        with patch.object(image.shutil, "which", return_value="fixture-tool"):
            output.write_bytes(b"keep-fixture")
            with self.assertRaises(ValueError):
                image.optimize(self.source, output, evidence)
            self.assertEqual(output.read_bytes(), b"keep-fixture")
            output.unlink()
            with patch.object(image, "extract", side_effect=ValueError("fixture-extraction-failed")), self.assertRaises(ValueError):
                image.optimize(self.source, output, evidence)
        self.assertFalse(output.exists())
        self.assertFalse(evidence.exists())
        self.assertEqual(image.digest_file(self.source), before)


    @unittest.skipUnless(os.name == "posix", "Runtime compatibility selection is Linux-only")
    def test_runtime_extraction_validates_content_and_executable_bits_without_launching_an_app(self):
        original = self.root / "original"
        binary = original / "usr/bin/ni-launcher"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"inert-runtime-fixture")
        binary.chmod(0o755)
        expected = image.inventory(original)
        with patch.object(image.subprocess, "run") as execute:
            execute.return_value.returncode = 1
            self.assertFalse(image.runtime_extract_verified(self.source, self.root / "unsupported", expected))
            self.assertEqual(execute.call_args.args[0][-1], "--appimage-extract")
            execute.return_value.returncode = 0
            self.assertFalse(image.runtime_extract_verified(self.source, self.root / "absent", expected))

            def decode(_command, **options):
                import shutil
                shutil.copytree(original, options["cwd"] / "squashfs-root")
                return __import__("subprocess").CompletedProcess(_command, 0)
            execute.side_effect = decode
            self.assertTrue(image.runtime_extract_verified(self.source, self.root / "valid", expected))
            changed = image.inventory(original)
            changed["usr/bin/ni-launcher"]["sha256"] = "0" * 64
            with self.assertRaises(ValueError):
                image.runtime_extract_verified(self.source, self.root / "changed", changed)
            binary.chmod(0o644)
            with self.assertRaises(ValueError):
                image.runtime_extract_verified(self.source, self.root / "permissions", expected)

    @unittest.skipUnless(os.name == "posix", "Optimizer is Linux-only")
    def test_smaller_but_runtime_incompatible_compression_is_never_selected(self):
        self.source.write_bytes(self.data + b"padding" * 512)
        expected = {"usr/bin/ni-launcher": {"kind": "file", "mode": 0o755, "bytes": 32, "sha256": "a" * 64}}
        output, evidence = self.root / "output.AppImage", self.root / "report.json"
        original_layout = image.layout(self.source)
        sizes = {"zstd-22-1MiB": 256, "xz-1MiB-x86": 128}

        def tools(command):
            if command[0] == "mksquashfs":
                destination = Path(command[2])
                destination.write_bytes(b"x" * sizes[destination.stem])
                return ""
            return str(self.offset)

        def extraction(_source, _offset, destination):
            destination.mkdir()

        with patch.object(image.shutil, "which", return_value="fixture-tool"), \
                patch.object(image, "layout", side_effect=lambda _path: (original_layout[0], bytearray(original_layout[1]), original_layout[2], original_layout[3])), \
                patch.object(image, "extract", side_effect=extraction), \
                patch.object(image, "inventory", return_value=expected), \
                patch.object(image, "run", side_effect=tools), \
                patch.object(image, "runtime_extract_verified", side_effect=lambda source, *_args: not source.name.startswith("xz-")):
            report = image.optimize(self.source, output, evidence)
        self.assertEqual(output.stat().st_size, self.offset + sizes["zstd-22-1MiB"])
        self.assertTrue(report["runtimeExtractionVerified"])
        self.assertFalse(report["attempts"][1]["runtimeExtractionVerified"])
        self.assertLess(report["attempts"][1]["bytes"], report["afterBytes"])

    @unittest.skipUnless(os.name == "posix", "Optimizer is Linux-only")
    def test_incompatible_original_runtime_blocks_output_before_compression(self):
        expected = {"usr/bin/ni-launcher": {"kind": "file", "mode": 0o755}}
        output, evidence = self.root / "output.AppImage", self.root / "report.json"
        with patch.object(image.shutil, "which", return_value="fixture-tool"), \
                patch.object(image, "extract"), patch.object(image, "inventory", return_value=expected), \
                patch.object(image, "runtime_extract_verified", return_value=False), \
                patch.object(image, "run") as tools, self.assertRaises(ValueError):
            image.optimize(self.source, output, evidence)
        tools.assert_not_called()
        self.assertFalse(output.exists())
        self.assertFalse(evidence.exists())



if __name__ == "__main__":
    unittest.main()
