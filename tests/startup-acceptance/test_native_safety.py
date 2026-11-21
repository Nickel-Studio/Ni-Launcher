"""Native harness safety tests. No executable is launched, including the PE fixture."""
import ast
import ctypes
import hashlib
import io
import json
import os
import socket
import stat
import struct
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from dataclasses import replace
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

import native_release as native


class NativeSafetyTests(unittest.TestCase):
    def setUp(self):
        # Fail closed even if a new test accidentally reaches any native launch API.
        self.kernel = MagicMock()
        self.kernel.CreateProcessW.side_effect = AssertionError("Safety tests must never launch an executable")
        self.win_dll_patch = patch.object(native.ctypes, "WinDLL", return_value=self.kernel, create=True)
        self.win_dll_patch.start()
        self.addCleanup(self.win_dll_patch.stop)
        self.spawn_patch = patch.object(native.subprocess, "Popen", side_effect=AssertionError("Safety tests must never spawn a process"))
        self.spawn = self.spawn_patch.start()
        self.addCleanup(self.spawn_patch.stop)
        self.reference_patch = patch.object(native, "load_microsoft_reference", side_effect=AssertionError("Safety tests must never read real API configuration"))
        self.reference_loader = self.reference_patch.start()
        self.addCleanup(self.reference_patch.stop)
        native.reject_links(native.EVIDENCE_ROOT)
        native.EVIDENCE_ROOT.mkdir(parents=True, exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="native-safety-", dir=native.EVIDENCE_ROOT)
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.sources = self.root / "source"
        self.sources.mkdir()
        self.source = self.sources / "fixture.exe"
        # Inert PE header, not a runnable application; all launch paths are mocked.
        header = bytearray(512)
        header[:2] = b"MZ"
        struct.pack_into("<I", header, 60, 128)
        header[128:132] = b"PE\x00\x00"
        self.source.write_bytes(header)
        self.protocol = native.load_protocol()

    def prepare(self, name="copy"):
        return native.prepare_copy(self.source, self.root / name, self.protocol, native.file_hash(self.source))

    def test_derives_current_backend_protocol_and_release(self):
        self.assertEqual(self.protocol.version, "1.0.2")
        self.assertEqual(self.protocol.eula_revision, "minecraft-eula-v1")
        self.assertEqual(self.protocol.data_directory, "Nickel")
        self.assertEqual(self.protocol.game_directory, ".minecraft")
        self.assertEqual(self.protocol.import_record, "portable-import.json")

    def test_prepare_default_never_spawns_a_process(self):
        output = self.root / "prepare-default"
        with patch.object(native.subprocess, "Popen", side_effect=AssertionError("No process may be spawned")) as spawn:
            self.assertEqual(native.main(["--exe", str(self.source), "--evidence-dir", str(output)]), 0)
            spawn.assert_not_called()
        report = json.loads((output / "native-prepared.json").read_text(encoding="utf-8"))
        self.assertFalse(report["nativeExecuted"])
        self.assertFalse(report["nativeAcceptance"])
        self.assertTrue(Path(report["copiedExe"]).is_file())

    def test_explicit_prepare_only_never_spawns_a_process(self):
        with patch.object(native.subprocess, "Popen", side_effect=AssertionError("No process may be spawned")) as spawn:
            self.assertEqual(native.main(["--exe", str(self.source), "--prepare-only", "--evidence-dir", str(self.root / "prepare-explicit")]), 0)
            spawn.assert_not_called()

    def test_copies_only_binary_not_adjacent_user_configuration(self):
        (self.sources / "accounts.json").write_text('{"fixtureOnly":true}', encoding="utf-8")
        (self.sources / "credentials.key").write_bytes(b"synthetic-test-key")
        (self.sources / "secrets.local.json").write_text('{"fixtureOnly":true}', encoding="utf-8")
        (self.sources / self.protocol.marker).write_text("source marker must not be copied", encoding="utf-8")
        sandbox = self.prepare()
        for name in ["accounts.json", "credentials.key", "secrets.local.json"]:
            self.assertFalse((sandbox.root / name).exists())
            self.assertFalse((sandbox.record.parent / name).exists())
        marker = json.loads((sandbox.root / self.protocol.marker).read_text(encoding="utf-8"))
        self.assertEqual(marker["dataDirectory"], self.protocol.data_directory)
        self.assertEqual(sandbox.local_record(), {})

    def test_source_binary_and_timestamp_remain_unchanged(self):
        before_hash = native.file_hash(self.source)
        before_time = self.source.stat().st_mtime_ns
        sandbox = self.prepare()
        self.assertEqual(native.file_hash(self.source), before_hash)
        self.assertEqual(self.source.stat().st_mtime_ns, before_time)
        self.assertEqual(native.file_hash(sandbox.exe), before_hash)
        self.assertNotEqual(sandbox.exe.resolve(), self.source.resolve())

    def test_existing_output_is_never_overwritten(self):
        output = self.root / "occupied"
        output.mkdir()
        sentinel = output / "sentinel.txt"
        sentinel.write_text("keep", encoding="ascii")
        with self.assertRaises(native.SafetyError):
            native.prepare_copy(self.source, output, self.protocol)
        self.assertEqual(sentinel.read_text(encoding="ascii"), "keep")

    def test_output_outside_evidence_is_rejected_before_creation(self):
        output = native.ROOT / "native-safety-outside-must-not-exist"
        self.assertFalse(output.exists())
        with self.assertRaises(native.SafetyError):
            native.prepare_copy(self.source, output, self.protocol)
        self.assertFalse(output.exists())

    def test_invalid_executable_headers_are_rejected(self):
        bad = self.sources / "invalid.exe"
        bad.write_bytes(b"not a PE executable")
        with self.assertRaises(native.SafetyError):
            native.prepare_copy(bad, self.root / "invalid-copy", self.protocol)
        self.assertFalse((self.root / "invalid-copy").exists())

    def test_non_executable_extension_is_rejected(self):
        bad = self.sources / "fixture.txt"
        bad.write_bytes(self.source.read_bytes())
        with self.assertRaises(native.SafetyError):
            native.executable_source(bad)

    def test_release_hash_mismatch_prevents_copy(self):
        with self.assertRaises(native.SafetyError):
            native.prepare_copy(self.source, self.root / "wrong-hash", self.protocol, "0" * 64)
        self.assertFalse((self.root / "wrong-hash").exists())

    def test_launch_guard_rejects_original_source(self):
        sandbox = self.prepare()
        with patch.object(native.subprocess, "Popen") as spawn:
            with self.assertRaises(native.SafetyError):
                native.assert_copy(replace(sandbox, exe=self.source))
            spawn.assert_not_called()

    def test_launch_guard_rejects_changed_binary(self):
        sandbox = self.prepare()
        sandbox.exe.write_bytes(b"changed fixture")
        with self.assertRaises(native.SafetyError):
            native.assert_copy(sandbox)

    def test_completed_import_guard_prevents_pending_payload(self):
        sandbox = self.prepare()
        marker = json.loads((sandbox.root / self.protocol.marker).read_text(encoding="utf-8"))
        imported = json.loads((sandbox.record.parent / self.protocol.import_record).read_text(encoding="utf-8"))
        self.assertEqual(marker["packageId"], imported["packageId"])
        self.assertEqual(imported["versionId"], "startup-acceptance-fixture")
        self.assertFalse((sandbox.record.parent / self.protocol.payload).exists())
        native.assert_copy(sandbox)
        (sandbox.record.parent / self.protocol.import_record).write_text("{}", encoding="utf-8")
        with self.assertRaises(native.SafetyError):
            native.assert_copy(sandbox)

    def test_launch_guard_rejects_extra_marker_fields(self):
        sandbox = self.prepare()
        path = sandbox.root / self.protocol.marker
        marker = json.loads(path.read_text(encoding="utf-8"))
        marker["personalization"] = "outside-config.json"
        path.write_text(json.dumps(marker), encoding="utf-8")
        with self.assertRaises(native.SafetyError):
            native.assert_copy(sandbox)

    def test_private_webview_environment_drops_ambient_secrets(self):
        sandbox = self.prepare()
        session = sandbox.root / "session"
        parent = {"Path": "synthetic-path", "SystemRoot": "C:\\Windows", "APPDATA": "outside-profile", "VITE_FIXTURE_SECRET": "do-not-propagate", "API_TOKEN": "do-not-propagate", "NICKEL_SECRETS": "outside.json", "NICKEL_BUILD_SECRETS": "outside-build.json", "NICKEL_MSA_CLIENT_ID": "do-not-propagate", "NICKEL_CURSEFORGE_API_KEY": "do-not-propagate", "NICKEL_DIAG": "1", "HTTP_PROXY": "outside-proxy", "PYTHONPATH": "outside-modules", "WEBVIEW2_USER_DATA_FOLDER": "outside-webview", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS": "outside-debug-flags", "WEBVIEW2_PIPE_FOR_SCRIPT_DEBUGGER": "outside-pipe"}
        before = dict(parent)
        environment = native.isolated_environment(sandbox, session, 45678, parent)
        self.assertEqual(parent, before)
        for key in parent:
            if key.upper() not in {"PATH", "SYSTEMROOT", "APPDATA", "WEBVIEW2_USER_DATA_FOLDER", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"}:
                self.assertNotIn(key.upper(), environment)
        for name in ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "HOME", "TEMP", "TMP", "WEBVIEW2_USER_DATA_FOLDER"]:
            self.assertTrue(Path(environment[name]).is_relative_to(session))
            self.assertTrue(Path(environment[name]).is_dir())
        self.assertIn("--remote-debugging-address=127.0.0.1", environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"])
        self.assertIn("--remote-debugging-port=45678", environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"])
        self.assertIn("--enable-automation", environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"])
        self.assertEqual(environment["PATH"], "synthetic-path")

    def test_reparse_points_are_rejected(self):
        original = Path.lstat

        def attributes(path, *args, **kwargs):
            if path == self.source:
                return SimpleNamespace(st_mode=stat.S_IFREG, st_file_attributes=0x400)
            return original(path, *args, **kwargs)

        with patch.object(Path, "lstat", attributes):
            with self.assertRaises(native.SafetyError):
                native.executable_source(self.source)

    def test_seed_rejects_non_consent_fields(self):
        sandbox = self.prepare()
        with self.assertRaises(native.SafetyError):
            sandbox.seed_record({"realName": "synthetic fixture must not be stored"})
        self.assertFalse(sandbox.record.exists())

    @unittest.skipUnless(os.name == "nt", "Windows-specific mocked launch")
    def test_mocked_launch_targets_only_the_copy_and_private_environment(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "mocked-session", [])
        with patch.object(native, "WindowsJob") as job, patch.object(native, "OwnedNativeProcess") as spawn:
            spawn.return_value.pid = 999999
            spawn.return_value.poll.return_value = 0
            spawn.return_value.wait.return_value = 0
            with self.assertRaises(native.NativeAssertion):
                session.__enter__()
        arguments, options = spawn.call_args
        self.assertIs(arguments[0], sandbox)
        self.assertFalse(options)
        self.assertTrue(Path(arguments[1]["WEBVIEW2_USER_DATA_FOLDER"]).is_relative_to(session.output))
        self.assertIs(arguments[2], job.return_value)
        self.assertFalse(session.details["forcedCleanup"])
        self.assertFalse(session.details["cleanupFailed"])
        self.assertTrue((session.output / "session.json").is_file())
        job.return_value.close.assert_called_once_with()

    def test_cleanup_is_idempotent_and_never_kills_by_process_name(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "mocked-cleanup", [])
        session.output.mkdir()
        session.process = MagicMock(pid=999999)
        session.process.poll.return_value = None
        session.job = MagicMock()
        session.job.close.side_effect = lambda: setattr(session.process.poll, "return_value", 0)
        session.cleanup()
        session.cleanup()
        session.process.terminate.assert_not_called()
        session.process.kill.assert_not_called()
        session.process.close.assert_called_once_with()
        session.job.close.assert_called_once_with()
        self.assertTrue(session.details["forcedCleanup"])
        self.assertFalse(session.details["cleanupFailed"])
        self.assertTrue((session.output / "session.json").is_file())

    @unittest.skipUnless(os.name == "nt", "Windows-specific real file-sharing test")
    def test_real_windows_lock_blocks_replace_and_releases_for_retry(self):
        sandbox = self.prepare()
        sandbox.seed_record({})
        temporary = sandbox.record.parent / "replacement.json"
        temporary.write_text('{"eulaVersion":"minecraft-eula-v1"}', encoding="utf-8")
        self.win_dll_patch.stop()
        with native.WindowsRecordLock(sandbox.record):
            self.assertEqual(sandbox.local_record(), {})
            with self.assertRaises(OSError):
                os.replace(temporary, sandbox.record)
        os.replace(temporary, sandbox.record)
        self.assertEqual(sandbox.local_record()["eulaVersion"], "minecraft-eula-v1")
        self.assertFalse(temporary.exists())

    def test_run_without_explicit_hash_stops_before_any_reference_read_or_launch(self):
        output = self.root / "must-not-start"
        with patch.object(native, "OwnedNativeProcess") as launch:
            with self.assertRaises(native.SafetyError):
                native.main(["--run", "--exe", str(self.source), "--evidence-dir", str(output)])
            launch.assert_not_called()
        self.reference_loader.assert_not_called()
        self.spawn.assert_not_called()
        self.kernel.CreateProcessW.assert_not_called()
        self.assertFalse(output.exists())

    def test_hash_validation_rejects_empty_partial_and_malformed_values(self):
        for value in ["", "abc", "0" * 63, "0" * 65, "g" * 64, " " + "0" * 64, True]:
            with self.subTest(value=value), self.assertRaises(native.SafetyError):
                native.validated_sha256(value)
        self.assertEqual(native.validated_sha256("A" * 64), "a" * 64)

    def test_copy_without_authorized_hash_cannot_be_launched(self):
        sandbox = native.prepare_copy(self.source, self.root / "not-authorized", self.protocol)
        native.assert_copy(sandbox)
        with self.assertRaises(native.SafetyError):
            native.assert_copy(sandbox, launching=True)
        with self.assertRaises(native.SafetyError):
            native.OwnedNativeProcess(sandbox, {}, MagicMock())
        self.kernel.CreateProcessW.assert_not_called()

    def test_main_hash_mismatch_creates_no_output(self):
        output = self.root / "wrong-main-hash"
        with self.assertRaises(native.SafetyError):
            native.main(["--run", "--exe", str(self.source), "--expected-sha256", "0" * 64, "--evidence-dir", str(output)])
        self.assertFalse(output.exists())
        self.reference_loader.assert_not_called()

    def test_help_never_reads_source_reference_or_launches(self):
        with redirect_stdout(io.StringIO()) as captured, patch.object(native, "load_protocol") as protocol:
            with self.assertRaises(SystemExit) as exited:
                native.main(["--help"])
        self.assertEqual(exited.exception.code, 0)
        self.assertIn("mandatory for --run", " ".join(captured.getvalue().split()))
        protocol.assert_not_called()
        self.reference_loader.assert_not_called()
        self.kernel.CreateProcessW.assert_not_called()

    def test_portable_path_protocol_is_validated_before_any_creation(self):
        for field, value in [("launcher", "../outside.exe"), ("data_directory", "C:\\outside"), ("marker", "CON.json"), ("payload", "bad:stream"), ("launcher", "same.txt"), ("data_directory", self.protocol.game_directory)]:
            with self.subTest(field=field), self.assertRaises(native.SafetyError):
                native.prepare_copy(self.source, self.root / "invalid-protocol", replace(self.protocol, **{field: value}))
            self.assertFalse((self.root / "invalid-protocol").exists())

    def test_source_change_during_copy_is_rejected(self):
        original = native.shutil.copyfileobj

        def copy_and_change(incoming, outgoing, size):
            original(incoming, outgoing, size)
            self.source.write_bytes(self.source.read_bytes() + b"changed")

        with patch.object(native.shutil, "copyfileobj", copy_and_change):
            with self.assertRaises(native.SafetyError):
                self.prepare()
        self.kernel.CreateProcessW.assert_not_called()

    def test_hard_linked_source_and_records_are_rejected(self):
        linked = self.sources / "linked.exe"
        os.link(self.source, linked)
        with self.assertRaises(native.SafetyError):
            native.executable_source(self.source)
        linked.unlink()
        sandbox = self.prepare()
        sandbox.seed_record({})
        os.link(sandbox.record, self.sources / "linked-record.json")
        with self.assertRaises(native.SafetyError):
            sandbox.local_record()

    @unittest.skipUnless(os.name == "nt", "Windows path validation")
    def test_network_device_alternate_stream_and_drive_relative_paths_are_rejected(self):
        for value in [r"\\server\share\release.exe", r"\\?\UNC\server\share\release.exe", r"\\.\PhysicalDrive0", "C:release.exe", r"C:\release.exe:hidden", "//server/share/release.exe"]:
            with self.subTest(path=value), self.assertRaises(native.SafetyError):
                native.local_path(value)
        self.assertTrue(native.same_local_path("\\\\?\\" + str(self.source), self.source))

    def test_private_tree_rejects_nested_reparse_before_traversal(self):
        sandbox = self.prepare()
        nested = sandbox.root / "nested"
        nested.mkdir()
        original = Path.lstat

        def attributes(path, *args, **kwargs):
            if path == nested:
                return SimpleNamespace(st_mode=stat.S_IFDIR, st_file_attributes=0x400)
            return original(path, *args, **kwargs)

        with patch.object(Path, "lstat", attributes):
            with self.assertRaises(native.SafetyError):
                native.assert_copy(sandbox)

    def test_external_json_in_copy_is_rejected_without_reading_it(self):
        sandbox = self.prepare()
        configuration = sandbox.root / "secrets.local.json"
        configuration.write_text("synthetic-sensitive-marker", encoding="ascii")
        with self.assertRaises(native.SafetyError):
            native.assert_copy(sandbox)
        self.assertEqual(configuration.read_text(encoding="ascii"), "synthetic-sensitive-marker")

    def test_payload_is_never_allowed_in_native_startup_copy(self):
        sandbox = self.prepare()
        (sandbox.record.parent / self.protocol.payload).write_bytes(b"synthetic payload must not run")
        with self.assertRaises(native.SafetyError):
            native.assert_copy(sandbox)

    def test_record_cannot_be_redirected_into_another_sandbox(self):
        sandbox = self.prepare()
        other = self.prepare("other")
        redirected = replace(sandbox, record=other.record)
        with self.assertRaises(native.SafetyError):
            redirected.seed_record({})
        self.assertFalse(other.record.exists())

    def test_record_field_types_cannot_hide_user_data(self):
        for value in [{"eulaAcceptedAt": True}, {"eulaAcceptedAt": 0}, {"eulaAcceptedAt": -1}, {"eulaAcceptedAt": 1.2}, {"eulaAcceptedAt": {}}, {"eulaAcceptedAt": 2 ** 53}, {"eulaVersion": {"private": "data"}}, {"changelogSeenVersion": "x\n"}]:
            with self.subTest(value=value), self.assertRaises(native.SafetyError):
                native.validate_record(value)

    def test_cdp_port_cannot_inject_browser_arguments(self):
        sandbox = self.prepare()
        for value in [0, 80, 65536, True, "45678 --outside"]:
            with self.subTest(value=value), self.assertRaises(native.SafetyError):
                native.isolated_environment(sandbox, self.root / "invalid-port", value, {})
        self.assertFalse((self.root / "invalid-port").exists())

    def test_cdp_discovery_accepts_only_direct_loopback_websocket(self):
        valid = "ws://127.0.0.1:45678/devtools/browser/fixture-id"
        self.assertEqual(native.validated_cdp_websocket(valid, 45678), valid)
        for value in ["ws://localhost:45678/devtools/browser/id", "ws://127.0.0.1:1/devtools/browser/id", "ws://outside:45678/devtools/browser/id", "wss://127.0.0.1:45678/devtools/browser/id", "ws://user:pass@127.0.0.1:45678/devtools/browser/id", valid + "?private=value", valid + "#fragment", "ws://[::1]:45678/devtools/browser/id", None]:
            with self.subTest(value=value), self.assertRaises(native.SafetyError):
                native.validated_cdp_websocket(value, 45678)

    def test_cdp_http_redirects_are_rejected(self):
        with self.assertRaises(native.SafetyError):
            native.NoCdpRedirects().redirect_request(None, None, 302, "redirect", {}, "https://outside.invalid")

    def test_cdp_requires_owned_loopback_listener_in_both_address_families(self):
        job = MagicMock()
        job.contains_pid.side_effect = lambda pid: pid == 999999
        valid = [{"address": "127.0.0.1", "pid": 999999}]
        with patch.object(native, "tcp_listeners", return_value=valid):
            native.assert_owned_cdp(45678, job)
        for rows in [[], [{"address": "0.0.0.0", "pid": 999999}], [{"address": "127.0.0.1", "pid": 17584}], valid + [{"address": "::", "pid": 999999}], valid + [{"address": "::1", "pid": 12345}]]:
            with self.subTest(rows=rows), patch.object(native, "tcp_listeners", return_value=rows):
                with self.assertRaises(native.SafetyError):
                    native.assert_owned_cdp(45678, job)

    def test_webview_profile_and_debug_flags_are_verified_without_logging_arguments(self):
        profile = self.root / "private-webview"
        valid = ["runtime.exe", "--user-data-dir=" + str(profile), "--remote-debugging-port=45678", "--remote-debugging-address=127.0.0.1"]
        native.assert_browser_arguments(valid, profile, 45678)
        for arguments in [[], valid + ["--user-data-dir=outside"], [item for item in valid if not item.startswith("--user-data-dir")], [item.replace("45678", "45679") for item in valid]]:
            with self.subTest(arguments=arguments), self.assertRaises(native.SafetyError):
                native.assert_browser_arguments(arguments, profile, 45678)

    def test_dev_fixture_urls_cannot_count_as_production_tauri_pages(self):
        self.assertTrue(native.production_page_url("http://tauri.localhost/"))
        self.assertTrue(native.production_page_url("tauri://localhost/"))
        for url in ["http://localhost:1420/", "http://127.0.0.1:1420/", "about:blank", "file:///fixture.html", "https://outside.invalid/", "http://tauri.localhost/?fixture=1"]:
            with self.subTest(url=url):
                self.assertFalse(native.production_page_url(url))

    def test_private_microsoft_reference_defaults_match_backend_projection(self):
        source = self.root / "synthetic-reference.json"
        source.write_text(json.dumps({"microsoft": {"clientId": "  synthetic-client  ", "authority": " ", "redirectUri": "bad\u0000value", "clientSecret": "synthetic-private-marker"}, "curseforge": {"apiKey": "synthetic-private-key"}}), encoding="utf-8")
        self.reference_patch.stop()
        with redirect_stdout(io.StringIO()) as stdout, redirect_stderr(io.StringIO()) as stderr:
            reference = native.load_microsoft_reference(source)
            challenge = reference.challenge()
        expected = json.dumps([challenge["salt"], "synthetic-client", "consumers", "nickel://auth"], separators=(",", ":")).encode("utf-8")
        self.assertTrue(challenge["digest"] == hashlib.sha256(expected).hexdigest())
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")
        self.assertNotIn("synthetic-client", repr(reference))
        self.assertNotIn("synthetic-private", repr(reference))

    def test_private_reference_parse_errors_do_not_echo_values(self):
        self.reference_patch.stop()
        source = self.root / "synthetic-broken-reference.json"
        for text in ['{"microsoft":{"clientId":"synthetic-private-marker"},broken}', '{"microsoft":{"clientId":"00000000-0000-0000-0000-000000000000"}}', ' ' * (256 * 1024 + 1)]:
            source.write_text(text, encoding="ascii")
            with self.assertRaises(native.SafetyError) as raised:
                native.load_microsoft_reference(source)
            self.assertNotIn("synthetic-private-marker", str(raised.exception))

    def test_embedded_config_probe_returns_only_boolean_evidence(self):
        sandbox = self.prepare()
        reference = native.MicrosoftReference({"clientId": "synthetic-private-client", "authority": "synthetic-authority", "redirectUri": "synthetic://callback"})
        page = MagicMock()
        page.evaluate.return_value = {"ipcResolved": True, "publicFieldsOnly": True, "matches": True}
        result = native.verify_embedded_microsoft(page, sandbox, reference)
        self.assertTrue(all(type(value) is bool for value in result.values()))
        self.assertFalse(result["realAuthenticationExercised"])
        self.assertFalse(result["curseForgeAuthenticationVerified"])
        script, challenge = page.evaluate.call_args.args
        self.assertIn("invoke('msa_config_get', {})", script)
        self.assertEqual(set(challenge), {"salt", "digest"})
        self.assertNotIn("synthetic-private-client", script + json.dumps(challenge) + json.dumps(result))

    def test_embedded_config_probe_mismatch_or_extra_fields_never_pass(self):
        sandbox = self.prepare()
        reference = native.MicrosoftReference({"clientId": "synthetic-client", "authority": "consumers", "redirectUri": "synthetic://auth"})
        page = MagicMock()
        for value in [{"ipcResolved": True, "publicFieldsOnly": True, "matches": False}, {"ipcResolved": True, "publicFieldsOnly": False, "matches": True}, {"ipcResolved": True, "publicFieldsOnly": True, "matches": True, "clientSecret": "synthetic-private-marker"}, {"ipcResolved": 1, "publicFieldsOnly": True, "matches": True}, None]:
            page.evaluate.return_value = value
            with self.subTest(value=value), self.assertRaises((native.SafetyError, native.NativeAssertion)) as raised:
                native.verify_embedded_microsoft(page, sandbox, reference)
            self.assertNotIn("synthetic-private-marker", str(raised.exception))

    def test_raw_failures_do_not_leak_ipc_values_or_source_lines(self):
        for kind in [RuntimeError, native.SafetyError, native.NativeAssertion]:
            try:
                raise kind("synthetic-private-marker")
            except Exception as error:
                with patch.object(native.traceback, "extract_tb", side_effect=AssertionError("Source lines must not be read")):
                    failure = native.safe_failure(error)
            self.assertNotIn("synthetic-private-marker", json.dumps(failure))
            self.assertEqual(failure["error"], "Native operation failed; raw exception and IPC details are withheld")
            self.assertEqual(set(failure["frames"][0]), {"file", "line", "function"})

    def test_cleanup_closes_children_even_after_normal_parent_exit(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "normal-cleanup", [])
        session.output.mkdir()
        session.process = MagicMock(pid=999999)
        session.process.poll.return_value = 0
        session.job = MagicMock()
        session.cleanup()
        session.job.close.assert_called_once_with()
        session.process.terminate.assert_not_called()
        self.assertFalse(session.details["forcedCleanup"])

    def test_cleanup_failure_is_not_silently_reported_as_success(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "failed-cleanup", [])
        session.output.mkdir()
        session.process = MagicMock(pid=999999)
        session.process.poll.return_value = 0
        session.job = MagicMock()
        session.job.close.side_effect = RuntimeError("synthetic-private-marker")
        with self.assertRaises(native.SafetyError):
            session.cleanup()
        self.assertTrue(session.details["cleanupFailed"])
        self.assertNotIn("synthetic-private-marker", (session.output / "session.json").read_text(encoding="utf-8"))

    def test_cleanup_protects_controller_pids(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "protected-cleanup", [])
        session.output.mkdir()
        session.process = MagicMock(pid=17584)
        session.process.poll.return_value = None
        session.job = MagicMock()
        with self.assertRaises(native.SafetyError):
            session.cleanup()
        session.process.terminate.assert_not_called()
        session.process.kill.assert_not_called()
        session.job.close.assert_not_called()

    def test_windows_job_rejects_foreign_or_protected_processes(self):
        job = native.WindowsJob.__new__(native.WindowsJob)
        job.handle, job.kernel = 321, MagicMock()
        for process in [SimpleNamespace(created=False, pid=999999, handle=123), SimpleNamespace(created=True, pid=17584, handle=123)]:
            with self.assertRaises(native.SafetyError):
                job.assign(process)
        job.kernel.AssignProcessToJobObject.assert_not_called()
        self.assertFalse(job.contains_pid(17584))
        job.kernel.OpenProcess.assert_not_called()

    def test_native_exit_cannot_be_satisfied_by_forced_cleanup(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "exit-check", [])
        session.exited.set()
        session.exit_info = {"exitCode": 0}
        session.details["forcedCleanup"] = True
        session.process = MagicMock(pid=999999)
        with self.assertRaises(native.NativeAssertion):
            session.wait_exit()

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked suspended launch")
    def test_suspended_process_is_hash_checked_contained_then_resumed(self):
        sandbox = self.prepare()
        environment = {"SYSTEMROOT": "C:\\Windows", "USERPROFILE": str(self.root / "profile")}
        job = MagicMock()
        events = []

        def create(application, command, _process_security, _thread_security, inherit, flags, block, cwd, startup, information):
            self.assertEqual(application, str(sandbox.exe))
            self.assertEqual(command.value, native.subprocess.list2cmdline([str(sandbox.exe)]))
            self.assertEqual(cwd, str(sandbox.root))
            self.assertFalse(inherit)
            self.assertEqual(flags & 4, 4)
            self.assertEqual(flags & 0x400, 0x400)
            value = ctypes.cast(information, ctypes.POINTER(native.ProcessInformation)).contents
            value.process, value.thread, value.pid = 123, 456, 999999
            events.append("create-suspended")
            return True

        def image(_handle, _flags, output, _size):
            output.value = str(sandbox.exe)
            events.append("verify-image")
            return True

        self.kernel.CreateProcessW.side_effect = create
        self.kernel.QueryFullProcessImageNameW.side_effect = image
        self.kernel.ResumeThread.side_effect = lambda _thread: events.append("resume") or 1
        job.assign.side_effect = lambda _process: events.append("assign-job")
        process = native.OwnedNativeProcess(sandbox, environment, job)
        self.assertEqual(events, ["create-suspended", "verify-image", "assign-job", "resume"])
        process.close()
        self.kernel.TerminateProcess.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked launch failure")
    def test_job_assignment_failure_never_resumes_the_release(self):
        sandbox = self.prepare()

        def create(*arguments):
            value = ctypes.cast(arguments[-1], ctypes.POINTER(native.ProcessInformation)).contents
            value.process, value.thread, value.pid = 123, 456, 999999
            return True

        self.kernel.CreateProcessW.side_effect = create
        self.kernel.QueryFullProcessImageNameW.side_effect = lambda _handle, _flags, output, _size: setattr(output, "value", str(sandbox.exe)) or True
        self.kernel.WaitForSingleObject.return_value = 0
        self.kernel.GetExitCodeProcess.return_value = True
        job = MagicMock()
        job.assign.side_effect = native.SafetyError("synthetic assignment failure")
        with self.assertRaises(native.SafetyError):
            native.OwnedNativeProcess(sandbox, {}, job)
        self.kernel.ResumeThread.assert_not_called()
        self.kernel.CloseHandle.assert_any_call(456)

    @unittest.skipUnless(os.name == "nt", "Windows-only real job handle, no processes")
    def test_real_empty_windows_job_closes_without_touching_any_process(self):
        self.win_dll_patch.stop()
        job = native.WindowsJob()
        self.assertEqual(job.member_pids(), [])
        self.assertFalse(job.contains_pid(17584))
        job.close()
        job.close()
        self.assertIsNone(job.handle)

    @unittest.skipUnless(os.name == "nt", "Windows-only real TCP inspection, no processes")
    def test_real_tcp_listener_inspection_reads_only_the_reserved_loopback_port(self):
        self.win_dll_patch.stop()
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen(1)
            rows = native.tcp_listeners(listener.getsockname()[1])
        self.assertTrue(any(item["address"] == "127.0.0.1" and item["pid"] == os.getpid() for item in rows))

    def test_backend_exit_evidence_requires_a_paused_renderer(self):
        script = Path(native.__file__).read_text(encoding="utf-8")
        tree = ast.parse(script)
        unit = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "backend_rejection_without_renderer")
        code = ast.get_source_segment(script, unit)
        self.assertIn('renderer.send("Debugger.pause")', code)
        self.assertIn('check(paused.is_set()', code)
        self.assertNotIn("window_close", code)
        self.assertNotIn("setInterval", code)
        self.assertNotIn("terminate", code)


    def test_controller_environment_removes_protocol_logging_and_restores_parent(self):
        directory = self.root / "private-controller"
        with patch.dict(os.environ, {"DEBUG": "pw:protocol", "PWDEBUG": "1", "NODE_OPTIONS": "--inspect", "NICKEL_SECRETS": "synthetic-private-marker"}):
            before = dict(os.environ)
            with self.assertRaises(RuntimeError):
                with native.PrivateControllerEnvironment(directory):
                    for key in ["DEBUG", "PWDEBUG", "NODE_OPTIONS", "NICKEL_SECRETS"]:
                        self.assertNotIn(key, os.environ)
                    for key in ["TEMP", "TMP", "HOME", "APPDATA", "LOCALAPPDATA", "WEBVIEW2_USER_DATA_FOLDER"]:
                        self.assertTrue(Path(os.environ[key]).is_relative_to(directory))
                    raise RuntimeError("synthetic operation failure")
            self.assertEqual(dict(os.environ), before)
        self.kernel.CreateProcessW.assert_not_called()

    def test_cleanup_parent_poll_error_does_not_bypass_job_cleanup(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "poll-error-cleanup", [])
        session.output.mkdir()
        session.process = MagicMock(pid=999999)
        session.process.poll.side_effect = RuntimeError("synthetic-private-marker")
        session.job = MagicMock()
        with self.assertRaises(native.SafetyError):
            session.cleanup()
        session.job.close.assert_called_once_with()
        session.process.wait.assert_called_once_with(timeout=10)
        session.process.close.assert_called_once_with()
        self.assertTrue(session.details["cleanupFailed"])
        self.assertNotIn("synthetic-private-marker", (session.output / "session.json").read_text(encoding="utf-8"))

    def test_cleanup_write_failure_is_fail_closed_and_not_hidden_by_second_call(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "write-error-cleanup", [])
        session.output.mkdir()
        session.job = MagicMock()
        with patch.object(native, "write_json", side_effect=RuntimeError("synthetic-private-marker")):
            with self.assertRaises(native.SafetyError):
                session.cleanup()
        self.assertTrue(session.details["cleanupFailed"])
        with self.assertRaises(native.SafetyError):
            session.cleanup()
        session.job.close.assert_called_once_with()

    def test_job_cleanup_waits_for_every_owned_descendant(self):
        job = native.WindowsJob.__new__(native.WindowsJob)
        job.handle, job.kernel = 321, MagicMock()
        job.member_pids = MagicMock(side_effect=[[999999, 999998], [999998], []])
        with patch.object(native.time, "sleep"):
            job.close()
        job.kernel.TerminateJobObject.assert_called_once_with(321, 1)
        job.kernel.CloseHandle.assert_called_once_with(321)
        job.kernel.TerminateProcess.assert_not_called()
        self.assertIsNone(job.handle)

    def test_job_cleanup_refuses_protected_controller_members(self):
        for pid in native.PROTECTED_PIDS:
            job = native.WindowsJob.__new__(native.WindowsJob)
            job.handle, job.kernel = 321, MagicMock()
            job.member_pids = MagicMock(return_value=[pid])
            with self.subTest(pid=pid), self.assertRaises(native.SafetyError):
                job.close()
            job.kernel.TerminateJobObject.assert_not_called()
            job.kernel.CloseHandle.assert_not_called()

    def test_job_cleanup_timeout_does_not_discard_live_job_handle(self):
        job = native.WindowsJob.__new__(native.WindowsJob)
        job.handle, job.kernel = 321, MagicMock()
        job.member_pids = MagicMock(return_value=[999999])
        with patch.object(native.time, "perf_counter", side_effect=[0, 11]):
            with self.assertRaises(native.SafetyError):
                job.close()
        self.assertEqual(job.handle, 321)
        job.kernel.CloseHandle.assert_not_called()

    def test_job_close_failure_retains_handle_for_task_owned_retry(self):
        job = native.WindowsJob.__new__(native.WindowsJob)
        job.handle, job.kernel = 321, MagicMock()
        job.member_pids = MagicMock(return_value=[])
        job.kernel.CloseHandle.return_value = False
        with self.assertRaises(native.SafetyError):
            job.close()
        self.assertEqual(job.handle, 321)
        job.kernel.CloseHandle.return_value = True
        job.close()
        self.assertIsNone(job.handle)

    @unittest.skipUnless(os.name == "nt", "Windows-only real inert-image file lock")
    def test_executable_lock_denies_mutation_without_launching_any_image(self):
        sandbox = self.prepare()
        before = native.file_hash(sandbox.exe)
        self.win_dll_patch.stop()
        with native.WindowsExecutableLock(sandbox.exe):
            self.assertEqual(native.file_hash(sandbox.exe), before)
            with self.assertRaises(OSError):
                sandbox.exe.write_bytes(b"must not replace the locked inert image")
            with self.assertRaises(OSError):
                sandbox.exe.unlink()
        self.assertEqual(native.file_hash(sandbox.exe), before)
        self.kernel.CreateProcessW.assert_not_called()
        self.spawn.assert_not_called()

    def mock_created_native(self, sandbox, image=None):
        def create(*arguments):
            value = ctypes.cast(arguments[-1], ctypes.POINTER(native.ProcessInformation)).contents
            value.process, value.thread, value.pid = 123, 456, 999999
            return True

        def exit_code(_handle, output):
            ctypes.cast(output, ctypes.POINTER(native.wintypes.DWORD)).contents.value = 0
            return True

        self.kernel.CreateFileW.return_value = 789
        self.kernel.CreateProcessW.side_effect = create
        self.kernel.QueryFullProcessImageNameW.side_effect = lambda _handle, _flags, output, _size: setattr(output, "value", str(image or sandbox.exe)) or True
        self.kernel.ResumeThread.return_value = 1
        self.kernel.WaitForSingleObject.return_value = 0
        self.kernel.GetExitCodeProcess.side_effect = exit_code
        self.kernel.CloseHandle.return_value = True
        self.kernel.TerminateProcess.return_value = True

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked image verification")
    def test_created_foreign_image_is_never_assigned_or_resumed(self):
        sandbox = self.prepare()
        self.mock_created_native(sandbox, self.source)
        job = MagicMock()
        with self.assertRaises(native.SafetyError):
            native.OwnedNativeProcess(sandbox, {}, job)
        job.assign.assert_not_called()
        self.kernel.ResumeThread.assert_not_called()
        self.kernel.CloseHandle.assert_any_call(123)
        self.kernel.CloseHandle.assert_any_call(456)

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked pre-resume mutation")
    def test_copy_change_after_creation_is_rejected_before_resume(self):
        sandbox = self.prepare()
        self.mock_created_native(sandbox)
        job = MagicMock()
        job.assign.side_effect = lambda _process: sandbox.exe.write_bytes(b"synthetic changed image")
        with self.assertRaises(native.SafetyError):
            native.OwnedNativeProcess(sandbox, {}, job)
        self.kernel.ResumeThread.assert_not_called()
        self.assertEqual(native.file_hash(self.source), sandbox.sha256)

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked suspended cleanup")
    def test_failed_job_assignment_terminates_only_its_new_suspended_handle(self):
        sandbox = self.prepare()
        self.mock_created_native(sandbox)
        self.kernel.WaitForSingleObject.side_effect = [0x102, 0, 0]
        job = MagicMock()
        job.assign.side_effect = native.SafetyError("synthetic assignment failure")
        with self.assertRaises(native.SafetyError):
            native.OwnedNativeProcess(sandbox, {}, job)
        self.kernel.ResumeThread.assert_not_called()
        self.kernel.TerminateProcess.assert_called_once_with(123, 1)
        self.kernel.CloseHandle.assert_any_call(456)
        self.kernel.CloseHandle.assert_any_call(123)

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked constructor recovery")
    def test_session_recovers_owned_process_handles_from_creation_failure(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "creation-error-session", [])
        process = MagicMock(pid=999999, handle=123, resumed=True)
        process.poll.return_value = None
        failure = native.SafetyError("synthetic-private-marker")
        failure.owned_process, failure.creation_cleanup_failed = process, True
        with patch.object(native, "WindowsJob") as job, patch.object(native, "OwnedNativeProcess", side_effect=failure):
            job.return_value.close.side_effect = lambda: setattr(process.poll, "return_value", 0)
            with self.assertRaises(native.SafetyError):
                session.__enter__()
            job.return_value.close.assert_called_once_with()
        process.close.assert_called_once_with()
        process.terminate.assert_not_called()
        self.assertTrue(session.details["creationCleanupInitiallyFailed"])
        self.assertTrue(session.details["nativeExecuted"])
        self.assertTrue(session.details["forcedCleanup"])
        self.assertFalse(session.details["cleanupFailed"])
        self.assertNotIn("synthetic-private-marker", (session.output / "session.json").read_text(encoding="utf-8"))
        self.kernel.CreateProcessW.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked CDP foreign listener")
    def test_foreign_cdp_listener_never_receives_http_or_playwright_attachment(self):
        sandbox = self.prepare()
        playwright = MagicMock()
        session = native.NativeSession(playwright, sandbox, self.root / "foreign-cdp-session", [])
        foreign = [{"address": "127.0.0.1", "pid": 17584}]
        with patch.object(native, "WindowsJob") as job, patch.object(native, "OwnedNativeProcess") as process, patch.object(native, "tcp_listeners", return_value=foreign), patch.object(native, "build_opener") as opener, patch.object(native.threading, "Thread") as observer:
            process.return_value.pid = 999999
            process.return_value.poll.return_value = None
            job.return_value.contains_pid.return_value = False
            job.return_value.close.side_effect = lambda: setattr(process.return_value.poll, "return_value", 0)
            observer.return_value.is_alive.return_value = False
            with self.assertRaises(native.SafetyError):
                session.__enter__()
            opener.return_value.open.assert_not_called()
            job.return_value.close.assert_called_once_with()
        playwright.chromium.connect_over_cdp.assert_not_called()
        self.kernel.CreateProcessW.assert_not_called()

    def mock_backend_exit_session(self, pause=True, resume=False, exit_delay=5.02, ticks=1, resume_at=None, disconnect_at=None):
        session = MagicMock()
        session.exited = native.threading.Event()
        session.status.return_value = {"exiting": True}
        session.sandbox.local_record.return_value = {}
        session.wait_exit.return_value = {"controllerMonotonicSeconds": 10 + exit_delay, "exitCode": 0}
        session.page.locator.return_value.evaluate.return_value = True
        session.page.locator.return_value.count.return_value = 0
        renderer = session.page.context.new_cdp_session.return_value
        callbacks, waits = {}, 0
        renderer.on.side_effect = lambda name, callback: callbacks.update({name: callback})

        def wait(_milliseconds):
            nonlocal waits
            waits += 1
            if waits == resume_at:
                callbacks["Debugger.resumed"]({})
            if waits >= ticks:
                session.exited.set()
            if waits == disconnect_at:
                raise RuntimeError("synthetic-private-marker")

        def send(command):
            if command == "Debugger.pause":
                if pause:
                    callbacks["Debugger.paused"]({})
                if resume:
                    callbacks["Debugger.resumed"]({})
            return {}

        session.page.wait_for_timeout.side_effect = wait
        renderer.send.side_effect = send
        return session

    def test_backend_only_exit_requires_pause_and_no_renderer_resume(self):
        for pause, resume in [(False, False), (True, True)]:
            session = self.mock_backend_exit_session(pause=pause, resume=resume)
            with self.subTest(pause=pause, resume=resume), patch.object(native.time, "perf_counter", side_effect=[10, 10.01, 10.02]):
                with self.assertRaises(native.NativeAssertion):
                    native.backend_rejection_without_renderer(session)

    def test_mocked_backend_only_exit_records_no_authentication_or_frontend_fallback(self):
        session = self.mock_backend_exit_session()
        with patch.object(native.time, "perf_counter", side_effect=[10, 10.01, 10.02]):
            result = native.backend_rejection_without_renderer(session)
        self.assertTrue(result["rendererPaused"])
        self.assertTrue(result["rendererObservationContinuous"])
        self.assertFalse(result["rendererResumed"])
        self.assertFalse(result["frontendFallbackCouldRun"])
        self.assertFalse(result["frontendRefusalTimerStarted"])
        session.page.wait_for_timeout.assert_any_call(0)
        self.kernel.CreateProcessW.assert_not_called()

    def test_backend_only_exit_rejects_accelerated_or_late_process_exit(self):
        for delay in [4.9, 6.1]:
            session = self.mock_backend_exit_session(exit_delay=delay)
            with self.subTest(delay=delay), patch.object(native.time, "perf_counter", side_effect=[10, 10.01, 10.02]):
                with self.assertRaises(native.NativeAssertion):
                    native.backend_rejection_without_renderer(session)


    def test_backend_only_exit_rejects_renderer_resume_mid_wait_or_in_final_event_pump(self):
        for ticks, resume_at in [(4, 3), (1, 2)]:
            session = self.mock_backend_exit_session(ticks=ticks, resume_at=resume_at)
            with self.subTest(ticks=ticks, resume_at=resume_at), patch.object(native.time, "perf_counter", side_effect=[10, 10.01] + [10.02] * 20):
                with self.assertRaises(native.NativeAssertion):
                    native.backend_rejection_without_renderer(session)
        self.kernel.CreateProcessW.assert_not_called()

    def test_backend_only_exit_cannot_hide_an_observation_gap_behind_later_native_exit(self):
        session = self.mock_backend_exit_session(ticks=4, disconnect_at=2)
        with patch.object(native.time, "perf_counter", side_effect=[10, 10.01] + [10.02] * 20):
            with self.assertRaises(native.NativeAssertion) as raised:
                native.backend_rejection_without_renderer(session)
        session.wait_exit.assert_not_called()
        self.assertNotIn("synthetic-private-marker", str(raised.exception))

    def test_mocked_backend_only_exit_keeps_pumping_debugger_events_until_owned_exit(self):
        session = self.mock_backend_exit_session(ticks=4)
        with patch.object(native.time, "perf_counter", side_effect=[10, 10.01] + [10.02] * 20):
            result = native.backend_rejection_without_renderer(session)
        self.assertTrue(result["rendererObservationContinuous"])
        self.assertEqual(session.page.wait_for_timeout.call_count, 5)
        self.assertEqual(session.page.wait_for_timeout.call_args.args, (0,))

    def test_cleanup_observer_failure_still_disconnects_browser_and_records_sanitized_failure(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "observer-error-cleanup", [])
        session.output.mkdir()
        session.process = MagicMock(pid=999999)
        session.process.poll.return_value = 0
        session.job, session.observer, session.browser = MagicMock(), MagicMock(), MagicMock()
        session.observer.join.side_effect = RuntimeError("synthetic-private-marker")
        session.browser.is_connected.return_value = False
        with self.assertRaises(native.SafetyError):
            session.cleanup()
        session.job.close.assert_called_once_with()
        session.browser.close.assert_called_once_with()
        session.process.close.assert_not_called()
        self.assertTrue(session.details["cleanupFailed"])
        self.assertNotIn("synthetic-private-marker", (session.output / "session.json").read_text(encoding="utf-8"))

    def test_cleanup_browser_close_probe_failure_or_surviving_connection_never_passes(self):
        sandbox = self.prepare()
        for index, (close_error, connected) in enumerate([(True, False), (False, True), (False, RuntimeError("synthetic-private-marker"))]):
            session = native.NativeSession(None, sandbox, self.root / ("browser-error-cleanup-" + str(index)), [])
            session.output.mkdir()
            session.process = MagicMock(pid=999999)
            session.process.poll.return_value = 0
            session.job, session.browser = MagicMock(), MagicMock()
            if close_error:
                session.browser.close.side_effect = RuntimeError("synthetic-private-marker")
            if isinstance(connected, Exception):
                session.browser.is_connected.side_effect = connected
            else:
                session.browser.is_connected.return_value = connected
            with self.subTest(index=index), self.assertRaises(native.SafetyError):
                session.cleanup()
            session.job.close.assert_called_once_with()
            session.process.close.assert_called_once_with()
            session.browser.close.assert_called_once_with()
            session.browser.is_connected.assert_called_once_with()
            self.assertTrue(session.details["cleanupFailed"])
            self.assertNotIn("synthetic-private-marker", (session.output / "session.json").read_text(encoding="utf-8"))
            with self.assertRaises(native.SafetyError):
                session.cleanup()
            session.browser.close.assert_called_once_with()

    def test_cleanup_missing_output_is_not_silently_treated_as_success(self):
        sandbox = self.prepare()
        session = native.NativeSession(None, sandbox, self.root / "missing-cleanup-output", [])
        session.job = MagicMock()
        with self.assertRaises(native.SafetyError):
            session.cleanup()
        session.job.close.assert_called_once_with()
        self.assertTrue(session.details["cleanupFailed"])
        self.assertFalse(session.output.exists())

    def test_webview_flags_reject_dangling_duplicates_and_unsafe_profile_paths(self):
        profile = self.root / "private-webview"
        valid = ["runtime.exe", "--user-data-dir=" + str(profile), "--remote-debugging-port=45678", "--remote-debugging-address=127.0.0.1"]
        for flag in ["--user-data-dir", "--remote-debugging-port", "--remote-debugging-address"]:
            with self.subTest(flag=flag), self.assertRaises(native.SafetyError):
                native.assert_browser_arguments(valid + [flag], profile, 45678)
        with self.assertRaises(native.SafetyError):
            native.assert_browser_arguments(valid, native.ROOT, 45678)

    def test_owned_webview_probe_verifies_only_listener_members_and_never_logs_arguments(self):
        profile = self.root / "private-webview-probe"
        profile.mkdir()
        job = MagicMock()
        job.contains_pid.return_value = True
        rows = [{"address": "127.0.0.1", "pid": 999998}, {"address": "::1", "pid": 999998}]
        job.process_arguments.return_value = ["runtime.exe", "--user-data-dir=" + str(profile), "--remote-debugging-port=45678", "--remote-debugging-address=127.0.0.1", "--synthetic-private-marker"]
        with patch.object(native, "tcp_listeners", return_value=rows), redirect_stdout(io.StringIO()) as stdout, redirect_stderr(io.StringIO()) as stderr:
            native.assert_owned_webview(45678, job, profile)
        job.process_arguments.assert_called_once_with(999998)
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")
        with patch.object(native, "tcp_listeners", return_value=[{"address": "127.0.0.1", "pid": 17584}]):
            job.contains_pid.return_value = False
            job.process_arguments.reset_mock()
            with self.assertRaises(native.SafetyError):
                native.assert_owned_webview(45678, job, profile)
            job.process_arguments.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked WebView2 profile rejection")
    def test_wrong_owned_webview_profile_never_receives_http_or_playwright_attachment(self):
        sandbox = self.prepare()
        playwright = MagicMock()
        session = native.NativeSession(playwright, sandbox, self.root / "wrong-webview-session", [])
        rows = [{"address": "127.0.0.1", "pid": 999998}]
        with patch.object(native, "WindowsJob") as job, patch.object(native, "OwnedNativeProcess") as process, patch.object(native, "tcp_listeners", return_value=rows), patch.object(native, "build_opener") as opener, patch.object(native.threading, "Thread") as observer:
            process.return_value.pid = 999999
            process.return_value.poll.return_value = None
            job.return_value.contains_pid.return_value = True
            job.return_value.process_arguments.return_value = ["runtime.exe", "--user-data-dir=" + str(self.root / "synthetic-other-profile"), "--synthetic-private-marker"]
            job.return_value.close.side_effect = lambda: setattr(process.return_value.poll, "return_value", 0)
            observer.return_value.is_alive.return_value = False
            with self.assertRaises(native.SafetyError):
                session.__enter__()
            opener.return_value.open.assert_not_called()
            job.return_value.process_arguments.assert_called_once_with(999998)
            job.return_value.close.assert_called_once_with()
        playwright.chromium.connect_over_cdp.assert_not_called()
        self.assertNotIn("webviewProfileVerifiedBeforeAttachment", session.details)
        self.assertNotIn("synthetic-private-marker", (session.output / "session.json").read_text(encoding="utf-8"))
        self.kernel.CreateProcessW.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "Windows-only non-launching command-line parsing")
    def test_real_windows_argument_parser_handles_private_profile_quotes_without_spawning(self):
        arguments = [r"C:\Synthetic Runtime\runtime.exe", "--user-data-dir=" + str(self.root / "quoted private profile"), '--synthetic="quoted value"', "trailing\\", ""]
        self.win_dll_patch.stop()
        self.assertEqual(native.windows_command_arguments(native.subprocess.list2cmdline(arguments)), arguments)
        self.kernel.CreateProcessW.assert_not_called()
        self.spawn.assert_not_called()

    def test_observed_command_line_validation_precedes_any_native_parser_call(self):
        for command in [None, "", "synthetic\0private-marker", "x" * 65536]:
            with self.subTest(commandType=type(command).__name__), self.assertRaises(native.SafetyError):
                native.windows_command_arguments(command)
        self.kernel.CommandLineToArgvW.assert_not_called()

    def mock_listener_command_query(self, descriptor_change=None):
        job = native.WindowsJob.__new__(native.WindowsJob)
        job.handle, job.kernel = 321, self.kernel
        self.kernel.OpenProcess.return_value = 123
        self.kernel.CloseHandle.return_value = True

        def member(_process, _job, output):
            ctypes.cast(output, ctypes.POINTER(native.wintypes.BOOL)).contents.value = True
            return True

        class UnicodeString(ctypes.Structure):
            _fields_ = [("length", native.wintypes.USHORT), ("maximum", native.wintypes.USHORT), ("buffer", ctypes.c_void_p)]

        raw = "runtime.exe --synthetic-private-marker".encode("utf-16-le")

        def query(_process, category, document, _capacity, output):
            self.assertEqual(category, 60)
            required = ctypes.sizeof(UnicodeString) + len(raw) + 2
            ctypes.cast(output, ctypes.POINTER(native.wintypes.ULONG)).contents.value = required
            if document is None:
                return ctypes.c_long(0xC0000004).value
            value = UnicodeString.from_buffer(document)
            value.length, value.maximum = len(raw), len(raw) + 2
            value.buffer = ctypes.addressof(document) + ctypes.sizeof(UnicodeString)
            ctypes.memmove(value.buffer, raw + b"\0\0", len(raw) + 2)
            if descriptor_change:
                descriptor_change(value, document)
            return 0

        self.kernel.IsProcessInJob.side_effect = member
        self.kernel.NtQueryInformationProcess.side_effect = query
        return job

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked local command-line observation")
    def test_listener_command_line_probe_uses_read_only_owned_handle_and_local_descriptor(self):
        job = self.mock_listener_command_query()
        with patch.object(native, "windows_command_arguments", return_value=["runtime.exe"]) as parse, redirect_stdout(io.StringIO()) as stdout, redirect_stderr(io.StringIO()) as stderr:
            self.assertEqual(job.process_arguments(999998), ["runtime.exe"])
        self.kernel.OpenProcess.assert_called_once_with(0x1000, False, 999998)
        self.kernel.IsProcessInJob.assert_called_once()
        self.kernel.CloseHandle.assert_called_once_with(123)
        parse.assert_called_once_with("runtime.exe --synthetic-private-marker")
        self.assertEqual(stdout.getvalue() + stderr.getvalue(), "")
        self.kernel.CreateProcessW.assert_not_called()

    @unittest.skipUnless(os.name == "nt", "Windows-only mocked command-line descriptor bounds")
    def test_listener_command_probe_never_dereferences_foreign_or_invalid_descriptors(self):
        changes = [lambda value, document: setattr(value, "buffer", 1), lambda value, document: setattr(value, "buffer", ctypes.addressof(document)), lambda value, document: setattr(value, "buffer", ctypes.addressof(document) + len(document)), lambda value, document: setattr(value, "length", 1), lambda value, document: setattr(value, "length", 0), lambda value, document: setattr(value, "maximum", 0)]
        for index, change in enumerate(changes):
            job = self.mock_listener_command_query(change)
            self.kernel.CloseHandle.reset_mock()
            with self.subTest(index=index), patch.object(native.ctypes, "wstring_at") as dereference, self.assertRaises(native.SafetyError):
                job.process_arguments(999998)
            dereference.assert_not_called()
            self.kernel.CloseHandle.assert_called_once_with(123)

    def test_listener_command_probe_refuses_unowned_and_protected_processes_before_query(self):
        job = native.WindowsJob.__new__(native.WindowsJob)
        job.handle, job.kernel = 321, self.kernel
        for pid in [*native.PROTECTED_PIDS, 0, -1, True]:
            with self.subTest(pid=pid), self.assertRaises(native.SafetyError):
                job.process_arguments(pid)
        self.kernel.OpenProcess.assert_not_called()
        self.kernel.OpenProcess.return_value = 123
        self.kernel.IsProcessInJob.return_value = True
        self.kernel.CloseHandle.return_value = True
        with self.assertRaises(native.SafetyError):
            job.process_arguments(999998)
        self.kernel.NtQueryInformationProcess.assert_not_called()
        self.kernel.CloseHandle.assert_called_once_with(123)


if __name__ == "__main__":
    unittest.main(verbosity=2)
