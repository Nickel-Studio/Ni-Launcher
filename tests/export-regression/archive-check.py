"""Independently verify retained production export archives using Python zipfile."""
import argparse
from collections import Counter
import hashlib
import io
import json
import ntpath
from pathlib import Path, PurePosixPath
import re
import stat
import zipfile


PERSONAL_GROUPS = {
    "jeiPersonal", "emiPersonal", "patchouliPersonal", "screenshots", "maps",
    "saves", "schematics", "replays", "servers", "personal", "disabledMods",
}
PORTABLE_MARKER = "nowhere.portable.json"
GUIDE_FILE = "\u4f7f\u7528\u8bf4\u660e.txt"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def relative_path(value, label, allow_backslash=False):
    require(isinstance(value, str) and value, f"{label}: missing path")
    require(allow_backslash or "\\" not in value, f"{label}: backslash")
    value = value.replace("\\", "/")
    require(not ntpath.splitdrive(value)[0] and not value.startswith("/"), f"{label}: absolute path")
    require(not any(part in ("", ".", "..") for part in value.split("/")), f"{label}: unsafe component")
    require(not any(ord(char) < 32 for char in value), f"{label}: control character")
    return value


def descendant(root, value, label):
    relative = relative_path(value, label, allow_backslash=True)
    path = root.joinpath(*PurePosixPath(relative).parts)
    resolved = path.resolve()
    require(resolved.is_relative_to(root.resolve()), f"{label}: escaped fixture")
    return path


def check_zip_structure(archive, expected_names, label):
    entries = archive.infolist()
    names = [entry.filename for entry in entries]
    duplicates = [name for name, count in Counter(names).items() if count != 1]
    require(not duplicates, f"{label}: duplicate entries {duplicates}")
    require(set(names) == set(expected_names), f"{label}: unexpected or missing ZIP entries; missing={sorted(set(expected_names) - set(names))[:10]}, extra={sorted(set(names) - set(expected_names))[:10]}")
    for entry in entries:
        relative_path(entry.filename, f"{label}/{entry.filename}")
        require(not entry.is_dir(), f"{label}: unexpected directory entry")
        require(not stat.S_ISLNK(entry.external_attr >> 16), f"{label}: symlink entry")
        require(not entry.flag_bits & 1, f"{label}: encrypted entry")
    require(archive.testzip() is None, f"{label}: CRC failure")
    return names


def check_no_host_data(value, fixture_root, label):
    text = json.dumps(value, ensure_ascii=False).replace("\\\\", "/").replace("\\", "/").lower()
    root = str(fixture_root.resolve()).replace("\\", "/").lower()
    require(root not in text, f"{label}: fixture path leaked")
    require(not re.search(r"[a-z]:/|file:/|/users/|/home/|/tmp/|//server/", text), f"{label}: host path leaked")
    if isinstance(value, dict):
        forbidden = {"dir", "javaPath", "rootDirectory", "preLaunchCommand", "accessToken", "refreshToken", "clientSecret", "password"}
        require(not (forbidden & set(value)), f"{label}: sensitive request/settings fields")
        for key, item in value.items():
            check_no_host_data(item, fixture_root, f"{label}/{key}")
    elif isinstance(value, list):
        for index, item in enumerate(value):
            check_no_host_data(item, fixture_root, f"{label}/{index}")


