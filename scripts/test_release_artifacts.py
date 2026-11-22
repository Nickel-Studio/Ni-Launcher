"""Release gate tests with synthetic binaries and mocked official/native checks."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import unittest
from unittest.mock import patch

import collect_release
import optimize_appimage
import prepare_release
import test_package_portable as portable_tests
from package_portable import TARGETS, sha256

REMOTE = {"releaseConfigurationComplete": True, "curseforgeRemoteCredentialAccepted": True,
          "microsoftDeviceCodeClientAccepted": True}
NATIVE_CHECKS = ["actualNativeExecutableScanned", "releaseConfigurationComplete",
                 "embeddedEnvelopeMatchesAuthorizedWhitelist", "curseforgeLiteralAbsent", "microsoftClientLiteralAbsent",
                 "legacyPlaintextBuildOutputAbsent", "releaseReadyForPublication"]
INNER = b"inert-native-appimage-launcher-fixture" * 16


class ReleaseArtifactTests(unittest.TestCase):
    def setUp(self):
        self.fixture = portable_tests.PortableTests()
        self.fixture.setUp()
        self.addCleanup(self.fixture.tearDown)
        self.root = self.fixture.root
        self.inputs = self.root / "inputs"
        self.output = self.root / "assets"
        self.commands = []
        self.remote_ok = self.native_ok = True

    def sources(self, target):
        release = self.root / "builds" / TARGETS[target][0] / "release"
        bundle = release / "bundle"
        release.mkdir(parents=True)
        source = self.fixture.binary(target)
        if "windows" in target:
            shutil.copy2(source, release / "ni-launcher.exe")
            for name in ["msi/Nickel_windows.msi", "nsis/Nickel_windows-setup.exe"]:
                value = bundle / name
                value.parent.mkdir(parents=True, exist_ok=True)
                value.write_bytes(b"inert-installer-fixture")
        elif "apple" in target:
            destination = bundle / "macos/Nickel.app"
            destination.parent.mkdir(parents=True)
            shutil.copytree(source, destination, symlinks=True)
            installer = bundle / "dmg" / f"Nickel_{TARGETS[target][0]}.dmg"
            installer.parent.mkdir()
            installer.write_bytes(b"inert-dmg-fixture")
        else:
            for name in ["appimage/Nickel_linux.AppImage", "deb/Nickel_linux.deb"]:
                value = bundle / name
                value.parent.mkdir(parents=True, exist_ok=True)
                value.write_bytes(source.read_bytes() if name.endswith(".AppImage") else b"inert-deb-fixture")
                value.chmod(0o755)
        return release

    def request(self, command, **options):
        self.commands.append(command)
        self.assertEqual(command[0], "node")
        self.assertNotIn("--allow-incomplete-local", command)
        self.assertEqual(options["cwd"], self.root)
        if command[1].endswith("release-secrets.mjs"):
            self.assertEqual(command[-1], "--remote")
            return subprocess.CompletedProcess(command, 0 if self.remote_ok else 1, json.dumps(REMOTE).encode(), b"private-error-fixture")
        self.assertTrue(command[1].endswith("audit-release.mjs"))
        binary = Path(command[command.index("--binary") + 1])
        evidence = Path(command[command.index("--evidence") + 1])
        checks = {name: True for name in NATIVE_CHECKS}
        checks.update(sha256=sha256(binary), bytes=binary.stat().st_size, remoteAuthenticationVerified=False,
                      localAttackerCannotRecoverCredentials=False)
        if self.native_ok:
            evidence.write_text(json.dumps(checks), encoding="utf-8")
        return subprocess.CompletedProcess(command, 0 if self.native_ok else 1, b"", b"private-error-fixture")

    def compress(self, source, output, evidence):
        shutil.copy2(source, output)
        report = {"appimageCompressionVerified": True, "noFilesRemoved": True, "runtimeCodeUnchanged": True,
                  "runtimeExtractionVerified": True, "allFileContentsPermissionsLinksAndXattrsMatch": True,
                  "beforeBytes": source.stat().st_size, "afterBytes": output.stat().st_size, "sha256": sha256(output),
                  "launcherSha256": __import__("hashlib").sha256(INNER).hexdigest(), "launcherBytes": len(INNER)}
        evidence.write_text(json.dumps(report), encoding="utf-8")
        return report

    def extract(self, _source, _offset, output):
        binary = output / "usr/bin/ni-launcher"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(INNER)

    def prepare(self, target):
        release = self.sources(target)
        output = self.inputs / TARGETS[target][0]
        with patch.object(prepare_release.subprocess, "run", side_effect=self.request), \
                patch.object(optimize_appimage, "optimize", side_effect=self.compress), \
                patch.object(optimize_appimage, "layout", return_value=(0, b"", None, {})), \
                patch.object(optimize_appimage, "extract", side_effect=self.extract):
            result = prepare_release.prepare(target, release, output, self.root)
        return output, result

    def prepare_all(self):
        return {target: self.prepare(target)[0] for target in TARGETS}

    def refresh(self, directory):
        path = directory / "release-manifest.json"
        manifest = json.loads(path.read_text(encoding="utf-8"))
        for entry in manifest["files"]:
            candidate = directory / entry["name"]
            entry.update(bytes=candidate.stat().st_size, sha256=sha256(candidate))
        path.write_text(json.dumps(manifest), encoding="utf-8")
        (directory / "SHA256SUMS.txt").write_text("".join(f"{entry['sha256']}  {entry['name']}\n" for entry in manifest["files"]), encoding="utf-8")

    def test_prepare_windows_requires_installers_portable_native_audit_and_official_preflight(self):
        directory, result = self.prepare("x86_64-pc-windows-msvc")
        self.assertTrue(result["releaseUploadSetVerified"])
        self.assertTrue((directory / result["portable"]).is_file())
        self.assertEqual(len(result["installers"]), 2)
        self.assertEqual(len(self.commands), 2)
        remote = json.loads((directory / "remote-config-audit.json").read_text())
        self.assertEqual(remote, {**REMOTE, "completedAccountLoginVerified": False})
        self.assertFalse(any(value.name == "secrets.local.json" for value in directory.rglob("*")))
        with self.assertRaises(ValueError), patch.object(prepare_release.subprocess, "run", side_effect=AssertionError("No request on occupied output")):
            prepare_release.prepare("x86_64-pc-windows-msvc", self.root / "release", directory, self.root)

    def test_failed_remote_or_native_gate_never_exposes_an_upload_directory(self):
        for phase in ["remote", "native"]:
            release = self.sources("x86_64-pc-windows-msvc")
            output = self.root / f"failed-{phase}"
            self.remote_ok, self.native_ok = phase != "remote", phase != "native"
            with patch.object(prepare_release.subprocess, "run", side_effect=self.request), self.assertRaises(ValueError) as error:
                prepare_release.prepare("x86_64-pc-windows-msvc", release, output, self.root)
            self.assertNotIn("private-error-fixture", str(error.exception))
            self.assertFalse(output.exists())
            self.assertTrue((release / "ni-launcher.exe").is_file())
            shutil.rmtree(release.parent)

    @unittest.skipUnless(os.name == "posix", "All Unix portable formats require POSIX modes")
    def test_all_four_target_sets_collect_only_verified_installers_portable_and_evidence(self):
        directories = self.prepare_all()
        result = collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
        self.assertTrue(result["allFourReleaseTargetsVerified"])
        self.assertTrue(result["portableForEveryTarget"])
        self.assertEqual(len([value for value in self.output.iterdir() if "_portable" in value.name and value.name.endswith((".zip", ".tar.gz"))]), 4)
        for directory in directories.values():
            native = json.loads((directory / "native-config-audit.json").read_text())
            self.assertTrue(native["releaseReadyForPublication"])
        self.assertTrue((self.output / "SHA256SUMS.txt").is_file())
        self.assertFalse((self.output / "build").exists())

    @unittest.skipUnless(os.name == "posix", "All-target gate fixture requires POSIX archives")
    def test_missing_target_portable_or_unexpected_files_blocks_collection_before_output(self):
        directories = self.prepare_all()
        windows = directories["x86_64-pc-windows-msvc"]
        manifest_path = windows / "release-manifest.json"
        manifest = json.loads(manifest_path.read_text())
        archive = windows / manifest["portable"]
        saved = archive.read_bytes()
        archive.unlink()
        with self.assertRaises(ValueError):
            collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
        self.assertFalse(self.output.exists())
        archive.write_bytes(saved)
        (windows / "unexpected.txt").write_text("fixture")
        with self.assertRaises(ValueError):
            collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
        self.assertFalse(self.output.exists())
        (windows / "unexpected.txt").unlink()
        manifest_path.rename(windows / "manifest-hidden.json")
        with self.assertRaises(ValueError):
            collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
        self.assertFalse(self.output.exists())

    @unittest.skipUnless(os.name == "posix", "All-target gate fixture requires POSIX archives")
    def test_changed_hash_duplicate_names_and_path_traversal_never_pass(self):
        directories = self.prepare_all()
        directory = directories["x86_64-pc-windows-msvc"]
        manifest_path = directory / "release-manifest.json"
        original = json.loads(manifest_path.read_text())
        for transform in [lambda value: value["files"].append(value["files"][0].copy()),
                          lambda value: value["files"][0].update(name="../private"),
                          lambda value: value["files"][0].update(sha256="0" * 64)]:
            value = json.loads(json.dumps(original))
            transform(value)
            manifest_path.write_text(json.dumps(value))
            with self.assertRaises(ValueError):
                collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
            self.assertFalse(self.output.exists())

    @unittest.skipUnless(os.name == "posix", "All-target gate fixture requires POSIX archives")
    def test_status_and_launcher_provenance_are_checked_even_when_evidence_hashes_are_updated(self):
        directories = self.prepare_all()
        directory = directories["x86_64-pc-windows-msvc"]
        evidence = directory / "native-config-audit.json"
        original = json.loads(evidence.read_text())
        for change in [{"releaseReadyForPublication": False}, {"sha256": "0" * 64}]:
            evidence.write_text(json.dumps({**original, **change}))
            self.refresh(directory)
            with self.assertRaises(ValueError):
                collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
            self.assertFalse(self.output.exists())
        evidence.write_text(json.dumps(original))
        self.refresh(directory)
        linux = directories["x86_64-unknown-linux-gnu"]
        compression = linux / "appimage-compression.json"
        value = json.loads(compression.read_text())
        value["launcherSha256"] = "0" * 64
        compression.write_text(json.dumps(value))
        self.refresh(linux)
        with self.assertRaises(ValueError):
            collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
        self.assertFalse(self.output.exists())


    @unittest.skipUnless(os.name == "posix", "All-target gate fixture requires POSIX archives")
    def test_old_or_failed_runtime_extraction_evidence_blocks_the_entire_release(self):
        directories = self.prepare_all()
        linux = directories["x86_64-unknown-linux-gnu"]
        compression = linux / "appimage-compression.json"
        original = json.loads(compression.read_text())
        for value in [False, None]:
            changed = dict(original)
            if value is None:
                changed.pop("runtimeExtractionVerified")
            else:
                changed["runtimeExtractionVerified"] = value
            compression.write_text(json.dumps(changed))
            self.refresh(linux)
            with self.subTest(value=value), self.assertRaises(ValueError):
                collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
            self.assertFalse(self.output.exists())


    @unittest.skipUnless(os.name == "posix", "All-target gate fixture requires POSIX archives")
    def test_collector_copy_failure_is_atomic_and_keeps_inputs(self):
        directories = self.prepare_all()
        original = collect_release.shutil.copy2
        def failed_copy(source, output):
            original(source, output)
            output.write_bytes(b"changed-during-copy-fixture")
        with patch.object(collect_release.shutil, "copy2", side_effect=failed_copy), self.assertRaises(ValueError):
            collect_release.collect(self.inputs, self.output, "v1.0.2", self.root)
        self.assertFalse(self.output.exists())
        self.assertTrue(all((directory / "release-manifest.json").is_file() for directory in directories.values()))

    def test_wrong_tag_stops_before_creating_output(self):
        with self.assertRaises(ValueError):
            collect_release.collect(self.inputs, self.output, "v1.0.1", self.root)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
