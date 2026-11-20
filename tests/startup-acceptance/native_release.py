"""Prepare or test an isolated copy of a release EXE; preparation never launches it.

Run --help for usage. Actual window tests require an isolated Windows desktop and
Python Playwright. This is filesystem isolation, not an operating-system sandbox.
No source-adjacent configuration is copied, no startup arguments are supplied,
and the synthetic portable import is already complete to avoid game downloads.
"""
import argparse
import ctypes
import hashlib
import json
import ntpath
import os
import re
import shutil
import socket
import stat
import struct
import subprocess
import sys
import threading
import time
import traceback
import unicodedata
import uuid
from ctypes import wintypes
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, build_opener

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
EVIDENCE_ROOT = HERE / "evidence"
REQUIRED_VERSION = "1.0.2"
EULA_TITLE = "\u5f00\u59cb\u4e4b\u524d\uff0c\u8bf7\u786e\u8ba4 Minecraft EULA"
ACCEPT = "\u540c\u610f\u5e76\u7b7e\u7f72"
REJECT = "\u62d2\u7edd"
ACK = "\u6211\u5df2\u77e5\u6653"
EXIT_TITLE = "\u65e0\u6cd5\u7ee7\u7eed\u63d0\u4f9b\u670d\u52a1"
EXIT_PREFIX = "\u6b63\u5728\u9000\u51fa..."
SETTINGS = "\u8bbe\u7f6e"
CONSENT_FIELDS = {"eulaVersion", "eulaAcceptedAt", "changelogSeenVersion"}
MICROSOFT_FIELDS = {"clientId", "authority", "redirectUri"}
SYNTHETIC_SETTINGS = {"accountType": "offline", "offlineUsername": "Acceptance", "themeMode": "dark", "accent": "nickel", "titlebarTint": True}
PROTECTED_PIDS = {17584, 49808, os.getpid()}


class SafetyError(ValueError):
    pass


class NativeAssertion(AssertionError):
    pass


def safe_failure(error):
    # Exception text can include private IPC values, even for harness assertions.
    kind = "SafetyError" if isinstance(error, SafetyError) else "NativeAssertion" if isinstance(error, NativeAssertion) else "NativeError"
    return {
        "errorType": kind,
        "error": "Native operation failed; raw exception and IPC details are withheld",
        "frames": [{"file": Path(frame.f_code.co_filename).name, "line": line, "function": frame.f_code.co_name} for frame, line in traceback.walk_tb(error.__traceback__)],
    }


def check(condition, message):
    if not condition:
        raise NativeAssertion(message)


def local_path(path):
    text = os.fspath(path)
    if not isinstance(text, str) or not text or any(ord(char) < 32 for char in text):
        raise SafetyError("A non-empty local filesystem path is required")
    if os.name == "nt":
        if text.startswith("\\\\?\\") and re.match(r"^\\\\\?\\[A-Za-z]:\\", text):
            text = text[4:]
        drive, tail = ntpath.splitdrive(text)
        if text.startswith("\\\\") or ":" in tail or (drive and not re.fullmatch(r"[A-Za-z]:", drive)):
            raise SafetyError("Network, device and alternate-stream paths are not allowed")
        if drive and not tail.startswith(("\\", "/")):
            raise SafetyError("Drive-relative paths are not allowed")
    return Path(os.path.abspath(text))


def same_local_path(actual, expected):
    return os.path.normcase(str(local_path(actual))) == os.path.normcase(str(local_path(expected)))


def reject_links(path):
    current = local_path(path)
    for candidate in [current, *current.parents]:
        try:
            info = candidate.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise SafetyError("Symlinks and reparse points are not allowed")
        if stat.S_ISREG(info.st_mode) and getattr(info, "st_nlink", 1) != 1:
            raise SafetyError("Hard-linked files are not allowed")
    return current


def evidence_path(path, new=False):
    candidate = reject_links(path).resolve()
    base = reject_links(EVIDENCE_ROOT).resolve()
    if candidate == base or not candidate.is_relative_to(base):
        raise SafetyError("Native output must stay inside tests/startup-acceptance/evidence")
    if new and candidate.exists():
        raise SafetyError("Refusing to reuse existing native output: " + str(candidate))
    return candidate


def write_json(path, value):
    path = evidence_path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("x", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=True, indent=2)
        stream.write("\n")


def file_hash(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def validated_sha256(value, required=False):
    if value is None and not required:
        return None
    if not isinstance(value, str) or not re.fullmatch(r"[a-fA-F0-9]{64}", value):
        raise SafetyError("--run requires an explicit 64-character --expected-sha256")
    return value.lower()


def validate_protocol(protocol):
    names = [protocol.marker, protocol.data_directory, protocol.game_directory, protocol.payload, protocol.import_record, protocol.launcher]
    for name in names:
        if not isinstance(name, str) or name in {"", ".", ".."} or re.search(r'[\\/:<>"|?*\x00-\x1f]', name) or name.endswith((".", " ")):
            raise SafetyError("Unexpected portable path component in current backend protocol")
        if re.fullmatch(r"(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?", name, re.IGNORECASE):
            raise SafetyError("Reserved portable path components are not allowed")
    if len({name.casefold() for name in names}) != len(names) or protocol.launcher.lower().endswith(".exe") is False:
        raise SafetyError("Portable path components must be distinct and include an EXE")
    if type(protocol.schema) is not int or protocol.schema < 1 or protocol.version != REQUIRED_VERSION:
        raise SafetyError("Unexpected portable schema or release version")
    if not isinstance(protocol.eula_revision, str) or not protocol.eula_revision.strip():
        raise SafetyError("The current native EULA revision is required")
    return protocol


@dataclass(frozen=True)
class Protocol:
    marker: str
    format: str
    schema: int
    data_directory: str
    game_directory: str
    payload: str
    import_record: str
    launcher: str
    eula_revision: str
    version: str


def load_protocol():
    sources = {
        "pack": (ROOT / "src-tauri/src/portable_pack.rs").read_text(encoding="utf-8"),
        "paths": (ROOT / "src-tauri/src/app_paths.rs").read_text(encoding="utf-8"),
        "bootstrap": (ROOT / "src-tauri/src/portable_bootstrap.rs").read_text(encoding="utf-8"),
        "onboarding": (ROOT / "src-tauri/src/onboarding.rs").read_text(encoding="utf-8"),
    }

    def string(group, name):
        matches = re.findall(r"^\s*(?:pub\s+)?const\s+" + re.escape(name) + r'\s*:\s*&str\s*=\s*"([^"\r\n]+)"\s*;', sources[group], re.MULTILINE)
        if len(matches) != 1:
            raise SafetyError("Cannot derive current backend constant: " + name)
        return matches[0]

    schema = re.findall(r"^\s*pub\s+const\s+SCHEMA_VERSION\s*:\s*u32\s*=\s*(\d+)\s*;", sources["pack"], re.MULTILINE)
    version = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))["version"]
    if len(schema) != 1 or version != REQUIRED_VERSION:
        raise SafetyError("Backend portable schema or release version does not match this acceptance suite")
    protocol = Protocol(
        string("pack", "MANIFEST_FILE"), string("pack", "FORMAT"), int(schema[0]),
        string("paths", "DATA_DIRECTORY"), string("paths", "GAME_DIRECTORY"),
        string("pack", "PAYLOAD_FILE"), string("bootstrap", "IMPORT_RECORD"),
        string("pack", "WINDOWS_LAUNCHER"), string("onboarding", "EULA_VERSION"), version,
    )
    return validate_protocol(protocol)


def executable_source(path):
    source = reject_links(path).resolve(strict=True)
    if source.suffix.lower() != ".exe" or not source.is_file():
        raise SafetyError("--exe must refer to a regular Windows EXE file")
    with source.open("rb") as stream:
        header = stream.read(64)
        if len(header) < 64 or header[:2] != b"MZ":
            raise SafetyError("Source is not a PE executable")
        offset = struct.unpack_from("<I", header, 60)[0]
        if offset < 64 or offset > source.stat().st_size - 4:
            raise SafetyError("Invalid PE header offset")
        stream.seek(offset)
        if stream.read(4) != b"PE\x00\x00":
            raise SafetyError("Source has no PE signature")
    return source


@dataclass
class Sandbox:
    root: Path
    exe: Path
    record: Path
    source: Path
    sha256: str
    protocol: Protocol
    expected_sha256: str | None = None

    def record_path(self):
        root = evidence_path(self.root)
        path = evidence_path(self.record)
        if path != root / self.protocol.data_directory / "onboarding.json":
            raise SafetyError("Onboarding records must belong to the copied release")
        return path

    def seed_record(self, value):
        validate_record(value)
        write_json(self.record_path(), value)

    def local_record(self):
        path = self.record_path()
        if not path.exists():
            return {}
        if not path.is_file() or path.stat().st_size > 16 * 1024:
            raise SafetyError("Unexpected oversized or invalid onboarding record")
        value = json.loads(path.read_text(encoding="utf-8"))
        validate_record(value)
        return value


def validate_record(value):
    if not isinstance(value, dict) or not set(value).issubset(CONSENT_FIELDS):
        raise SafetyError("Records may contain only local onboarding fields")
    for name in ["eulaVersion", "changelogSeenVersion"]:
        item = value.get(name)
        if item is not None and (not isinstance(item, str) or not item or len(item) > 512 or any(unicodedata.category(char) == "Cc" for char in item)):
            raise SafetyError("Invalid onboarding revision or version")
    timestamp = value.get("eulaAcceptedAt")
    if timestamp is not None and (type(timestamp) is not int or not 0 < timestamp <= 2 ** 53 - 1):
        raise SafetyError("Invalid local onboarding signature time")


