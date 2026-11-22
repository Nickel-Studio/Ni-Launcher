"""Distribution acceptance guard regression; never launches an application."""
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

import native_security as security


class DistributionSessionTests(unittest.TestCase):
    def session(self, selected):
        root = Path("C:/synthetic/portable")
        sandbox = SimpleNamespace(root=root, exe=root / "Nickel.exe", sha256="0" * 64,
                                  protocol=SimpleNamespace(game_directory=".minecraft"))
        session = security.DistributionSession(None, sandbox, security.native.EVIDENCE_ROOT / "guard-fixture", [])
        session.page = MagicMock()
        session.page.evaluate.return_value = selected
        return session

    def test_default_initialization_precedes_the_unchanged_native_isolation_checks(self):
        session = self.session("C:/synthetic/portable/.minecraft")
        with patch.object(security.native, "same_local_path", return_value=True), \
                patch.object(security.native.NativeSession, "verify_isolation") as verify:
            session.verify_isolation()
        session.page.evaluate.assert_called_once_with("window.__TAURI_INTERNALS__.invoke('instance_ensure_default', {})")
        verify.assert_called_once_with()
        self.assertTrue(session.details["distributionDefaultInstanceInitialized"])

    def test_outside_or_invalid_default_never_reaches_the_base_isolation_check(self):
        for selected in ["C:/outside", None, 123]:
            session = self.session(selected)
            with self.subTest(selected=selected), \
                    patch.object(security.native, "same_local_path", return_value=False), \
                    patch.object(security.native.NativeSession, "verify_isolation") as verify:
                with self.assertRaises(security.native.NativeAssertion):
                    session.verify_isolation()
                verify.assert_not_called()
                self.assertNotIn("distributionDefaultInstanceInitialized", session.details)

    def test_security_result_contains_only_booleans_and_preserves_failed_checks(self):
        page = MagicMock()
        result = {str(index): True for index in range(9)}
        result["0"] = False
        page.evaluate.return_value = result
        with tempfile.TemporaryDirectory(prefix="nickel-security-result-") as temporary:
            observed, valid = security.security_checks(page, Path(temporary), "http://127.0.0.1/fixture")
            self.assertEqual(observed, result)
            self.assertTrue(valid.is_file())
            page.evaluate.return_value = dict(result, private="value-must-not-be-logged")
            with self.assertRaises(security.native.NativeAssertion):
                security.security_checks(page, Path(temporary), "http://127.0.0.1/fixture")



class DeviceCodeUiTests(unittest.TestCase):
    def flags(self, **updates):
        return dict(deviceCodeDisplayed=False, dialogPresent=True, failureDisplayed=False,
                    requestStillPending=True, transportFailureReported=False,
                    officialClientRejected=False, responseParsingFailed=False, **updates)

    def test_csp_safe_polling_waits_for_real_ui_status_without_dynamic_eval(self):
        page = MagicMock()
        pending = self.flags()
        accepted = dict(pending, deviceCodeDisplayed=True, requestStillPending=False)
        page.evaluate.side_effect = [pending, accepted]
        page.wait_for_function.side_effect = AssertionError("Dynamic eval must never be used")
        with patch.object(security.time, "monotonic", side_effect=[0, 1]):
            self.assertEqual(security.device_code_ui_checks(page), accepted)
        page.wait_for_timeout.assert_called_once_with(100)
        page.wait_for_function.assert_not_called()
        self.assertEqual(page.evaluate.call_count, 2)

    def test_error_and_timeout_keep_boolean_failure_evidence(self):
        for result in [dict(self.flags(), failureDisplayed=True, requestStillPending=False), self.flags()]:
            page = MagicMock()
            page.evaluate.return_value = result
            with self.subTest(result=result), patch.object(security.time, "monotonic", side_effect=[0, 46]):
                self.assertEqual(security.device_code_ui_checks(page), result)
            page.wait_for_timeout.assert_not_called()

    def test_private_or_invalid_result_shape_is_rejected(self):
        for result in [dict(self.flags(), private="withheld-fixture"), None, {str(i): "withheld-fixture" for i in range(7)}]:
            page = MagicMock()
            page.evaluate.return_value = result
            with self.subTest(result=result), patch.object(security.time, "monotonic", return_value=0):
                with self.assertRaises(security.native.NativeAssertion):
                    security.device_code_ui_checks(page)


if __name__ == "__main__":
    unittest.main()
