"""Package and audit distribution-only portable archives for the Build matrix."""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import struct
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
MARKER = "nickel.launcher-portable.json"
MODPACK_MARKER = "nickel.portable.json"
TARGETS = {
    "x86_64-pc-windows-msvc": ("windows-x64", "Nickel.exe", ".zip"),
    "aarch64-apple-darwin": ("macos-arm64", "Nickel.app", ".tar.gz"),
    "x86_64-apple-darwin": ("macos-x64", "Nickel.app", ".tar.gz"),
    "x86_64-unknown-linux-gnu": ("linux-x64", "Nickel.AppImage", ".tar.gz"),
}
LICENSE_FILES = (
    "LICENSE", "LICENSE-ASSETS", "NOTICE.md", "src-tauri/vendor/minecraft-java-rs-core/LICENSE",
    "src/assets/fonts/NOTICE.md", "src/assets/credits/README.md",
)
PRIVATE_NAMES = {"secrets.local.json", "credentials.key", "accounts.json", "active_account.json", "portable-import.json", "pack.mrpack"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(filename):
    value = hashlib.sha256()
    with Path(filename).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def safe_name(name):
    path = PurePosixPath(name)
    require(name and "\\" not in name and not path.is_absolute() and
            all(part not in {"", ".", ".."} and ":" not in part and not any(ord(c) < 32 for c in part) for part in name.split("/")),
            "Unsafe portable archive path")
    require(not any(part.casefold() in PRIVATE_NAMES for part in path.parts), "Private runtime file in portable archive")
    return path


def license_sources(root):
    result = {name: root / name for name in LICENSE_FILES}
    for source in sorted((root / "LICENSES").rglob("*")):
        require(not source.is_symlink(), "Linked license input")
        if source.is_file():
            result[source.relative_to(root).as_posix()] = source
    require("LICENSES/dependencies-NOTICES.txt" in result, "Dependency notices are required")
    require(all(source.is_file() and source.stat().st_size > 0 for source in result.values()), "Missing portable license input")
    return result


def check_binary(source, target):
    require(source.is_file() and not source.is_symlink() and source.stat().st_size > 64, "Missing portable native binary")
    with source.open("rb") as stream:
        header = stream.read(64)
        if target == "x86_64-pc-windows-msvc":
            require(header[:2] == b"MZ", "Portable Windows input is not a PE executable")
            stream.seek(struct.unpack_from("<I", header, 60)[0])
            pe = stream.read(24)
            require(pe[:4] == b"PE\0\0" and struct.unpack_from("<H", pe, 4)[0] == 0x8664,
                    "Portable Windows input must be x64")
        elif target == "x86_64-unknown-linux-gnu":
            require(header[:4] == b"\x7fELF" and header[4:6] == b"\x02\x01" and header[8:11] == b"AI\x02" and
                    struct.unpack_from("<H", header, 18)[0] == 62, "Portable Linux input must be an x64 type-2 AppImage")
        else:
            architecture = 0x0100000C if target == "aarch64-apple-darwin" else 0x01000007
            require(header[:4] == b"\xcf\xfa\xed\xfe" and struct.unpack_from("<I", header, 4)[0] == architecture,
                    "Portable macOS input has the wrong Mach-O architecture")


def marker_for(target, version):
    require(target in TARGETS, "Unsupported release target")
    require(isinstance(version, str) and re.fullmatch(r"[A-Za-z0-9.+-]{1,128}", version), "Invalid release version")
    return {
        "schemaVersion": 1, "format": "nickel-launcher-portable", "version": version, "target": target,
        "launcher": TARGETS[target][1], "dataDirectory": "Nickel", "gameDirectory": ".minecraft",
    }


def tree_entries(root):
    entries = {}
    for current, directories, files in os.walk(root, followlinks=False):
        for name in sorted(directories + files):
            source = Path(current) / name
            relative = source.relative_to(root).as_posix()
            safe_name(relative)
            info = source.lstat()
            mode = stat.S_IMODE(info.st_mode)
            if stat.S_ISLNK(info.st_mode):
                link = os.readlink(source)
                require(not os.path.isabs(link) and source.resolve().is_relative_to(root.resolve()) and source.exists(),
                        "Portable symlink escapes or is broken")
                entries[relative] = {"kind": "symlink", "mode": mode, "link": link}
            elif stat.S_ISDIR(info.st_mode):
                entries[relative] = {"kind": "directory", "mode": mode}
            elif stat.S_ISREG(info.st_mode):
                entries[relative] = {"kind": "file", "mode": mode, "bytes": info.st_size, "sha256": sha256(source)}
            else:
                raise ValueError("Unsupported portable filesystem entry")
    return dict(sorted(entries.items()))


def archive_entries(archive):
    entries = {}
    prefix = None
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as stream:
            require(stream.testzip() is None, "Portable ZIP CRC mismatch")
            members = [(entry.filename.rstrip("/"), entry) for entry in stream.infolist()]
            for name, entry in members:
                safe_name(name)
                parts = PurePosixPath(name).parts
                prefix = prefix or parts[0]
                require(parts[0] == prefix and name not in entries, "Multiple roots or duplicate portable entries")
                if len(parts) == 1:
                    require(entry.is_dir(), "Portable root must be a directory")
                    continue
                relative = "/".join(parts[1:])
                require(relative not in entries, "Duplicate portable entry")
                mode = (entry.external_attr >> 16) & 0o7777
                if entry.is_dir():
                    entries[relative] = {"kind": "directory", "mode": mode}
                else:
                    require(stat.S_IFMT(entry.external_attr >> 16) in {0, stat.S_IFREG}, "Linked ZIP entry")
                    with stream.open(entry) as content:
                        digest = hashlib.file_digest(content, "sha256").hexdigest()
                    entries[relative] = {"kind": "file", "mode": mode, "bytes": entry.file_size, "sha256": digest}
    else:
        with tarfile.open(archive, "r:gz") as stream:
            for entry in stream:
                name = entry.name.rstrip("/")
                safe_name(name)
                parts = PurePosixPath(name).parts
                prefix = prefix or parts[0]
                require(parts[0] == prefix, "Multiple portable roots")
                if len(parts) == 1:
                    require(entry.isdir(), "Portable root must be a directory")
                    continue
                relative = "/".join(parts[1:])
                require(relative not in entries, "Duplicate portable entry")
                if entry.isdir():
                    entries[relative] = {"kind": "directory", "mode": entry.mode}
                elif entry.issym():
                    link = PurePosixPath(entry.linkname)
                    require(not link.is_absolute() and "\\" not in entry.linkname, "Unsafe portable symlink")
                    resolved = list(parts[:-1])
                    for part in link.parts:
                        if part == "..":
                            require(len(resolved) > 1, "Portable symlink escapes root")
                            resolved.pop()
                        elif part != ".":
                            resolved.append(part)
                    entries[relative] = {"kind": "symlink", "mode": entry.mode, "link": entry.linkname}
                elif entry.isfile():
                    with stream.extractfile(entry) as content:
                        digest = hashlib.file_digest(content, "sha256").hexdigest()
                    entries[relative] = {"kind": "file", "mode": entry.mode, "bytes": entry.size, "sha256": digest}
                else:
                    raise ValueError("Unsupported portable archive entry")
    require(prefix and entries, "Empty portable archive")
    return prefix, dict(sorted(entries.items()))


def read_archive_file(archive, name):
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as stream:
            return stream.read(name)
    with tarfile.open(archive, "r:gz") as stream:
        entry = stream.getmember(name)
        require(entry.isfile() and entry.size < 64 * 1024, "Invalid portable metadata entry")
        return stream.extractfile(entry).read()


def validate_archive(archive, root=ROOT, expected=None):
    prefix, entries = archive_entries(archive)
    marker = json.loads(read_archive_file(archive, f"{prefix}/{MARKER}"))
    require(marker == marker_for(marker.get("target"), marker.get("version")), "Portable marker contract mismatch")
    target = marker["target"]
    launcher = marker["launcher"]
    binary = f"{launcher}/Contents/MacOS/ni-launcher" if launcher.endswith(".app") else launcher
    require(entries.get(binary, {}).get("kind") == "file" and entries[binary]["bytes"] > 64, "Portable launcher missing")
    require(MODPACK_MARKER not in entries and all(entries.get(name, {}).get("kind") == "directory" for name in ["Nickel", ".minecraft"]), "Portable root contract mismatch")
    require(not any(name.startswith(("Nickel/", ".minecraft/")) for name in entries), "Portable data directories must ship empty")
    require("README-portable.txt" in entries and "SOURCE.txt" in entries, "Portable usage and source notices missing")
    for name, source in license_sources(root).items():
        require(entries.get(name, {}).get("sha256") == sha256(source), "Portable license content mismatch")
    if target != "x86_64-pc-windows-msvc":
        require(entries[binary]["mode"] & 0o111, "Portable executable permission lost")
    if expected is not None:
        require(entries == expected, "Portable archive changed file content, links, or permissions")
    return {
        "portableArchiveVerified": True, "target": target, "version": marker["version"],
        "archive": archive.name, "bytes": archive.stat().st_size, "sha256": sha256(archive),
        "licenseContentsMatch": True, "runtimeDataEmpty": True, "modpackBootstrapAbsent": True,
        "fileContentAndPermissionsMatch": expected is not None, "entries": entries,
    }


def write_archive(staged, destination, suffix):
    if suffix == ".zip":
        with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as stream:
            for source in [staged, *sorted(staged.rglob("*"))]:
                require(not source.is_symlink(), "ZIP input must not contain symlinks")
                stream.write(source, source.relative_to(staged.parent).as_posix())
    else:
        # Preserve the complete .app and its internal links; never rewrite a signed bundle.
        with destination.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0, compresslevel=9) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT, dereference=False) as stream:
                for source in [staged, *sorted(staged.rglob("*"))]:
                    name = source.relative_to(staged.parent).as_posix()
                    info = stream.gettarinfo(str(source), arcname=name)
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = 0
                    if info.isfile():
                        with source.open("rb") as content:
                            stream.addfile(info, content)
                    else:
                        stream.addfile(info)