def prepare_copy(source, directory, protocol=None, expected_sha256=None):
    expected_sha256 = validated_sha256(expected_sha256)
    source = executable_source(source)
    directory = evidence_path(directory, new=True)
    protocol = validate_protocol(protocol or load_protocol())
    before = file_hash(source)
    if expected_sha256 is not None and before != expected_sha256:
        raise SafetyError("Source EXE does not match --expected-sha256")
    directory.mkdir(parents=True, exist_ok=False)
    copied = evidence_path(directory / protocol.launcher, new=True)
    # Copy only the specified binary, never its containing directory or user files.
    with source.open("rb") as incoming, copied.open("xb") as outgoing:
        shutil.copyfileobj(incoming, outgoing, 1024 * 1024)
    if file_hash(copied) != before or file_hash(source) != before:
        raise SafetyError("Source EXE changed while the isolated copy was being prepared")
    package_id = uuid.uuid4().hex
    data = directory / protocol.data_directory
    data.mkdir()
    (directory / protocol.game_directory).mkdir()
    write_json(directory / protocol.marker, {
        "schemaVersion": protocol.schema, "format": protocol.format, "packageId": package_id,
        "name": "Startup acceptance fixture", "version": protocol.version,
        "instanceName": "startup-acceptance-fixture",
        "payload": protocol.data_directory + "/" + protocol.payload,
        "gameDirectory": protocol.game_directory, "dataDirectory": protocol.data_directory,
    })
    # The backend recognizes this record before resolving any portable payload.
    write_json(data / protocol.import_record, {"packageId": package_id, "versionId": "startup-acceptance-fixture"})
    write_json(data / "settings.json", SYNTHETIC_SETTINGS)
    sandbox = Sandbox(directory, copied, data / "onboarding.json", source, before, protocol, expected_sha256)
    write_json(directory / "preparation.json", {
        "workspace": str(ROOT), "sourceExe": str(source), "copiedExe": str(copied),
        "sha256": before, "explicitHashAuthorized": expected_sha256 is not None,
        "nativeExecuted": False, "nativeAcceptance": False,
        "configurationDirectory": str(data), "markerFile": str(directory / protocol.marker),
        "portableImport": "synthetic already-installed fixture; no payload download",
        "copiedSourceFiles": [str(source)], "startupArguments": [],
        "versionCheck": "Native onboarding_get must report " + REQUIRED_VERSION + " during --run",
        "realAuthenticationExercised": False, "microsoftConfigurationCompared": False,
    })
    return sandbox


def assert_private_tree(directory):
    root = evidence_path(directory)
    real_root = Path(os.path.realpath(root))
    for current, directories, files in os.walk(root, followlinks=False):
        evidence_path(current)
        for name in [*directories, *files]:
            path = Path(current) / name
            info = path.lstat()
            linked = stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400
            if linked:
                # Windows 会在隔离配置目录的 INetCache 下创建内部缓存联接；
                # 只有该固定位置、目标仍位于本次输出内的目录联接可以保留，且不进入其内部。
                internal_cache = stat.S_ISDIR(info.st_mode) and path.parent.name.casefold() == "inetcache"
                if not internal_cache or not Path(os.path.realpath(path)).is_relative_to(real_root):
                    raise SafetyError("Only links that stay inside the task-owned native output are allowed")
                if name in directories:
                    directories.remove(name)
                continue
            value = evidence_path(path)
            if name.casefold() == "secrets.local.json":
                raise SafetyError("External API configuration must not exist in native output")
            if not value.is_dir() and not value.is_file():
                raise SafetyError("Only regular task-owned files and directories are allowed")


def assert_copy(sandbox, launching=False):
    validate_protocol(sandbox.protocol)
    root = evidence_path(sandbox.root)
    exe = evidence_path(sandbox.exe)
    sandbox.record_path()
    if exe == sandbox.source.resolve() or exe.parent != root or exe.name != sandbox.protocol.launcher:
        raise SafetyError("Only the task-owned sandbox EXE may be launched")
    if launching and validated_sha256(sandbox.expected_sha256, required=True) != sandbox.sha256:
        raise SafetyError("A task-owned EXE must have an explicitly authorized hash before launch")
    if not exe.is_file() or file_hash(exe) != sandbox.sha256:
        raise SafetyError("Sandbox EXE is missing or changed")
    assert_private_tree(root)
    marker_path = evidence_path(root / sandbox.protocol.marker)
    import_path = evidence_path(root / sandbox.protocol.data_directory / sandbox.protocol.import_record)
    for path in [marker_path, import_path]:
        if not path.is_file() or path.stat().st_size > 64 * 1024:
            raise SafetyError("Sandbox portable guard is missing or invalid")
    marker = json.loads(marker_path.read_text(encoding="utf-8"))
    package_id = marker.get("packageId") if isinstance(marker, dict) else None
    expected_marker = {
        "schemaVersion": sandbox.protocol.schema, "format": sandbox.protocol.format,
        "packageId": package_id, "name": "Startup acceptance fixture",
        "version": sandbox.protocol.version, "instanceName": "startup-acceptance-fixture",
        "payload": sandbox.protocol.data_directory + "/" + sandbox.protocol.payload,
        "gameDirectory": sandbox.protocol.game_directory, "dataDirectory": sandbox.protocol.data_directory,
    }
    if not isinstance(package_id, str) or not re.fullmatch(r"[a-f0-9]{32}", package_id) or marker != expected_marker:
        raise SafetyError("Sandbox portable-root marker changed")
    for directory in [root / sandbox.protocol.data_directory, root / sandbox.protocol.game_directory]:
        if not evidence_path(directory).is_dir():
            raise SafetyError("Sandbox data or game directory is invalid")
    if evidence_path(root / sandbox.protocol.data_directory / sandbox.protocol.payload).exists():
        raise SafetyError("Native startup acceptance must not have a portable payload")
    imported = json.loads(import_path.read_text(encoding="utf-8"))
    if imported != {"packageId": package_id, "versionId": "startup-acceptance-fixture"}:
        raise SafetyError("Sandbox import-completed guard changed")


def private_environment(directory, parent=None):
    parent = os.environ if parent is None else parent
    allowed = {"PATH", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT", "PROGRAMFILES", "PROGRAMFILES(X86)", "PROGRAMW6432", "SYSTEMDRIVE", "PROCESSOR_ARCHITECTURE", "PROCESSOR_IDENTIFIER", "NUMBER_OF_PROCESSORS", "OS"}
    environment = {key.upper(): value for key, value in parent.items() if key.upper() in allowed}
    directory = evidence_path(directory)
    home = directory / "profile"
    folders = {
        "USERPROFILE": home, "HOME": home,
        "APPDATA": home / "AppData/Roaming", "LOCALAPPDATA": home / "AppData/Local",
        "TEMP": directory / "temp", "TMP": directory / "temp",
        "WEBVIEW2_USER_DATA_FOLDER": directory / "webview2",
    }
    for key, folder in folders.items():
        folder = evidence_path(folder)
        folder.mkdir(parents=True, exist_ok=True)
        if not folder.is_dir():
            raise SafetyError("Private profile and temporary paths must be directories")
        environment[key] = str(folder)
    environment["HOMEDRIVE"], environment["HOMEPATH"] = ntpath.splitdrive(str(home))
    return environment


def isolated_environment(sandbox, session_directory, port, parent=None):
    if type(port) is not int or not 1024 <= port <= 65535:
        raise SafetyError("A non-privileged loopback CDP port is required")
    environment = private_environment(session_directory, parent)
    environment["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = " ".join([
        "--remote-debugging-port=" + str(port), "--remote-debugging-address=127.0.0.1",
        "--enable-automation", "--disable-background-networking", "--no-first-run",
    ])
    return environment


class PrivateControllerEnvironment:
    """Keep the Playwright driver private and prevent inherited protocol logging."""
    def __init__(self, directory):
        self.directory = evidence_path(directory, new=True)
        self.saved = None

    def __enter__(self):
        environment = private_environment(self.directory)
        self.saved = dict(os.environ)
        os.environ.clear()
        os.environ.update(environment)
        return self

    def __exit__(self, *_):
        if self.saved is not None:
            os.environ.clear()
            os.environ.update(self.saved)
            self.saved = None


