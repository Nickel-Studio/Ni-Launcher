"""Native media harness safety regression; never launches an application."""
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

import native_media as media


class MediaWaitTests(unittest.TestCase):
    def test_polling_avoids_dynamic_eval_under_production_csp(self):
        page = MagicMock()
        page.evaluate.side_effect = [False, True]
        page.wait_for_function.side_effect = AssertionError("Dynamic eval must not be used")
        with patch.object(media.time, "monotonic", side_effect=[0, 1, 2]):
            media.wait_for(page, "Boolean(document.querySelector('video'))")
        page.wait_for_function.assert_not_called()
        page.wait_for_timeout.assert_called_once_with(100)

    def test_timeout_does_not_report_success(self):
        with patch.object(media.time, "monotonic", side_effect=[0, 16]):
            with self.assertRaises(media.native.NativeAssertion):
                media.wait_for(MagicMock(), "true")

    def test_ready_keeps_modal_and_process_checks(self):
        session = SimpleNamespace(page=MagicMock(), assert_live=MagicMock())
        with patch.object(media, "wait_for") as wait, patch("playwright.sync_api.expect") as expect:
            media.ready(session)
        wait.assert_called_once()
        expect.return_value.to_have_count.assert_called_once_with(0)
        session.assert_live.assert_called_once_with()

    def test_result_evidence_rejects_private_values(self):
        for value in [None, {"passed": "private"}, {"passed": True, "extra": True}]:
            with self.subTest(value=value), self.assertRaises(media.native.NativeAssertion):
                media.flags(value, 1)
        self.assertEqual(media.flags({"passed": False}, 1), {"passed": False})

    def test_cli_process_is_protected(self):
        self.assertIn(media.os.getppid(), media.native.PROTECTED_PIDS)


if __name__ == "__main__":
    unittest.main()
