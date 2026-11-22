"""Launch a verified Linux portable archive under a private X display and data root.

Run through xvfb-run -a dbus-run-session. Verifies the real AppImage starts,
creates a visible owned window and selects adjacent portable data; not account login.
Only the newly created process group is stopped. The CLI process is never targeted.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import re
import signal
import struct
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
from package_portable import MARKER, MODPACK_MARKER, sha256, validate_archive


def require(condition):
    if not condition:
        raise ValueError("AppImage native startup acceptance failed; raw details withheld")


def group_members(pid):
    found = set()
    for candidate in Path("/proc").iterdir():
        if candidate.name.isdigit():
            try:
                if os.getpgid(int(candidate.name)) == pid:
                    found.add(int(candidate.name))
            except (OSError, ProcessLookupError):
                pass
    return found


def visible_windows(pid):
    members = group_members(pid)
    tree = subprocess.run(["xwininfo", "-root", "-tree"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    require(tree.returncode == 0)
    result = []
    identifiers = set(re.findall(rb"^\s+(0x[0-9a-f]+)\s", tree.stdout, re.MULTILINE))
    for identifier in sorted(identifiers):
        identifier = identifier.decode("ascii")
        if int(identifier, 16) == 0:
            continue
        owner = subprocess.run(["xprop", "-id", identifier, "_NET_WM_PID"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        values = re.findall(rb"=\s*(\d+)", owner.stdout)
        if not values or int(values[0]) not in members:
            continue
        state = subprocess.run(["xwininfo", "-id", identifier], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5)
        if state.returncode == 0 and b"IsViewable" in state.stdout:
            result.append({"window": identifier, "pid": int(values[0])})
    return result


class FileAccessObserver:
    """Watch private fixture reads without logging syscall paths or touching user data."""
    def __init__(self, files):
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.libc.inotify_init1.argtypes = [ctypes.c_int]
        self.libc.inotify_init1.restype = ctypes.c_int
        self.libc.inotify_add_watch.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_uint32]
        self.libc.inotify_add_watch.restype = ctypes.c_int
        self.fd = self.libc.inotify_init1(os.O_NONBLOCK | os.O_CLOEXEC)
        require(self.fd >= 0)
        self.names, self.accessed = {}, set()
        try:
            for name, path in files.items():
                watch = self.libc.inotify_add_watch(self.fd, os.fsencode(path), 0x00000001)
                require(watch >= 0)
                self.names[watch] = name
        except BaseException:
            self.close()
            raise

    def collect(self):
        while True:
            try:
                events = os.read(self.fd, 65536)
            except BlockingIOError:
                break
            offset = 0
            while offset + 16 <= len(events):
                watch, mask, _cookie, length = struct.unpack_from("iIII", events, offset)
                require(not mask & 0x00004000)  # An overflow cannot count as path-isolation evidence.
                if mask & 0x00000001 and watch in self.names:
                    self.accessed.add(self.names[watch])
                offset += 16 + length
        return set(self.accessed)

    def close(self):
        if self.fd >= 0:
            os.close(self.fd)
            self.fd = -1



def isolated_environment(root, config, parent=None):
    parent = os.environ if parent is None else parent
    environment = {key: value for key, value in parent.items()
                   if key in {"PATH", "DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS"}}
    # An inherited WSLg runtime socket can bypass the private Xvfb display.
    environment.update(APPIMAGE_EXTRACT_AND_RUN="1", GDK_BACKEND="x11")
    for name, directory in {"HOME": root / "home", "XDG_CONFIG_HOME": config,
                            "XDG_DATA_HOME": root / "user-data", "XDG_CACHE_HOME": root / "cache",
                            "TMPDIR": root / "temp", "XDG_RUNTIME_DIR": root / "runtime"}.items():
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        environment[name] = str(directory)
    return environment



def run(archive, expected, evidence):
    require(sys.platform == "linux" and os.environ.get("DISPLAY") and not evidence.exists())
    proof = json.loads((archive.parent / (archive.name.removesuffix(".tar.gz") + ".json")).read_text(encoding="utf-8"))
    report = validate_archive(archive, ROOT, proof["entries"])
    require(report["target"] == "x86_64-unknown-linux-gnu" and sha256(archive) == expected == proof["sha256"])
    report = {"actualPortableArchiveExecuted": False, "appimageNativeStartupPassed": False,
              "portableArchiveSha256": expected, "completedAccountLoginVerified": False,
              "packagedCspUiVerified": False, "cleanedOwnedProcessGroup": False}
    with tempfile.TemporaryDirectory(prefix="nickel-appimage-native-") as temporary:
        root = Path(temporary)
        extracted = root / "extracted"
        extracted.mkdir()
        with tarfile.open(archive, "r:gz") as stream:
            stream.extractall(extracted, filter="data")
        directories = list(extracted.iterdir())
        require(len(directories) == 1 and directories[0].is_dir())
        portable = directories[0]
        image = portable / "Nickel.AppImage"
        require(sha256(image) == proof["entries"]["Nickel.AppImage"]["sha256"] and image.stat().st_mode & 0o111)
        require((portable / MARKER).is_file() and not (portable / MODPACK_MARKER).exists())
        config = root / "system-config"
        system = config / "studio.nickel.launcher/settings.json"
        system.parent.mkdir(parents=True)
        system.write_text('{"accent":"rose","systemSentinel":true}', encoding="utf-8")
        sentinel = sha256(system)
        data = portable / "Nickel"
        adjacent = data / "settings.json"
        adjacent.write_text('{"accountType":"offline","offlineUsername":"Acceptance","themeMode":"dark","accent":"nickel","portableStartupFixture":true}', encoding="utf-8")
        environment = isolated_environment(root, config)
        observer = FileAccessObserver({"adjacent": adjacent, "system": system})
        output = tempfile.TemporaryFile()
        phase = "ownedProcessStartup"
        process = None
        try:
            process = subprocess.Popen([str(image)], cwd=portable, env=environment, stdout=output, stderr=output, start_new_session=True)
            report.update(actualPortableArchiveExecuted=True, launchedOwnedPid=process.pid, privateX11RuntimeSelected=True)
            phase = "ownedWindowAndPortableSettings"
            deadline = time.monotonic() + 75
            windows, accessed = [], set()
            while time.monotonic() < deadline:
                require(process.poll() is None)
                windows = visible_windows(process.pid)
                accessed = observer.collect()
                if windows and "adjacent" in accessed:
                    break
                time.sleep(0.25)
            report.update(ownedVisibleWindows=windows, adjacentPortableSettingsRead="adjacent" in accessed,
                          privateSystemSettingsRead="system" in accessed)
            require(windows and "adjacent" in accessed and "system" not in accessed)
            phase = "stableWindowAndPortableData"
            time.sleep(2)
            require(process.poll() is None and visible_windows(process.pid))
            accessed = observer.collect()
            require("system" not in accessed and sha256(system) == sentinel)
            # Reading onboarding_get does not create onboarding.json before consent is saved.
            require(data.is_dir() and not (data / "portable-import.json").exists() and not (data / "pack.mrpack").exists())
            report.update(appimageNativeStartupPassed=True, adjacentPortableDataSelected=True,
                          privateSystemConfigurationUnchanged=True, noModpackBootstrap=True,
                          appimageSha256=sha256(image))
        except Exception:
            report.update(appimageNativeStartupPassed=False, failureStage=phase)
        finally:
            observer.close()
            if process is not None:
                report["observedExitCodeBeforeCleanup"] = process.poll()
                require(process.pid not in (os.getpid(), os.getppid()))
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=10)
                deadline = time.monotonic() + 10
                while group_members(process.pid) and time.monotonic() < deadline:
                    time.sleep(0.1)
                report["cleanedOwnedProcessGroup"] = not group_members(process.pid)
            if not report["cleanedOwnedProcessGroup"]:
                report["appimageNativeStartupPassed"] = False
            output.seek(0)
            log = output.read(1024 * 1024)
            report["missingSharedLibraryReported"] = b"error while loading shared libraries" in log
            report["webProcessCrashReported"] = any(value in log for value in [b"segmentation fault", b"WebProcess CRASHED", b"Could not create EGL"])
            output.close()
    evidence.parent.mkdir(parents=True, exist_ok=True)
    with evidence.open("x", encoding="utf-8") as stream:
        json.dump(report, stream, indent=2)
        stream.write("\n")
    print(json.dumps(report))
    return 0 if report["appimageNativeStartupPassed"] else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--evidence", required=True, type=Path)
    args = parser.parse_args()
    return run(args.archive.absolute(), args.expected_sha256, args.evidence.absolute())


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception:
        print(json.dumps({"appimageNativeStartupPassed": False, "privateValuesWithheld": True}))
        raise SystemExit(1)