def windows_command_arguments(command):
    if os.name != "nt" or not isinstance(command, str) or not command or len(command) > 65535 or "\0" in command:
        raise SafetyError("Cannot parse an observed Windows process command line")
    shell = ctypes.WinDLL("shell32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    shell.CommandLineToArgvW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(ctypes.c_int)]
    shell.CommandLineToArgvW.restype = ctypes.POINTER(wintypes.LPWSTR)
    kernel.LocalFree.argtypes, kernel.LocalFree.restype = [wintypes.HANDLE], wintypes.HANDLE
    count = ctypes.c_int()
    values = shell.CommandLineToArgvW(command, ctypes.byref(count))
    if not values:
        raise SafetyError("Cannot parse task-owned process arguments")
    try:
        if not 0 < count.value <= 16384:
            raise SafetyError("Unexpected task-owned process argument count")
        return [values[index] for index in range(count.value)]
    finally:
        if kernel.LocalFree(ctypes.cast(values, wintypes.HANDLE)):
            raise SafetyError("Cannot release the task-owned command-line observation")


class JobBasicLimits(ctypes.Structure):
    _fields_ = [("process_time", ctypes.c_longlong), ("job_time", ctypes.c_longlong), ("flags", wintypes.DWORD), ("minimum_working_set", ctypes.c_size_t), ("maximum_working_set", ctypes.c_size_t), ("active_process_limit", wintypes.DWORD), ("affinity", ctypes.c_size_t), ("priority", wintypes.DWORD), ("scheduling", wintypes.DWORD)]


class JobIoCounters(ctypes.Structure):
    _fields_ = [(name, ctypes.c_ulonglong) for name in ["read_operations", "write_operations", "other_operations", "read_bytes", "write_bytes", "other_bytes"]]


class JobExtendedLimits(ctypes.Structure):
    _fields_ = [("basic", JobBasicLimits), ("io", JobIoCounters), ("process_memory", ctypes.c_size_t), ("job_memory", ctypes.c_size_t), ("peak_process_memory", ctypes.c_size_t), ("peak_job_memory", ctypes.c_size_t)]


class WindowsJob:
    """Own only suspended children created by this harness; closing kills that job."""
    def __init__(self):
        if os.name != "nt":
            raise SafetyError("Native process containment requires Windows")
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        for name, arguments, result in [
            ("CreateJobObjectW", [ctypes.c_void_p, wintypes.LPCWSTR], wintypes.HANDLE),
            ("SetInformationJobObject", [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD], wintypes.BOOL),
            ("AssignProcessToJobObject", [wintypes.HANDLE, wintypes.HANDLE], wintypes.BOOL),
            ("QueryInformationJobObject", [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.DWORD, ctypes.c_void_p], wintypes.BOOL),
            ("TerminateJobObject", [wintypes.HANDLE, wintypes.UINT], wintypes.BOOL),
            ("IsProcessInJob", [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)], wintypes.BOOL),
            ("OpenProcess", [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD], wintypes.HANDLE),
            ("CloseHandle", [wintypes.HANDLE], wintypes.BOOL),
        ]:
            function = getattr(self.kernel, name)
            function.argtypes, function.restype = arguments, result
        self.handle = self.kernel.CreateJobObjectW(None, None)
        if not self.handle:
            self.handle = None
            raise SafetyError("Cannot create a private native job")
        limits = JobExtendedLimits()
        limits.basic.flags = 0x2000
        if not self.kernel.SetInformationJobObject(self.handle, 9, ctypes.byref(limits), ctypes.sizeof(limits)):
            self.close()
            raise SafetyError("Cannot enable task-owned job cleanup")

    def assign(self, process):
        if not process.created or type(process.pid) is not int or process.pid <= 0 or process.pid in PROTECTED_PIDS or self.handle is None:
            raise SafetyError("Only a newly created task-owned process handle may enter the job")
        if not self.kernel.AssignProcessToJobObject(self.handle, process.handle):
            raise SafetyError("Cannot contain the suspended native release; it will not be resumed")

    def member_pids(self):
        if self.handle is None:
            return []
        for capacity in [32, 128, 512, 2048]:
            class ProcessList(ctypes.Structure):
                _fields_ = [("assigned", wintypes.DWORD), ("count", wintypes.DWORD), ("pids", ctypes.c_size_t * capacity)]
            value = ProcessList()
            if self.kernel.QueryInformationJobObject(self.handle, 3, ctypes.byref(value), ctypes.sizeof(value), None):
                if value.count <= capacity:
                    return [int(value.pids[index]) for index in range(value.count)]
            elif ctypes.get_last_error() != 234:
                raise SafetyError("Cannot inspect task-owned native processes")
        raise SafetyError("Unexpected task-owned process count")

    def contains_pid(self, pid):
        if type(pid) is not int or pid <= 0 or pid in PROTECTED_PIDS or self.handle is None:
            return False
        process = self.kernel.OpenProcess(0x1000, False, pid)
        if not process:
            return False
        try:
            member = wintypes.BOOL()
            return bool(self.kernel.IsProcessInJob(process, self.handle, ctypes.byref(member)) and member.value)
        finally:
            if not self.kernel.CloseHandle(process):
                raise SafetyError("Cannot close a read-only native ownership probe")


    def process_arguments(self, pid):
        if type(pid) is not int or pid <= 0 or pid in PROTECTED_PIDS or self.handle is None:
            raise SafetyError("Only task-owned listener arguments may be inspected")
        process = self.kernel.OpenProcess(0x1000, False, pid)
        if not process:
            raise SafetyError("Cannot open a read-only task-owned listener probe")
        try:
            member = wintypes.BOOL()
            if not self.kernel.IsProcessInJob(process, self.handle, ctypes.byref(member)) or not member.value:
                raise SafetyError("Refusing to inspect an unowned listener command line")
            library = ctypes.WinDLL("ntdll", use_last_error=True)
            query = library.NtQueryInformationProcess
            query.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p, wintypes.ULONG, ctypes.POINTER(wintypes.ULONG)]
            query.restype = ctypes.c_long

            class UnicodeString(ctypes.Structure):
                _fields_ = [("length", wintypes.USHORT), ("maximum", wintypes.USHORT), ("buffer", ctypes.c_void_p)]

            size = wintypes.ULONG()
            status = query(process, 60, None, 0, ctypes.byref(size))
            if status & 0xFFFFFFFF not in {0, 0xC0000004, 0xC0000023, 0x80000005}:
                raise SafetyError("Task-owned listener command-line inspection is unavailable")
            for _ in range(4):
                if not ctypes.sizeof(UnicodeString) <= size.value <= 128 * 1024:
                    raise SafetyError("Invalid task-owned listener command-line buffer")
                document = ctypes.create_string_buffer(size.value)
                status = query(process, 60, document, len(document), ctypes.byref(size))
                if status == 0:
                    break
                if status & 0xFFFFFFFF not in {0xC0000004, 0xC0000023, 0x80000005}:
                    raise SafetyError("Cannot inspect the task-owned listener command line")
            else:
                raise SafetyError("Task-owned listener command line changed repeatedly")
            value = UnicodeString.from_buffer(document)
            start = ctypes.addressof(document) + ctypes.sizeof(UnicodeString)
            end = ctypes.addressof(document) + len(document)
            if not value.buffer or value.length == 0 or value.length % 2 or value.maximum < value.length or not start <= value.buffer <= end - value.length:
                raise SafetyError("Invalid task-owned listener command-line descriptor")
            # The query returns a local UNICODE_STRING, never a remote pointer to dereference.
            command = ctypes.wstring_at(value.buffer, value.length // 2)
            return windows_command_arguments(command)
        finally:
            if not self.kernel.CloseHandle(process):
                raise SafetyError("Cannot close the read-only listener command-line probe")

    def close(self):
        if self.handle is None:
            return
        members = self.member_pids()
        if any(pid in PROTECTED_PIDS for pid in members):
            raise SafetyError("Refusing to close a job containing a protected controller process")
        if members:
            # Wait for every job member, not just the main launcher process.
            if not self.kernel.TerminateJobObject(self.handle, 1):
                raise SafetyError("Cannot stop task-owned job descendants")
            deadline = time.perf_counter() + 10
            while self.member_pids():
                if time.perf_counter() >= deadline:
                    raise SafetyError("Task-owned job descendants survived cleanup")
                time.sleep(0.02)
        if not self.kernel.CloseHandle(self.handle):
            raise SafetyError("Cannot close the task-owned job handle")
        self.handle = None


class ProcessStartupInfo(ctypes.Structure):
    _fields_ = [("size", wintypes.DWORD), ("reserved", wintypes.LPWSTR), ("desktop", wintypes.LPWSTR), ("title", wintypes.LPWSTR), ("x", wintypes.DWORD), ("y", wintypes.DWORD), ("width", wintypes.DWORD), ("height", wintypes.DWORD), ("x_count", wintypes.DWORD), ("y_count", wintypes.DWORD), ("fill", wintypes.DWORD), ("flags", wintypes.DWORD), ("show", wintypes.WORD), ("reserved_size", wintypes.WORD), ("reserved_bytes", ctypes.c_void_p), ("input", wintypes.HANDLE), ("output", wintypes.HANDLE), ("error", wintypes.HANDLE)]


class ProcessInformation(ctypes.Structure):
    _fields_ = [("process", wintypes.HANDLE), ("thread", wintypes.HANDLE), ("pid", wintypes.DWORD), ("thread_id", wintypes.DWORD)]


class OwnedNativeProcess:
    """Create suspended, verify the image, contain it, then resume its primary thread."""
    def __init__(self, sandbox, environment, job):
        assert_copy(sandbox, launching=True)
        if os.name != "nt":
            raise SafetyError("Native process creation requires Windows")
        self.created, self.resumed = False, False
        self.handle = self.thread = self.pid = None
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        for name, arguments, result in [
            ("CreateProcessW", [wintypes.LPCWSTR, wintypes.LPWSTR, ctypes.c_void_p, ctypes.c_void_p, wintypes.BOOL, wintypes.DWORD, ctypes.c_void_p, wintypes.LPCWSTR, ctypes.POINTER(ProcessStartupInfo), ctypes.POINTER(ProcessInformation)], wintypes.BOOL),
            ("QueryFullProcessImageNameW", [wintypes.HANDLE, wintypes.DWORD, wintypes.LPWSTR, ctypes.POINTER(wintypes.DWORD)], wintypes.BOOL),
            ("ResumeThread", [wintypes.HANDLE], wintypes.DWORD),
            ("WaitForSingleObject", [wintypes.HANDLE, wintypes.DWORD], wintypes.DWORD),
            ("GetExitCodeProcess", [wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD)], wintypes.BOOL),
            ("TerminateProcess", [wintypes.HANDLE, wintypes.UINT], wintypes.BOOL),
            ("CloseHandle", [wintypes.HANDLE], wintypes.BOOL),
        ]:
            function = getattr(self.kernel, name)
            function.argtypes, function.restype = arguments, result
        startup, information = ProcessStartupInfo(), ProcessInformation()
        startup.size = ctypes.sizeof(startup)
        command = ctypes.create_unicode_buffer(subprocess.list2cmdline([str(sandbox.exe)]))
        block = ctypes.create_unicode_buffer("\0".join(key + "=" + environment[key] for key in sorted(environment)) + "\0\0")
        try:
            with WindowsExecutableLock(sandbox.exe):
                assert_copy(sandbox, launching=True)
                if not self.kernel.CreateProcessW(str(sandbox.exe), command, None, None, False, 0x08000404, block, str(sandbox.root), ctypes.byref(startup), ctypes.byref(information)):
                    raise SafetyError("Cannot create the copied native release")
                self.created, self.handle, self.thread, self.pid = True, information.process, information.thread, int(information.pid)
                image, size = ctypes.create_unicode_buffer(32768), wintypes.DWORD(32768)
                if not self.handle or not self.thread or self.pid <= 0 or self.pid in PROTECTED_PIDS or not self.kernel.QueryFullProcessImageNameW(self.handle, 0, image, ctypes.byref(size)) or not same_local_path(image.value, sandbox.exe):
                    raise SafetyError("Suspended native image is not the authorized sandbox EXE")
                job.assign(self)
                assert_copy(sandbox, launching=True)
                if self.kernel.ResumeThread(self.thread) != 1:
                    raise SafetyError("Cannot resume the contained native release")
                self.resumed = True
            self.close_thread()
        except BaseException as error:
            cleanup_failed = False
            if self.created:
                try:
                    self.terminate()
                    self.wait(10)
                except Exception:
                    cleanup_failed = True
            try:
                self.close_thread()
                if not cleanup_failed:
                    self.close()
            except Exception:
                cleanup_failed = True
            failure = error if isinstance(error, SafetyError) and not cleanup_failed else SafetyError("Native creation or its task-owned handle cleanup failed")
            if self.created:
                failure.owned_process = self
                failure.creation_cleanup_failed = cleanup_failed
            raise failure from None

    def poll(self):
        if self.handle is None:
            raise SafetyError("The task-owned process handle is closed")
        state = self.kernel.WaitForSingleObject(self.handle, 0)
        if state == 0x102:
            return None
        if state != 0:
            raise SafetyError("Cannot inspect the task-owned process handle")
        code = wintypes.DWORD()
        if not self.kernel.GetExitCodeProcess(self.handle, ctypes.byref(code)):
            raise SafetyError("Cannot inspect the task-owned process exit")
        return int(code.value)

    def wait(self, timeout=None):
        if self.handle is None:
            raise SafetyError("The task-owned process handle is closed")
        milliseconds = 0xFFFFFFFF if timeout is None else max(0, int(timeout * 1000))
        state = self.kernel.WaitForSingleObject(self.handle, milliseconds)
        if state == 0x102:
            raise subprocess.TimeoutExpired("task-owned native release", timeout)
        if state != 0:
            raise SafetyError("Cannot wait on the task-owned process handle")
        return self.poll()

    def terminate(self):
        if not self.created or type(self.pid) is not int or self.pid <= 0 or self.pid in PROTECTED_PIDS:
            raise SafetyError("Refusing to terminate a process not created by this task")
        if self.poll() is None and not self.kernel.TerminateProcess(self.handle, 1):
            raise SafetyError("Cannot terminate the task-owned native process handle")

    def close_thread(self):
        if self.thread is not None:
            if not self.kernel.CloseHandle(self.thread):
                raise SafetyError("Cannot close the task-owned primary thread handle")
            self.thread = None

    def close(self):
        self.close_thread()
        if self.handle is not None:
            if not self.kernel.CloseHandle(self.handle):
                raise SafetyError("Cannot close the task-owned process handle")
            self.handle = None


