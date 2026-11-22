"""Linux private-file observer regression; no native application is launched."""
import os
from pathlib import Path
import sys
import tempfile
import unittest

from unittest.mock import patch

from appimage_startup import FileAccessObserver, isolated_environment, visible_windows


class X11IsolationTests(unittest.TestCase):
    def test_display_is_private_and_wslg_runtime_is_not_inherited(self):
        parent = {"PATH": "/usr/bin:/bin", "DISPLAY": ":99", "XAUTHORITY": "/synthetic/xauth",
                  "DBUS_SESSION_BUS_ADDRESS": "unix:path=/synthetic/bus",
                  "XDG_RUNTIME_DIR": "/mnt/wslg/runtime-dir", "WAYLAND_DISPLAY": "wayland-0",
                  "NICKEL_SECRETS": "/synthetic/private.json", "WEBKIT_DISABLE_DMABUF_RENDERER": "1"}
        with tempfile.TemporaryDirectory(prefix="nickel-x11-env-test-") as temporary:
            root = Path(temporary)
            config = root / "config"
            environment = isolated_environment(root, config, parent)
            self.assertEqual(environment["DISPLAY"], parent["DISPLAY"])
            self.assertEqual(environment["GDK_BACKEND"], "x11")
            self.assertEqual(environment["XDG_RUNTIME_DIR"], str(root / "runtime"))
            self.assertNotIn("WAYLAND_DISPLAY", environment)
            self.assertNotIn("NICKEL_SECRETS", environment)
            self.assertNotIn("WEBKIT_DISABLE_DMABUF_RENDERER", environment)
            for name in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "TMPDIR", "XDG_RUNTIME_DIR"]:
                self.assertTrue(Path(environment[name]).is_relative_to(root))
                self.assertTrue(Path(environment[name]).is_dir())
            if sys.platform == "linux":
                self.assertEqual((root / "runtime").stat().st_mode & 0o777, 0o700)

    def test_only_nonzero_owned_viewable_window_ids_are_queried(self):
        from types import SimpleNamespace
        tree = b"Root window id: 0x111 (root)\n Parent window id: 0x0 (none)\n  0x2a (app)\n  0x2a (duplicate)\n  0x0 (none)\n  0x2b (other)\n"
        queries = []

        def command(arguments, **_options):
            queries.append(arguments)
            if arguments == ["xwininfo", "-root", "-tree"]:
                return SimpleNamespace(returncode=0, stdout=tree)
            if arguments[0] == "xprop":
                return SimpleNamespace(returncode=0, stdout=b"_NET_WM_PID(CARDINAL) = " +
                                       (b"100" if arguments[2] == "0x2a" else b"200"))
            return SimpleNamespace(returncode=0, stdout=b"Map State: IsViewable")

        with patch("appimage_startup.group_members", return_value={100}), \
                patch("appimage_startup.subprocess.run", side_effect=command):
            self.assertEqual(visible_windows(100), [{"window": "0x2a", "pid": 100}])
        self.assertFalse(any("0x0" in arguments or "0x111" in arguments for arguments in queries))
        self.assertEqual(sum(arguments[:3] == ["xprop", "-id", "0x2a"] for arguments in queries), 1)


@unittest.skipUnless(sys.platform == "linux", "Linux inotify is required")
class FileAccessObserverTests(unittest.TestCase):
    def test_only_actual_reads_of_each_private_fixture_are_recorded(self):
        with tempfile.TemporaryDirectory(prefix="nickel-inotify-test-") as temporary:
            adjacent = Path(temporary) / "adjacent.json"
            system = Path(temporary) / "system.json"
            adjacent.write_bytes(b"synthetic-adjacent")
            system.write_bytes(b"synthetic-system")
            observer = FileAccessObserver({"adjacent": adjacent, "system": system})
            try:
                self.assertEqual(observer.collect(), set())
                self.assertEqual(adjacent.read_bytes(), b"synthetic-adjacent")
                self.assertEqual(observer.collect(), {"adjacent"})
                self.assertEqual(system.read_bytes(), b"synthetic-system")
                self.assertEqual(observer.collect(), {"adjacent", "system"})
                self.assertFalse(os.get_inheritable(observer.fd))
            finally:
                observer.close()
                observer.close()
            self.assertEqual(observer.fd, -1)

    def test_missing_fixture_fails_without_leaking_a_descriptor(self):
        before = len(list(Path("/proc/self/fd").iterdir()))
        with tempfile.TemporaryDirectory(prefix="nickel-inotify-test-") as temporary:
            with self.assertRaises(ValueError):
                FileAccessObserver({"absent": Path(temporary) / "missing.json"})
        self.assertEqual(len(list(Path("/proc/self/fd").iterdir())), before)


if __name__ == "__main__":
    unittest.main()