def package(target, source, output, root=ROOT):
    version = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
    marker = marker_for(target, version)
    require(target == "x86_64-pc-windows-msvc" or os.name == "posix", "Unix portable packaging requires POSIX permissions")
    label, launcher, suffix = TARGETS[target]
    stem = f"Nickel_{version}_{label}_portable"
    output.mkdir(parents=True, exist_ok=True)
    destination = output / f"{stem}{suffix}"
    evidence = output / f"{stem}.json"
    require(not destination.exists() and not evidence.exists(), "Refusing to overwrite existing portable release output")
    source = source.absolute()
    if launcher.endswith(".app"):
        require(source.is_dir() and source.suffix == ".app" and not source.is_symlink(), "Complete macOS .app required")
        check_binary(source / "Contents/MacOS/ni-launcher", target)
        require((source / "Contents/Info.plist").is_file(), "macOS Info.plist missing")
    else:
        check_binary(source, target)
    with tempfile.TemporaryDirectory(prefix="nickel-portable-") as temporary:
        staged = Path(temporary) / stem
        staged.mkdir()
        if source.is_dir():
            shutil.copytree(source, staged / launcher, symlinks=True, copy_function=shutil.copy2)
            require(tree_entries(source) == tree_entries(staged / launcher), "macOS bundle changed while copying")
        else:
            shutil.copy2(source, staged / launcher)
            require(sha256(source) == sha256(staged / launcher), "Portable native binary changed while copying")
            if target != "x86_64-pc-windows-msvc":
                (staged / launcher).chmod(0o755)
        for name, original in license_sources(root).items():
            copied = staged / name
            copied.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(original, copied)
        (staged / "Nickel").mkdir()
        (staged / ".minecraft").mkdir()
        (staged / MARKER).write_text(json.dumps(marker, indent=2) + "\n", encoding="utf-8")
        (staged / "README-portable.txt").write_text(
            f"Nickel {version} portable ({label})\n\n"
            f"Extract the WHOLE archive into a writable folder, then run {launcher}.\n"
            "Keep nickel.launcher-portable.json beside the launcher. Nickel/ stores settings and\n"
            "encrypted accounts; .minecraft/ stores the default game directory. Move the whole\n"
            "folder together. This marker never installs a modpack and does not accept the EULA.\n"
            "No user accounts, credential key files, secrets.local.json, Java runtime or game data\n"
            "are included. The launcher may embed authorized public Microsoft OAuth configuration\n"
            "and a CurseForge API key. Protection raises extraction cost, not client-side secrecy.\n\n"
            "Windows: Microsoft WebView2 must already be installed.\n"
            "Linux: chmod +x Nickel.AppImage; WebKitGTK/GTK dependencies must be available.\n"
            "If FUSE is unavailable, use APPIMAGE_EXTRACT_AND_RUN=1 ./Nickel.AppImage.\n"
            "macOS: keep the complete .app beside the marker; do not modify the signed bundle.\n\n"
            "See LICENSE, LICENSE-ASSETS, NOTICE.md, LICENSES/ and SOURCE.txt.\n",
            encoding="utf-8")
        repository = json.loads((root / "package.json").read_text(encoding="utf-8"))["repository"]["url"]
        (staged / "SOURCE.txt").write_text(
            f"Nickel {version}\nCorresponding source: {repository.removesuffix('.git')}/tree/v{version}\n"
            "Build instructions: docs/development.md in the corresponding source archive.\n"
            "Include the locally maintained src-tauri/vendor/minecraft-java-rs-core source.\n"
            "Redistributors must provide corresponding source and build instructions under AGPL-3.0.\n",
            encoding="utf-8")
        expected = tree_entries(staged)
        pending = Path(temporary) / destination.name
        write_archive(staged, pending, suffix)
        report = validate_archive(pending, root, expected)
        # Publish only after auditing the archive; no broad bundle globs or build caches.
        with pending.open("rb") as reader, destination.open("xb") as writer:
            shutil.copyfileobj(reader, writer)
        require(sha256(destination) == report["sha256"], "Portable output integrity mismatch")
        with evidence.open("x", encoding="utf-8") as writer:
            json.dump(report, writer, indent=2, sort_keys=True)
            writer.write("\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--output", type=Path, default=ROOT / "src-tauri/target/portable")
    parser.add_argument("--verify", type=Path)
    args = parser.parse_args()
    try:
        if args.verify:
            report = validate_archive(args.verify)
        else:
            require(args.target and args.source, "Target and source are required")
            report = package(args.target, args.source, args.output)
        print(json.dumps({key: value for key, value in report.items() if key != "entries"}))
    except Exception:
        print(json.dumps({"portableArchiveVerified": False, "privateValuesWithheld": True}))
        raise SystemExit(1)


if __name__ == "__main__":
    main()