class TcpListener4(ctypes.Structure):
    _fields_ = [(name, wintypes.DWORD) for name in ["state", "address", "port", "remote_address", "remote_port", "pid"]]


class TcpListener6(ctypes.Structure):
    _fields_ = [("address", ctypes.c_ubyte * 16), ("scope", wintypes.DWORD), ("port", wintypes.DWORD), ("remote_address", ctypes.c_ubyte * 16), ("remote_scope", wintypes.DWORD), ("remote_port", wintypes.DWORD), ("state", wintypes.DWORD), ("pid", wintypes.DWORD)]


def tcp_listeners(port):
    if os.name != "nt":
        raise SafetyError("CDP owner verification requires Windows")
    library = ctypes.WinDLL("iphlpapi", use_last_error=True)
    library.GetExtendedTcpTable.argtypes = [ctypes.c_void_p, ctypes.POINTER(wintypes.DWORD), wintypes.BOOL, wintypes.ULONG, ctypes.c_int, wintypes.ULONG]
    library.GetExtendedTcpTable.restype = wintypes.DWORD
    found = []
    for family, row_type in [(2, TcpListener4), (23, TcpListener6)]:
        size = wintypes.DWORD()
        result = library.GetExtendedTcpTable(None, ctypes.byref(size), False, family, 3, 0)
        if result not in {0, 122} or size.value > 8 * 1024 * 1024:
            raise SafetyError("Cannot inspect CDP listeners")
        for _ in range(4):
            table = ctypes.create_string_buffer(max(size.value, 4))
            result = library.GetExtendedTcpTable(table, ctypes.byref(size), False, family, 3, 0)
            if result == 0:
                break
            if result != 122 or size.value > 8 * 1024 * 1024:
                raise SafetyError("Cannot inspect CDP listener ownership")
        else:
            raise SafetyError("CDP listener table changed repeatedly")
        count = struct.unpack_from("<I", table.raw, 0)[0]
        if 4 + count * ctypes.sizeof(row_type) > len(table):
            raise SafetyError("Invalid native listener table")
        for index in range(count):
            row = row_type.from_buffer_copy(table, 4 + index * ctypes.sizeof(row_type))
            if socket.ntohs(row.port & 0xFFFF) == port:
                address = socket.inet_ntop(socket.AF_INET, struct.pack("<I", row.address)) if family == 2 else socket.inet_ntop(socket.AF_INET6, bytes(row.address))
                found.append({"address": address, "pid": int(row.pid)})
    return found


def assert_owned_cdp(port, job):
    listeners = tcp_listeners(port)
    if not any(item["address"] == "127.0.0.1" for item in listeners):
        raise SafetyError("The selected CDP port has no IPv4 loopback listener")
    if any(item["address"] not in {"127.0.0.1", "::1"} or not job.contains_pid(item["pid"]) for item in listeners):
        raise SafetyError("Refusing a non-loopback or unowned CDP listener")
    return listeners