def check_evidence(report_path):
    report = json.loads(report_path.read_bytes())
    root = report_path.parent
    require(report.get("schemaVersion") == 1, f"{report_path}: unknown schema")
    files = report["files"]
    require(len(files) == len(set(files)), f"{report_path}: duplicate selected files")
    require(len(files) == len({path.lower() for path in files}), f"{report_path}: case collision")
    for path in files:
        relative_path(path, "selected file")
        require(not path.startswith("@dir/"), "directory token entered snapshot payload")
    archive_path = descendant(root, report["archive"], "archive")
    snapshot_root = descendant(root, report["snapshotRoot"], "snapshot")
    payload_names = ["modrinth.index.json", *[f"overrides/{path}" for path in files]]
    outer_manifest = None
    outer_names = None
    with zipfile.ZipFile(archive_path) as container:
        if report["container"] == "portablezip":
            outer_manifest = json.loads(container.read(PORTABLE_MARKER))
            check_no_host_data(outer_manifest, root, "portable manifest")
            payload = relative_path(outer_manifest["payload"], "portable payload")
            require(payload == report["payload"], "reported nested payload differs from manifest")
            require(payload == f"{outer_manifest['dataDirectory']}/pack.mrpack", "portable payload not in relative data directory")
            for key in ("gameDirectory", "dataDirectory"):
                relative_path(outer_manifest[key], key)
            require(outer_manifest.get("launcher") is None and outer_manifest.get("personalization") is None, "unexpected optional outer content")
            outer_names = check_zip_structure(container, [PORTABLE_MARKER, GUIDE_FILE, payload], "portable outer ZIP")
            payload_bytes = container.read(payload)
            staged_payload = snapshot_root.parent / "portable-content" / Path(*PurePosixPath(payload).parts)
            require(payload_bytes == staged_payload.read_bytes(), "nested payload differs from frozen production payload")
            payload_buffer = io.BytesIO(payload_bytes)
            with zipfile.ZipFile(payload_buffer) as inner:
                manifest, comparisons, hashes, payload_size = check_payload(inner, payload_names, files, root, snapshot_root)
        else:
            require(report["container"] == "mrpack" and report["payload"] is None, "unknown container")
            manifest, comparisons, hashes, payload_size = check_payload(container, payload_names, files, root, snapshot_root)
    require(manifest["name"] == "Regression Pack", "unexpected pack identity")
    require(manifest["dependencies"]["minecraft"] == "1.20.1", "Minecraft dependency missing")
    require(manifest["formatVersion"] == 1 and manifest["game"] == "minecraft", "nonstandard mrpack manifest")
    require(manifest["files"] == [], "bundled fixture unexpectedly contains remote downloads")
    require(comparisons == report["byteComparisonCount"], "comparison count mismatch")
    snapshot_files = sorted(path.relative_to(snapshot_root).as_posix() for path in snapshot_root.rglob("*") if path.is_file())
    require(snapshot_files == sorted(files), "snapshot includes unselected physical files")
    preview_file = root / "preview.json"
    if preview_file.exists():
        preview_bytes = preview_file.read_bytes()
        preview = json.loads(preview_bytes)
        require(len(preview["files"]) == report["previewEntries"], "preview entry count mismatch")
        require(len(preview_bytes) == report["previewBytes"], "serialized preview size mismatch")
        require(all(not entry["selected"] for entry in preview["files"] if entry["group"] in PERSONAL_GROUPS), "personal content defaulted on")
        require(all(entry["kind"] == "directory" and entry["sizeBytes"] is None for entry in preview["files"]), "thousand-file preview was flattened")
    return {
        "evidence": str(report_path), "archive": str(archive_path),
        "requestFormat": report["requestFormat"], "container": report["container"],
        "previewEntries": report["previewEntries"], "previewBytes": report["previewBytes"],
        "selectedFiles": comparisons, "payloadEntries": len(payload_names),
        "payloadSourceBytes": payload_size, "archiveBytes": archive_path.stat().st_size,
        "archiveSha256": hashlib.sha256(archive_path.read_bytes()).hexdigest(),
        "payloadFilesSha256": hashes, "outerEntries": outer_names,
        "manifest": manifest, "outerManifest": outer_manifest,
    }


def check_payload(archive, expected_names, files, root, snapshot_root):
    check_zip_structure(archive, expected_names, "mrpack payload")
    manifest = json.loads(archive.read("modrinth.index.json"))
    check_no_host_data(manifest, root, "mrpack manifest")
    hashes = {}
    total = 0
    for relative in files:
        original = descendant(root, relative, "source").read_bytes()
        staged = descendant(snapshot_root, relative, "staged source").read_bytes()
        archived = archive.read(f"overrides/{relative}")
        require(original == staged == archived, f"byte mismatch: {relative}")
        hashes[relative] = hashlib.sha256(archived).hexdigest()
        total += len(archived)
    return manifest, len(files), hashes, total


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).parent / "backend-evidence")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--allow-partial", action="store_true", help="Do not require a real outer portable ZIP and thousand-file standard archive")
    args = parser.parse_args()
    reports = sorted(args.root.glob("nickel-export-regression-*/archive-evidence-*.json"))
    require(reports, "No retained backend archive evidence found")
    results = [check_evidence(path) for path in reports]
    if not args.allow_partial:
        require(any(item["container"] == "portablezip" and item["selectedFiles"] == 4805 for item in results), "Real production outer portable ZIP not verified")
        require(any(item["container"] == "mrpack" and item["requestFormat"] == "mrpack" and item["selectedFiles"] == 4805 for item in results), "Thousand-file standard mrpack not verified")
    report = {"status": "pass", "archivesVerified": len(results), "payloadByteComparisons": sum(item["selectedFiles"] for item in results), "archives": results}
    if args.output:
        require(not args.output.exists(), f"Refusing to replace existing evidence: {args.output}")
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n", encoding="utf-8")
    for item in results:
        print(f"{item['container']} {item['requestFormat']}: preview={item['previewEntries']} entries/{item['previewBytes']} bytes, selected={item['selectedFiles']}, payload entries={item['payloadEntries']}, archive bytes={item['archiveBytes']}; {item['archive']}")
        if item["outerEntries"]:
            print("  outer ZIP entries: " + json.dumps(item["outerEntries"], ensure_ascii=True))
    print(f"PASS: {len(results)} archives, {report['payloadByteComparisons']} source/snapshot/archive byte comparisons")


if __name__ == "__main__":
    main()
