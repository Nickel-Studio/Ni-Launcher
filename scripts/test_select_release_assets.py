"""Public Release selection tests: evidence stays in Actions, only verified binaries ship."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import select_release_assets
from package_portable import TARGETS

VERSION = "1.0.2"
INSTALLERS = [f"Nickel_{VERSION}_x64_en-US.msi", f"Nickel_{VERSION}_x64-setup.exe", f"Nickel_{VERSION}_aarch64.dmg",
              f"Nickel_{VERSION}_x64.dmg", f"Nickel_{VERSION}_amd64.deb", f"Nickel_{VERSION}_amd64.AppImage"]
PORTABLES = [f"Nickel_{VERSION}_{label}_portable{suffix}" for label, _, suffix in TARGETS.values()]
EVIDENCE = [f"{label}-{name}" for label, _, _ in TARGETS.values()
            for name in ("native-config-audit.json", "remote-config-audit.json", "release-manifest.json")]


class SelectReleaseAssetsTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / "package.json").write_text(json.dumps({"version": VERSION}), encoding="utf-8")
        self.assets = self.root / "release-assets"
        self.assets.mkdir()
        for name in INSTALLERS + PORTABLES + EVIDENCE:
            (self.assets / name).write_bytes(name.encode())
        self.write_sums()
        self.source = self.root / f"Nickel-{VERSION}-source.zip"
        self.source.write_bytes(b"source")
        self.output = self.root / "release-public"

    def write_sums(self):
        files = sorted(path for path in self.assets.iterdir() if path.name != "SHA256SUMS.txt")
        lines = "".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in files)
        (self.assets / "SHA256SUMS.txt").write_text(lines, encoding="utf-8")

    def select(self):
        return select_release_assets.select(self.assets, self.source, self.output, self.root)

    def test_only_installers_portables_and_source_are_published_with_matching_sums(self):
        result = self.select()
        expected = set(INSTALLERS + PORTABLES + [self.source.name])
        self.assertEqual({path.name for path in self.output.iterdir()}, expected | {"SHA256SUMS.txt"})
        self.assertEqual(result["evidenceExcluded"], len(EVIDENCE))
        sums = {line.split("  ")[1]: line.split("  ")[0]
                for line in (self.output / "SHA256SUMS.txt").read_text(encoding="utf-8").splitlines()}
        self.assertEqual(set(sums), expected)
        for name, digest in sums.items():
            self.assertEqual(hashlib.sha256((self.output / name).read_bytes()).hexdigest(), digest)

    def test_unclassified_file_blocks_publication(self):
        (self.assets / "notes.txt").write_text("unexpected", encoding="utf-8")
        self.write_sums()
        with self.assertRaisesRegex(ValueError, "Unclassified"):
            self.select()
        self.assertFalse(self.output.exists())

    def test_file_changed_or_added_after_collection_is_rejected(self):
        (self.assets / PORTABLES[0]).write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "changed after collection"):
            self.select()
        (self.assets / PORTABLES[0]).write_bytes(PORTABLES[0].encode())
        (self.assets / "late.json").write_text("{}", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "do not match"):
            self.select()
        self.assertFalse(self.output.exists())

    def test_missing_portable_or_misnamed_source_blocks_publication(self):
        (self.assets / PORTABLES[-1]).unlink()
        self.write_sums()
        with self.assertRaisesRegex(ValueError, "Portable archive set incomplete"):
            self.select()
        (self.assets / PORTABLES[-1]).write_bytes(PORTABLES[-1].encode())
        self.write_sums()
        self.source = self.source.rename(self.root / "Nickel-1.0.1-source.zip")
        with self.assertRaisesRegex(ValueError, "Source archive"):
            self.select()
        self.assertFalse(self.output.exists())

    def test_existing_output_is_never_overwritten(self):
        self.output.mkdir()
        with self.assertRaisesRegex(ValueError, "overwrite"):
            self.select()


if __name__ == "__main__":
    unittest.main()