class NoCdpRedirects(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        raise SafetyError("CDP HTTP redirects are not allowed")


def validated_cdp_websocket(value, port):
    try:
        parsed = urlsplit(value)
        valid = parsed.scheme == "ws" and parsed.hostname == "127.0.0.1" and parsed.port == port and parsed.username is None and parsed.password is None and not parsed.query and not parsed.fragment and re.fullmatch(r"/devtools/browser/[a-zA-Z0-9-]+", parsed.path)
    except (TypeError, ValueError):
        valid = False
    if not valid:
        raise SafetyError("Release CDP must advertise a direct IPv4 loopback browser websocket")
    return value


def assert_browser_arguments(arguments, profile, port):
    if not isinstance(arguments, list) or not all(isinstance(item, str) for item in arguments):
        raise SafetyError("Cannot verify the native WebView2 command line")
    profile = evidence_path(profile)
    for flag, expected in [("--user-data-dir", str(profile)), ("--remote-debugging-port", str(port)), ("--remote-debugging-address", "127.0.0.1")]:
        values = []
        for index, item in enumerate(arguments):
            if item.startswith(flag + "="):
                values.append(item[len(flag) + 1:].strip('"'))
            elif item == flag:
                values.append(arguments[index + 1].strip('"') if index + 1 < len(arguments) else None)
        valid = len(values) == 1 and isinstance(values[0], str) and (same_local_path(values[0], expected) if flag == "--user-data-dir" else values[0] == expected)
        if not valid:
            raise SafetyError("Native WebView2 isolation flags are absent, duplicated or unexpected")


def assert_owned_webview(port, job, profile):
    listeners = assert_owned_cdp(port, job)
    profile = evidence_path(profile)
    for pid in {item["pid"] for item in listeners}:
        assert_browser_arguments(job.process_arguments(pid), profile, port)
    if not profile.is_dir():
        raise SafetyError("The selected private WebView2 profile is unavailable")
    assert_owned_cdp(port, job)


def production_page_url(value):
    parsed = urlsplit(value)
    return parsed.scheme in {"http", "https", "tauri"} and parsed.hostname in {"tauri.localhost", "localhost"} and (parsed.hostname == "tauri.localhost" or parsed.scheme == "tauri") and parsed.port is None and parsed.username is None and parsed.password is None and not parsed.query and not parsed.fragment


def wait_for_page_target(endpoint, port, job, opener, timeout=45):
    # 在页面从 about:blank 导航到内嵌地址前附着 CDP 会绑定到过时目标，
    # 因此先确认页面目标已经稳定为发布地址，再做浏览器级连接。
    deadline = time.perf_counter() + timeout
    while time.perf_counter() < deadline:
        assert_owned_cdp(port, job)
        try:
            with opener.open(endpoint + "/json/list", timeout=0.4) as response:
                payload = response.read(64 * 1024 + 1)
            if len(payload) > 64 * 1024:
                raise SafetyError("Oversized CDP target response")
            targets = json.loads(payload)
            if not isinstance(targets, list):
                raise SafetyError("Invalid CDP target document")
            if any(isinstance(target, dict) and target.get("type") == "page" and isinstance(target.get("url"), str) and production_page_url(target["url"]) for target in targets):
                return
        except SafetyError:
            raise
        except (OSError, ValueError):
            pass
        time.sleep(0.1)
    raise SafetyError("The embedded page target did not reach the release address")


class MicrosoftReference:
    __slots__ = ("_expected",)

    def __init__(self, expected):
        self._expected = expected

    def __repr__(self):
        return "<private Microsoft configuration reference>"

    def challenge(self):
        salt = uuid.uuid4().hex
        document = json.dumps([salt, self._expected["clientId"], self._expected["authority"], self._expected["redirectUri"]], ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        return {"salt": salt, "digest": hashlib.sha256(document).hexdigest()}


def load_microsoft_reference(path=None):
    # Only --run reads the authorized reference, never copies it or logs its values.
    try:
        reference = reject_links(path or ROOT / "secrets.local.json")
        with reference.open("rb") as stream:
            content = stream.read(256 * 1024 + 1)
        if len(content) > 256 * 1024:
            raise ValueError()
        document = json.loads(content.decode("utf-8"))
        section = document.get("microsoft") if isinstance(document, dict) else None
        if not isinstance(section, dict):
            raise ValueError()

        def field(name):
            value = section.get(name)
            if not isinstance(value, str):
                return None
            value = value.strip()
            return value if value and len(value.encode("utf-8")) <= 8192 and not any(unicodedata.category(char) == "Cc" for char in value) else None

        client = field("clientId")
        if not client or client == "00000000-0000-0000-0000-000000000000":
            raise ValueError()
        expected = {"clientId": client, "authority": field("authority") or "consumers", "redirectUri": field("redirectUri") or "nickel://auth"}
        return MicrosoftReference(expected)
    except Exception:
        raise SafetyError("Authorized Microsoft reference is unavailable or invalid; no values are reported") from None


def verify_embedded_microsoft(page, sandbox, reference):
    assert_copy(sandbox)
    result = page.evaluate("""async ({salt, digest}) => {
        try {
            const value = await window.__TAURI_INTERNALS__.invoke('msa_config_get', {});
            const publicFieldsOnly = value !== null && typeof value === 'object' && !Array.isArray(value)
                && Object.keys(value).sort().join(',') === 'authority,clientId,redirectUri'
                && ['clientId', 'authority', 'redirectUri'].every(key => typeof value[key] === 'string' && value[key].length > 0);
            if (!publicFieldsOnly) return {ipcResolved: true, publicFieldsOnly: false, matches: false};
            const bytes = new TextEncoder().encode(JSON.stringify([salt, value.clientId, value.authority, value.redirectUri]));
            const hashed = await crypto.subtle.digest('SHA-256', bytes);
            const actual = [...new Uint8Array(hashed)].map(byte => byte.toString(16).padStart(2, '0')).join('');
            return {ipcResolved: true, publicFieldsOnly: true, matches: actual === digest};
        } catch {
            return {ipcResolved: false, publicFieldsOnly: false, matches: false};
        }
    }""", reference.challenge())
    if not isinstance(result, dict) or set(result) != {"ipcResolved", "publicFieldsOnly", "matches"} or not all(type(value) is bool for value in result.values()):
        raise SafetyError("The Microsoft configuration probe may return only boolean evidence")
    check(all(result.values()), "Live msa_config_get must expose exactly three matching embedded public fields")
    assert_copy(sandbox)
    return {"liveIpcResolved": True, "publicFieldsOnly": True, "matchesAuthorizedReference": True, "externalJsonAbsent": True, "valuesRecorded": False, "realAuthenticationExercised": False, "curseForgeAuthenticationVerified": False}


def native_windows(pid):
    if os.name != "nt":
        raise SafetyError("Native window acceptance requires Windows")
    from ctypes import wintypes
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.IsWindowVisible.restype = wintypes.BOOL
    user32.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    user32.EnumWindows.argtypes = [callback_type, wintypes.LPARAM]
    windows = []

    @callback_type
    def visit(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            rectangle = wintypes.RECT()
            if user32.GetWindowRect(hwnd, ctypes.byref(rectangle)):
                windows.append({"hwnd": hex(hwnd), "pid": pid, "rectangle": [rectangle.left, rectangle.top, rectangle.right, rectangle.bottom]})
        return True

    user32.EnumWindows(visit, 0)
    return windows


class WindowsRecordLock:
    """Allow record reads/writes but deny deletion, making atomic replacement fail."""
    share_mode = 3

    def __init__(self, path):
        self.path = evidence_path(path)
        self.handle = None

    def __enter__(self):
        if os.name != "nt":
            raise SafetyError("Windows file-lock acceptance requires Windows")
        self.path = evidence_path(self.path)
        self.kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel.CreateFileW.argtypes = [wintypes.LPCWSTR, wintypes.DWORD, wintypes.DWORD, ctypes.c_void_p, wintypes.DWORD, wintypes.DWORD, wintypes.HANDLE]
        self.kernel.CreateFileW.restype = wintypes.HANDLE
        self.kernel.CloseHandle.argtypes = [wintypes.HANDLE]
        self.kernel.CloseHandle.restype = wintypes.BOOL
        self.handle = self.kernel.CreateFileW(str(self.path), 0x80000000, self.share_mode, None, 3, 0x80, None)
        if not self.handle or self.handle == ctypes.c_void_p(-1).value:
            self.handle = None
            raise SafetyError("Cannot lock a task-owned native file")
        return self

    def __exit__(self, *_):
        if self.handle is not None:
            if not self.kernel.CloseHandle(self.handle):
                raise SafetyError("Cannot release a task-owned native file lock")
            self.handle = None


class WindowsExecutableLock(WindowsRecordLock):
    """Keep the authorized image read-only until its contained thread is resumed."""
    share_mode = 1


class NativeSession:
    def __init__(self, playwright, sandbox, output, sessions):
        self.playwright, self.sandbox = playwright, sandbox
        self.output = evidence_path(output, new=True)
        self.sessions = sessions
        self.process = self.browser = self.page = self.job = self.browser_cdp = None
        self.observer = None
        self.exited = threading.Event()
        self.exit_info = None
        self.cleaned = False
        self.details = {"sandboxExe": str(sandbox.exe), "sha256": sandbox.sha256, "startupArguments": [], "forcedCleanup": False, "cleanupFailed": False, "nativeExecuted": False}

    def __enter__(self):
        if os.name != "nt":
            raise SafetyError("--run requires an isolated Windows desktop")
        assert_copy(self.sandbox, launching=True)
        self.output.mkdir(parents=True, exist_ok=False)
        self.sessions.append(self.details)
        try:
            with socket.socket() as reservation:
                reservation.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            environment = isolated_environment(self.sandbox, self.output, port)
            # WebView2 在用户数据目录下固定创建 EBWebView，浏览器进程的
            # --user-data-dir 指向该子目录，校验必须使用真实 profile 路径。
            profile = str(Path(environment["WEBVIEW2_USER_DATA_FOLDER"]) / "EBWebView")
            self.job = WindowsJob()
            try:
                self.process = OwnedNativeProcess(self.sandbox, environment, self.job)
            except SafetyError as error:
                failed = getattr(error, "owned_process", None)
                if failed is not None:
                    self.details.update({"nativeProcessCreated": True, "nativeExecuted": failed.resumed, "ownedPid": failed.pid, "creationCleanupInitiallyFailed": error.creation_cleanup_failed})
                    if failed.handle is not None:
                        self.process = failed
                raise
            self.details.update({"ownedPid": self.process.pid, "cdpLoopbackPort": port, "webview2UserData": profile, "nativeExecuted": True, "containedBeforeResume": True})

            def observe_exit():
                try:
                    code = self.process.wait()
                    self.exit_info = {"exitCode": code, "controllerMonotonicSeconds": time.perf_counter()}
                    self.details["processExit"] = self.exit_info
                except Exception as error:
                    self.details["exitObservationFailure"] = safe_failure(error)
                finally:
                    self.exited.set()

            self.observer = threading.Thread(target=observe_exit, daemon=True)
            self.observer.start()
            endpoint = "http://127.0.0.1:" + str(port)
            opener = build_opener(ProxyHandler({}), NoCdpRedirects())
            deadline, websocket = time.perf_counter() + 45, None
            while time.perf_counter() < deadline:
                self.assert_live()
                if not tcp_listeners(port):
                    time.sleep(0.1)
                    continue
                assert_owned_webview(port, self.job, profile)
                try:
                    with opener.open(endpoint + "/json/version", timeout=0.4) as response:
                        payload = response.read(64 * 1024 + 1)
                    if len(payload) > 64 * 1024:
                        raise SafetyError("Oversized CDP discovery response")
                    metadata = json.loads(payload)
                    if not isinstance(metadata, dict):
                        raise SafetyError("Invalid CDP discovery document")
                    websocket = validated_cdp_websocket(metadata.get("webSocketDebuggerUrl"), port)
                    break
                except SafetyError:
                    raise
                except (OSError, ValueError):
                    time.sleep(0.1)
            if websocket is None:
                raise SafetyError("Release WebView2 CDP is unavailable; WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS must be honored")
            assert_owned_webview(port, self.job, profile)
            self.details["webviewProfileVerifiedBeforeAttachment"] = True
            wait_for_page_target(endpoint, port, self.job, opener)
            self.browser = self.playwright.chromium.connect_over_cdp(websocket, timeout=15000)
            assert_owned_cdp(port, self.job)
            self.browser_cdp = self.browser.new_browser_cdp_session()
            arguments = self.browser_cdp.send("Browser.getBrowserCommandLine")
            if not isinstance(arguments, dict):
                raise SafetyError("Invalid WebView2 command-line observation")
            assert_browser_arguments(arguments.get("arguments"), profile, port)
            self.details.update({"cdpOwnerVerified": True, "webviewProfileVerified": True, "webviewBrowserVersion": self.browser.version})
            deadline = time.perf_counter() + 20
            observed_urls = set()
            while time.perf_counter() < deadline:
                self.assert_live()
                candidates = [page for context in self.browser.contexts for page in context.pages]
                for page in candidates:
                    try:
                        # Playwright 的 page.url 可能保留连接时的 about:blank，
                        # 因此以页面内实时 location.href 判定发布地址。
                        href = page.evaluate("location.href")
                    except Exception:
                        continue
                    if not isinstance(href, str):
                        continue
                    observed_urls.add(href)
                    if not production_page_url(href):
                        continue
                    try:
                        if page.evaluate("Boolean(window.__TAURI_INTERNALS__ && document.querySelector('.ni-shell'))"):
                            self.page = page
                            break
                    except Exception:
                        pass
                if self.page is not None:
                    break
                time.sleep(0.1)
            if self.page is None:
                self.details["observedPageUrls"] = sorted(observed_urls)
                try:
                    self.details["rawCdpTargets"] = [
                        {"type": item.get("type"), "url": item.get("url")}
                        for item in self.browser_cdp.send("Target.getTargets").get("targetInfos", [])
                    ]
                except Exception as error:
                    self.details["rawCdpTargetsError"] = safe_failure(error)
                raise SafetyError("No live embedded production Tauri page was found; dev-server pages are not allowed")
            self.page.set_default_timeout(15000)
            from playwright.sync_api import expect
            expect(self.page.locator("#ni-splash")).to_have_count(0, timeout=15000)
            self.page.wait_for_function("document.body.inert === false", timeout=15000)
            windows = native_windows(self.process.pid)
            check(bool(windows), "The release process has no visible native window")
            self.details["visibleWindowsAtStartup"] = windows
            check(self.page.evaluate("!['__startupFixture', '__STARTUP_SETUP__', '__TAURI_MOCK_IPC__', '__nativeCountdown', '__nativeCountdownObserver'].some(key => key in window)"), "A simulated browser fixture cannot count as native acceptance")
            current = self.status()
            check(current.get("currentVersion") == REQUIRED_VERSION, "This is not the required release " + REQUIRED_VERSION)
            check(current.get("eulaVersion") == self.sandbox.protocol.eula_revision, "Native EULA revision does not match the current source contract")
            self.verify_isolation()
            return self
        except BaseException:
            self.cleanup()
            raise

    def assert_live(self):
        check(self.process is not None and self.process.poll() is None, "Task-owned native release process exited unexpectedly")

    def status(self):
        return self.page.evaluate("window.__TAURI_INTERNALS__.invoke('onboarding_get', {})")

    def verify_isolation(self):
        assert_copy(self.sandbox)
        expected_game = str(self.sandbox.root / self.sandbox.protocol.game_directory)
        native_game = self.page.evaluate("window.__TAURI_INTERNALS__.invoke('default_minecraft_dir', {})")
        if not isinstance(native_game, str) or not same_local_path(native_game, expected_game):
            raise SafetyError("Live backend selected a game directory outside the copied release")
        registry_text = self.page.evaluate("window.__TAURI_INTERNALS__.invoke('instances_get', {})")
        registry = json.loads(registry_text)
        roots = registry.get("roots")
        if not isinstance(roots, list) or len(roots) != 1 or not isinstance(roots[0], str) or not same_local_path(roots[0], expected_game) or not isinstance(registry.get("active"), str) or not same_local_path(registry["active"], expected_game):
            raise SafetyError("Live backend instance roots do not belong exclusively to the copied release")
        theme = self.page.evaluate("""() => {
            const root = document.documentElement;
            const shell = document.querySelector('.ni-shell');
            const titlebar = document.querySelector('.ni-titlebar');
            const background = shell?.querySelector('.ni-bg');
            return {
                darkTheme: root.dataset.theme === 'dark', nickelAccent: root.dataset.accent === 'nickel',
                tintedTitlebar: titlebar?.dataset.tinted === 'true',
                backgroundBeforeTitlebar: !!background && background.nextElementSibling === titlebar,
                nativeArtwork: !!background?.querySelector('[data-motif="nickel"]'),
                rootTokensPresent: ['--accent', '--accent-text', '--theme-chrome-bg'].every(key => getComputedStyle(root).getPropertyValue(key).trim().length > 0)
            };
        }""")
        check(isinstance(theme, dict) and all(type(value) is bool and value for value in theme.values()) and len(theme) == 6, "Native root order or persisted theme differs from the synthetic isolated settings")
        self.details.update({"backendGameRootVerified": True, "backendInstanceRootsVerified": True, "nativeRootAndTheme": theme})

    def wait_exit(self, timeout=15):
        check(self.exited.wait(timeout), "Task-owned native process did not exit")
        check(self.exit_info is not None and self.exit_info["exitCode"] == 0, "Native process exit was unsuccessful")
        check(not self.details["forcedCleanup"], "Harness cleanup cannot satisfy native process-exit acceptance")
        check(not native_windows(self.process.pid), "Native window survived process exit")
        return self.exit_info

    def close_normally(self):
        self.page.locator(".ni-winbtn--close").click()
        self.wait_exit()

    def cleanup(self):
        if self.cleaned:
            if self.details["cleanupFailed"]:
                raise SafetyError("Task-owned native cleanup previously failed")
            return
        self.cleaned = True
        errors, stopped = [], self.process is None
        protected = self.process is not None and self.process.pid in PROTECTED_PIDS
        if protected:
            errors.append(safe_failure(SafetyError("Refusing cleanup of a protected controller process")))
        else:
            if self.process is not None:
                try:
                    stopped = self.process.poll() is not None
                except Exception as error:
                    errors.append(safe_failure(error))
                if not stopped:
                    self.details["forcedCleanup"] = True
            # A failed parent-status probe must not bypass owned-descendant cleanup.
            try:
                if self.job is not None:
                    self.job.close()
                    self.details["taskJobClosed"] = True
                elif not stopped:
                    self.process.terminate()
            except Exception as error:
                errors.append(safe_failure(error))
            if not stopped:
                try:
                    self.process.wait(timeout=10)
                    stopped = True
                except Exception as error:
                    errors.append(safe_failure(error))
        if self.observer is not None:
            try:
                self.observer.join(timeout=2)
                if self.observer.is_alive():
                    raise SafetyError("Task-owned process observer did not finish")
            except Exception as error:
                errors.append(safe_failure(error))
                stopped = False
        if self.process is not None and stopped and not protected:
            try:
                self.process.close()
            except Exception as error:
                errors.append(safe_failure(error))
        if self.browser is not None:
            try:
                self.browser.close()
            except Exception as error:
                errors.append(safe_failure(error))
            try:
                if self.browser.is_connected():
                    raise SafetyError("Task-owned browser connection survived cleanup")
                self.details["cdpDisconnected"] = True
            except Exception as error:
                errors.append(safe_failure(error))
        output_ready = False
        try:
            output_ready = evidence_path(self.output).is_dir()
            if not output_ready:
                raise SafetyError("Native session output disappeared before cleanup")
            assert_private_tree(self.output)
            assert_copy(self.sandbox)
        except Exception as error:
            errors.append(safe_failure(error))
        self.details["cleanupFailed"] = bool(errors)
        if errors:
            self.details["cleanupErrors"] = errors
        if output_ready:
            try:
                write_json(self.output / "session.json", self.details)
            except Exception as error:
                errors.append(safe_failure(error))
                self.details["cleanupErrors"] = errors
        self.details["cleanupFailed"] = bool(errors)
        if errors:
            raise SafetyError("Task-owned native cleanup failed; no acceptance pass may be reported")

    def __exit__(self, error_type, error, _):
        self.cleanup()


def non_dismiss(session, title):
    from playwright.sync_api import expect
    page = session.page
    page.keyboard.press("Escape")
    page.locator(".ni-modal__backdrop").click(position={"x": 4, "y": 4})
    close = page.locator(".ni-winbtn--close").bounding_box()
    check(close is not None, "Production shell close button was not found")
    page.mouse.click(close["x"] + close["width"] / 2, close["y"] + close["height"] / 2)
    expect(page.get_by_role("dialog", name=title, exact=True)).to_be_visible()
    expect(page.locator(".startup-modal .ni-modal__head button")).to_have_count(0)
    for _ in range(6):
        page.keyboard.press("Tab")
        check(page.evaluate("document.querySelector('.startup-modal').contains(document.activeElement)"), "Keyboard focus escaped native onboarding")
    session.assert_live()
    check(page.locator(".ni-shell").evaluate("shell => shell.inert"), "Dismissal unlocked the native shell")


def ready(session):
    from playwright.sync_api import expect
    session.page.wait_for_function("document.querySelector('.ni-shell')?.inert === false")
    expect(session.page.locator(".startup-modal")).to_have_count(0)
    session.assert_live()


def acknowledge(session):
    from playwright.sync_api import expect
    title = "Nickel " + REQUIRED_VERSION + " \u66f4\u65b0\u8bf4\u660e"
    expect(session.page.get_by_role("dialog", name=title, exact=True)).to_be_visible()
    non_dismiss(session, title)
    session.page.get_by_role("button", name=ACK, exact=True).click()
    ready(session)


def blocked_gate(session, title, eula=False):
    from playwright.sync_api import expect
    page = session.page
    expect(page.get_by_role("dialog", name=title, exact=True)).to_be_visible()
    expect(page.get_by_role("dialog")).to_have_count(1)
    check(page.locator(".ni-shell").evaluate("shell => shell.inert"), "Native onboarding unlocked the shell before acknowledgment")
    check(page.locator(".startup-modal").evaluate("dialog => !dialog.closest('.ni-shell') && dialog.parentElement.parentElement === document.body"), "Native onboarding is not a separate dialog outside the blocked shell")
    finish_rejected = page.evaluate("""async () => {
        try { await window.__TAURI_INTERNALS__.invoke('onboarding_finish', {}); return false; }
        catch { return true; }
    }""")
    check(finish_rejected is True, "Backend startup dispatch did not wait for both native acknowledgments")
    if eula:
        expect(page.locator(".startup-changelog")).to_have_count(0)
        start_rejected = page.evaluate("""async () => {
            try { await window.__TAURI_INTERNALS__.invoke('portable_bootstrap_start', {}); return false; }
            catch { return true; }
        }""")
        check(start_rejected is True, "Backend portable dispatch did not wait for native EULA acceptance")
    snapshot = page.evaluate("window.__TAURI_INTERNALS__.invoke('app_tasks_snapshot', {})")
    check(isinstance(snapshot, list) and not snapshot, "Native startup created a task before acknowledgment")
    return {"shellBlocked": True, "separateDialog": True, "startupDispatchRejected": True, "noDownloadTasks": True, "portableStartRejectedBeforeEula": eula}


def backend_rejection_without_renderer(session):
    page = session.page
    check(page.locator(".ni-shell").evaluate("shell => shell.inert"), "Backend-only refusal unlocked the shell")
    renderer = page.context.new_cdp_session(page)
    paused, resumed = threading.Event(), threading.Event()
    renderer.on("Debugger.paused", lambda _event: paused.set())
    renderer.on("Debugger.resumed", lambda _event: resumed.set())
    renderer.send("Debugger.enable")
    sent = time.perf_counter()
    page.evaluate("window.__TAURI_INTERNALS__.invoke('onboarding_reject', {})")
    returned = time.perf_counter()
    check(returned - sent < 0.2, "Native refusal IPC latency is too large for timing acceptance")
    check(session.status().get("exiting") is True, "Real backend did not enter the pending-exit state")
    check(page.locator(".startup-exit-button").count() == 0, "Backend-only refusal unexpectedly started the frontend timer")

    def observe_pause(milliseconds=50):
        check(not resumed.is_set(), "Renderer resumed before backend-only process exit")
        try:
            page.wait_for_timeout(milliseconds)
        except Exception:
            check(session.exited.is_set(), "Renderer observation ended before native exit; uninterrupted pause cannot be proven")
        check(not resumed.is_set(), "Renderer resumed during backend-only process exit")

    renderer.send("Debugger.pause")
    observe_pause()
    check(paused.is_set(), "Renderer was not paused; frontend fallback cannot be ruled out")
    deadline = sent + 7
    while not session.exited.is_set():
        check(time.perf_counter() < deadline, "Backend-only refusal exceeded its real-time deadline")
        observe_pause()
    exit_info = session.wait_exit(timeout=max(0, deadline - time.perf_counter()))
    # Pump queued debugger events once more after the owned exit observer fires.
    observe_pause(0)
    delay = exit_info["controllerMonotonicSeconds"] - sent
    check(5 <= delay <= 6, "The native backend did not own a real five-second process exit")
    check(session.sandbox.local_record() == {}, "Backend-only refusal persisted consent")
    return {"exitDelaySeconds": delay, "ipcRoundTripSeconds": returned - sent, "rendererPaused": True, "rendererResumed": False, "rendererObservationContinuous": True, "frontendRefusalTimerStarted": False, "frontendFallbackCouldRun": False, "nativeExitCode": exit_info["exitCode"]}


def run_native(playwright, source, evidence, protocol, expected_sha256, reference, smoke=False):
    from playwright.sync_api import expect
    expected_sha256 = validated_sha256(expected_sha256, required=True)
    results, sessions = [], []
    first = prepare_copy(source, evidence / "sandboxes/first-run", protocol, expected_sha256)
    full_record, halted = None, False

    def case(name, function):
        nonlocal halted
        if smoke and name in {"native-version-upgrade-only-changelog", "native-save-failure-and-retry", "backend-owned-native-process-exit"}:
            return
        output = evidence_path(evidence / "cases" / name, new=True)
        output.mkdir(parents=True, exist_ok=False)
        started = time.perf_counter()
        entry = {"case": name, "nativeAcceptance": True, "passed": False}
        if halted:
            entry.update({"skipped": True, "reason": "An earlier native safety or cleanup failure halted all further launches"})
        else:
            try:
                entry["observations"] = function(output)
                entry["passed"] = True
            except Exception as error:
                entry.update(safe_failure(error))
                halted = isinstance(error, SafetyError) or any(item["cleanupFailed"] for item in sessions)
        entry["durationSeconds"] = round(time.perf_counter() - started, 3)
        write_json(output / "result.json", entry)
        results.append(entry)
        print(("PASS " if entry["passed"] else "SKIP " if entry.get("skipped") else "FAIL ") + name, flush=True)

    def first_run(output):
        nonlocal full_record
        with NativeSession(playwright, first, output / "session", sessions) as session:
            page = session.page
            gate = blocked_gate(session, EULA_TITLE, eula=True)
            expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_disabled()
            expect(page.get_by_role("checkbox")).to_have_attribute("aria-checked", "false")
            expect(page.locator(".startup-modal input[type='text']")).to_have_count(0)
            non_dismiss(session, EULA_TITLE)
            check(first.local_record() == {}, "First start already contained consent")
            page.screenshot(path=str(evidence_path(output / "eula.png", new=True)))
            before = int(time.time())
            page.get_by_role("checkbox").click()
            expect(page.get_by_role("button", name=ACCEPT, exact=True)).to_be_enabled()
            page.get_by_role("button", name=ACCEPT, exact=True).click()
            expect(page.locator(".startup-changelog")).to_be_visible()
            signed = first.local_record()
            check(signed.get("eulaVersion") == protocol.eula_revision, "Native revision was not persisted locally")
            accepted_at = signed.get("eulaAcceptedAt")
            check(type(accepted_at) is int and before <= accepted_at <= int(time.time()), "Native signature time is not valid Unix seconds")
            check(signed.get("changelogSeenVersion") != REQUIRED_VERSION, "Changelog was acknowledged before its button")
            blocked_gate(session, "Nickel " + REQUIRED_VERSION + " \u66f4\u65b0\u8bf4\u660e")
            changelog = json.loads((ROOT / "CHANGELOG.json").read_text(encoding="utf-8"))
            check(changelog.get("version") == REQUIRED_VERSION, "The native changelog contract is not version 1.0.2")
            for section in changelog["sections"]:
                expect(page.locator(".startup-changelog")).to_contain_text(section["title"])
                for item in section["items"]:
                    expect(page.locator(".startup-changelog")).to_contain_text(item)
            write_json(output / "signed-record.json", signed)
            page.screenshot(path=str(evidence_path(output / "changelog.png", new=True)))
            acknowledge(session)
            full_record = first.local_record()
            check(full_record.get("changelogSeenVersion") == REQUIRED_VERSION, "Native acknowledgment was not persisted")
            page.get_by_role("button", name=SETTINGS, exact=True).click()
            expect(page.locator('.ni-navtab[data-active="true"]')).to_have_text(SETTINGS)
            page.screenshot(path=str(evidence_path(output / "shell-settings.png", new=True)))
            write_json(output / "complete-record.json", full_record)
            session.close_normally()
            return {"record": full_record, "normalShellEnabled": True, "blockedGate": gate}

    case("first-run-native-order-and-shell", first_run)

    def restart(output):
        check(full_record is not None, "First-run native acceptance did not produce a reusable record")
        with NativeSession(playwright, first, output / "session", sessions) as session:
            ready(session)
            check(first.local_record() == full_record, "Native restart rewrote consent")
            check(not session.status()["needsEula"] and not session.status()["needsChangelog"], "Native restart prompted again")
            session.page.screenshot(path=str(evidence_path(output / "accepted-restart.png", new=True)))
            session.close_normally()
            return {"recordUnchanged": True, "freshPrivateWebviewProfile": True}

    case("accepted-native-restart-no-prompts", restart)

    def embedded_config(output):
        check(full_record is not None, "The same release copy has not completed native onboarding")
        with NativeSession(playwright, first, output / "session", sessions) as session:
            ready(session)
            observations = verify_embedded_microsoft(session.page, first, reference)
            check(first.local_record() == full_record, "Microsoft public configuration lookup rewrote consent")
            session.close_normally()
            return {**observations, "sameReleaseCopy": True, "explicitReleaseHashMatched": True}

    case("same-release-embedded-microsoft-public-config", embedded_config)

    def upgrade(output):
        sandbox = prepare_copy(source, evidence / "sandboxes/upgrade", protocol, expected_sha256)
        seed = {"eulaVersion": protocol.eula_revision, "eulaAcceptedAt": 1700000000, "changelogSeenVersion": "1.0.1"}
        sandbox.seed_record(seed)
        with NativeSession(playwright, sandbox, output / "session", sessions) as session:
            current = session.status()
            check(not current["needsEula"] and current["needsChangelog"], "Native upgrade reset EULA or skipped the new changelog")
            expect(session.page.get_by_role("dialog", name=EULA_TITLE, exact=True)).to_have_count(0)
            acknowledge(session)
            saved = sandbox.local_record()
            check(saved["eulaAcceptedAt"] == seed["eulaAcceptedAt"] and saved["changelogSeenVersion"] == REQUIRED_VERSION, "Native upgrade changed the signature")
            write_json(output / "upgraded-record.json", saved)
            session.close_normally()
            return {"record": saved}

    case("native-version-upgrade-only-changelog", upgrade)

    def save_failure(output):
        sandbox = prepare_copy(source, evidence / "sandboxes/save-retry", protocol, expected_sha256)
        sandbox.seed_record({})
        with NativeSession(playwright, sandbox, output / "session", sessions) as session:
            page = session.page
            expect(page.get_by_role("dialog", name=EULA_TITLE, exact=True)).to_be_visible()
            page.get_by_role("checkbox").click()
            with WindowsRecordLock(sandbox.record):
                page.get_by_role("button", name=ACCEPT, exact=True).click()
                expect(page.get_by_role("alert")).to_be_visible()
                check(sandbox.local_record() == {}, "Failed native save persisted consent")
                check(page.locator(".ni-shell").evaluate("shell => shell.inert"), "Failed native save unlocked the shell")
                page.screenshot(path=str(evidence_path(output / "save-failure.png", new=True)))
            page.get_by_role("button", name=ACCEPT, exact=True).click()
            acknowledge(session)
            check(not list(sandbox.record.parent.glob("onboarding.json.tmp-*")), "Native save failure left temporary record files")
            saved = sandbox.local_record()
            write_json(output / "retry-record.json", saved)
            session.close_normally()
            return {"realWindowsFileLock": True, "record": saved}

    case("native-save-failure-and-retry", save_failure)

    def refusal(output):
        sandbox = prepare_copy(source, evidence / "sandboxes/refusal-ui", protocol, expected_sha256)
        with NativeSession(playwright, sandbox, output / "session", sessions) as session:
            page = session.page
            expect(page.get_by_role("dialog", name=EULA_TITLE, exact=True)).to_be_visible()
            original_dialog = page.get_by_role("dialog", name=EULA_TITLE, exact=True).element_handle()
            expect(page.get_by_role("button", name=REJECT, exact=True)).to_have_class(re.compile(r"\bnw-btn--danger\b"))
            before_anchor = time.perf_counter()
            anchor_js = page.evaluate("""() => {
                window.__nativeCountdown = [];
                const capture = () => {
                    const button = document.querySelector('.startup-exit-button');
                    if (button && window.__nativeCountdown.at(-1)?.text !== button.textContent)
                        window.__nativeCountdown.push({text: button.textContent, at: performance.now()});
                };
                window.__nativeCountdownObserver = new MutationObserver(capture);
                window.__nativeCountdownObserver.observe(document.body, {subtree: true, childList: true, characterData: true});
                return performance.now();
            }""")
            after_anchor = time.perf_counter()
            check(after_anchor - before_anchor < 0.2, "Native clock observation latency is too large for timing acceptance")
            anchor = (before_anchor + after_anchor) / 2
            page.get_by_role("button", name=REJECT, exact=True).click()
            expect(page.get_by_role("dialog", name=EXIT_TITLE, exact=True)).to_be_visible()
            check(original_dialog.evaluate("dialog => !dialog.isConnected"), "Native refusal did not replace EULA with a separate dialog")
            check(session.status().get("exiting") is True, "UI refusal did not reach the real backend; frontend fallback must not hide this")
            check(page.locator(".ni-shell").evaluate("shell => shell.inert"), "Refusal unlocked the native shell")
            button = page.locator(".startup-exit-button")
            expect(button).to_be_disabled()
            check(button.evaluate("button => button.tagName === 'BUTTON' && button.disabled"), "Native exit control must be a truly disabled button")
            expect(button).to_have_text(EXIT_PREFIX + "5")
            expect(page.get_by_role("dialog").locator("button")).to_have_count(1)
            expect(page.locator(".startup-rejection")).to_contain_text("Nickel")
            expect(page.locator(".startup-rejection")).to_contain_text("\u65e0\u6cd5\u63d0\u4f9b\u670d\u52a1")
            page.mouse.move(1, 1)
            page.wait_for_timeout(200)
            style = "button => Object.fromEntries([...getComputedStyle(button)].map(key => [key, getComputedStyle(button).getPropertyValue(key)]))"
            before = button.evaluate(style)
            text_before = button.text_content()
            png_before = button.screenshot(path=str(evidence_path(output / "hover-before.png", new=True)))
            button.hover()
            hover = button.evaluate(style)
            png_hover = button.screenshot(path=str(evidence_path(output / "hover-during.png", new=True)))
            check(button.text_content() == text_before, "Native screenshot sampling crossed a countdown tick; rerun this case")
            check(before == hover and png_before == png_hover, "Native refusal hover changed disabled appearance")
            page.keyboard.press("Escape")
            page.locator(".ni-modal__backdrop").click(position={"x": 3, "y": 3})
            samples = []
            while not session.exited.is_set():
                try:
                    samples = page.evaluate("window.__nativeCountdown")
                except Exception:
                    break
                check(time.perf_counter() - anchor <= 7, "Native refusal did not exit within the real-time deadline")
                time.sleep(0.04)
            exit_info = session.wait_exit()
            check(bool(samples), "No native countdown samples were captured")
            values = [sample["text"] for sample in samples]
            expected = [EXIT_PREFIX + str(value) for value in range(5, -1, -1)]
            check(values in [expected, expected[:-1]], "Native countdown skipped or repeated a visible second")
            intervals = [(samples[index]["at"] - samples[index - 1]["at"]) / 1000 for index in range(1, len(samples))]
            check(all(0.8 <= value <= 1.4 for value in intervals), "Native countdown ticks did not follow real seconds")
            first_at = anchor + (samples[0]["at"] - anchor_js) / 1000
            delay = exit_info["controllerMonotonicSeconds"] - first_at
            tolerance = (after_anchor - before_anchor) / 2 + 0.05
            check(5 - tolerance <= delay <= 6, "Native UI refusal used an accelerated or excessively delayed exit")
            check(sandbox.local_record() == {}, "Native refusal saved a consent bypass")
            observations = {
                "samples": samples, "tickIntervalsSeconds": intervals, "nativeProcessExitDelaySeconds": delay,
                "clockObservationToleranceSeconds": tolerance, "computedStyleBefore": before,
                "computedStyleHover": hover, "pngSha256Before": hashlib.sha256(png_before).hexdigest(),
                "pngSha256Hover": hashlib.sha256(png_hover).hexdigest(), "realBackendPendingExitObserved": True,
                "separateRefusalDialog": True, "trueDisabledButton": True,
                "zeroFrameObserved": values == expected,
                "zeroFrameNote": "A native process exit can close the window before its final zero frame is painted; no sample is invented.",
            }
            write_json(output / "real-clock-and-hover.json", observations)
        with NativeSession(playwright, sandbox, output / "restart-session", sessions) as session:
            expect(session.page.get_by_role("dialog", name=EULA_TITLE, exact=True)).to_be_visible()
            check(session.status()["needsEula"] and sandbox.local_record() == {}, "Refusal restart bypassed native EULA")
            session.page.screenshot(path=str(evidence_path(output / "refused-restart-eula.png", new=True)))
            if smoke:
                session.page.evaluate("window.__TAURI_INTERNALS__.invoke('window_close', {})")
                session.wait_exit()
            else:
                observations["restartBackendExit"] = backend_rejection_without_renderer(session)
        return observations

    case("native-refusal-real-clock-hover-and-restart", refusal)

    def backend_exit(output):
        sandbox = prepare_copy(source, evidence / "sandboxes/backend-exit", protocol, expected_sha256)
        with NativeSession(playwright, sandbox, output / "session", sessions) as session:
            expect(session.page.get_by_role("dialog", name=EULA_TITLE, exact=True)).to_be_visible()
            session.page.screenshot(path=str(evidence_path(output / "backend-only-pending-exit.png", new=True)))
            return backend_rejection_without_renderer(session)

    case("backend-owned-native-process-exit", backend_exit)
    return {
        "scope": "copied-native-release-window-and-real-backend",
        "workspace": str(ROOT), "nativeAcceptance": True,
        "nativeExecuted": any(item["nativeExecuted"] for item in sessions), "sourceExe": str(source),
        "sourceSha256": expected_sha256, "explicitReleaseHashAuthorized": True,
        "requiredVersion": REQUIRED_VERSION, "ipc": "live Tauri IPC; never simulated",
        "caseCount": len(results), "passedCount": sum(result["passed"] for result in results),
        "results": results, "sessions": sessions, "haltedForSafety": halted,
        "microsoftConfigurationCompared": any(item["case"] == "same-release-embedded-microsoft-public-config" and item["passed"] for item in results),
        "realAuthenticationExercised": False, "curseForgeAuthenticationVerified": False,
        "passed": bool(results) and all(result["passed"] for result in results) and not any(item["forcedCleanup"] or item["cleanupFailed"] for item in sessions),
        "omissions": ["No game process, startup arguments, authentication or download is exercised.", "The native portable fixture is already imported; pending-payload import behavior is not proven by this run.", "Environment paths and a job object are not an OS security sandbox; use a clean account or VM for native release acceptance."],
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--exe", required=True, type=Path, help="Explicit release binary; only a verified copy is ever launched")
    parser.add_argument("--evidence-dir", type=Path, help="New subdirectory inside tests/startup-acceptance/evidence")
    parser.add_argument("--expected-sha256", help="64-character release provenance guard; mandatory for --run")
    parser.add_argument("--smoke", action="store_true", help="Only essential first-run, restart, embedded configuration and refusal paths")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--run", action="store_true", help="Run only the hash-authorized copy on an isolated Windows desktop; privately compare embedded Microsoft public configuration")
    mode.add_argument("--prepare-only", action="store_true", help="Prepare fixtures only (the default); never launch any EXE or read local API configuration")
    args = parser.parse_args(argv)
    expected_sha256 = validated_sha256(args.expected_sha256, required=args.run)
    if args.run and os.name != "nt":
        raise SafetyError("--run requires Windows")
    source = executable_source(args.exe)
    if expected_sha256 is not None and file_hash(source) != expected_sha256:
        raise SafetyError("Source EXE does not match --expected-sha256")
    protocol = load_protocol()
    evidence = evidence_path(args.evidence_dir or EVIDENCE_ROOT / ("native-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + uuid.uuid4().hex[:8]), new=True)
    reference = load_microsoft_reference() if args.run else None
    evidence.mkdir(parents=True, exist_ok=False)
    if not args.run:
        sandbox = prepare_copy(source, evidence / "prepared-copy", protocol, expected_sha256)
        report = {"workspace": str(ROOT), "prepared": True, "nativeExecuted": False, "nativeAcceptance": False, "sourceExe": str(source), "copiedExe": str(sandbox.exe), "sha256": sandbox.sha256, "explicitHashAuthorized": expected_sha256 is not None, "microsoftConfigurationCompared": False, "realAuthenticationExercised": False, "nextMode": "--run requires an explicit hash, a new evidence directory and an isolated Windows desktop"}
        report_path = evidence / "native-prepared.json"
    else:
        with PrivateControllerEnvironment(evidence / "controller"):
            from playwright.sync_api import sync_playwright
            with sync_playwright() as playwright:
                report = run_native(playwright, source, evidence, protocol, expected_sha256, reference, smoke=args.smoke)
        report["controllerEnvironmentIsolated"] = True
        report["protocolLoggingDisabled"] = True
        report_path = evidence / "native-results.json"
    report["finishedAt"] = datetime.now(timezone.utc).isoformat()
    write_json(report_path, report)
    print("Native evidence: " + str(report_path), flush=True)
    return 0 if not args.run or report["passed"] else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        failure = safe_failure(error)
        print(failure["errorType"] + ": " + failure["error"], file=sys.stderr)
        sys.exit(1)
