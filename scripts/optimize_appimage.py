"""Losslessly repack an unsigned type-2 AppImage and record content/size evidence."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import tempfile


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest_file(path, algorithm="sha256"):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, algorithm).digest()


def layout(source):
    with source.open("rb") as stream:
        header = stream.read(64)
        require(header[:6] == b"\x7fELF\x02\x01" and header[8:11] == b"AI\x02", "Expected ELF64 type-2 AppImage")
        table = struct.unpack_from("<Q", header, 40)[0]
        size, count, strings_index = struct.unpack_from("<HHH", header, 58)
        require(size == 64 and 0 < count < 512 and strings_index < count, "Unsupported AppImage ELF layout")
        require(table + size * count < source.stat().st_size, "Truncated AppImage")
        stream.seek(table)
        sections = [struct.unpack("<IIQQQQIIQQ", stream.read(size)) for _ in range(count)]
        names_section = sections[strings_index]
        require(names_section[5] < 64 * 1024, "Oversized ELF string table")
        stream.seek(names_section[4])
        names = stream.read(names_section[5])
        sections_by_name = {}
        offset = table + size * count
        for section in sections:
            require(section[0] < len(names), "Invalid ELF section name")
            name = names[section[0]:].split(b"\0", 1)[0].decode("ascii")
            if section[1] != 8:  # NOBITS has no file payload.
                offset = max(offset, section[4] + section[5])
                sections_by_name[name] = (section[4], section[5])
        stream.seek(offset)
        superblock = stream.read(96)
        require(superblock[:4] == b"hsqs" and len(superblock) == 96, "SquashFS missing at ELF boundary")
        require(struct.unpack_from("<HH", superblock, 28) == (4, 0), "Unsupported SquashFS version")
        used = struct.unpack_from("<Q", superblock, 40)[0]
        require(96 <= used <= source.stat().st_size - offset, "Truncated SquashFS payload")
        require(offset < 16 * 1024 * 1024, "Oversized AppImage runtime")
        stream.seek(0)
        runtime = bytearray(stream.read(offset))
    for name in [".sha256_sig", ".sig_key"]:
        if name in sections_by_name:
            start, length = sections_by_name[name]
            require(not any(runtime[start:start + length]), "Refusing to invalidate an AppImage signature")
    require(not source.with_suffix(source.suffix + ".sig").exists(), "Optimize before generating updater signatures")
    md5 = sections_by_name.get(".digest_md5")
    if md5:
        require(md5[1] == 16, "Unsupported AppImage digest section")
    return offset, runtime, md5, {"compressorId": struct.unpack_from("<H", superblock, 20)[0],
                                  "blockBytes": struct.unpack_from("<I", superblock, 12)[0]}


def run(command):
    process = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=1200)
    require(process.returncode == 0, "AppImage compression or extraction tool failed")
    return process.stdout.decode("utf-8", errors="replace")


def inventory(root):
    result = {}
    for source in [root, *sorted(root.rglob("*"))]:
        info = source.lstat()
        relative = source.relative_to(root).as_posix()
        entry = {"mode": stat.S_IMODE(info.st_mode), "uid": info.st_uid, "gid": info.st_gid,
                 "mtimeNs": info.st_mtime_ns}
        if stat.S_ISREG(info.st_mode):
            entry.update(kind="file", bytes=info.st_size, sha256=digest_file(source).hex())
        elif stat.S_ISDIR(info.st_mode):
            entry["kind"] = "directory"
        elif stat.S_ISLNK(info.st_mode):
            entry.update(kind="symlink", link=os.readlink(source))
        else:
            raise ValueError("AppImage contains a special filesystem entry")
        entry["xattrs"] = {name: os.getxattr(source, name, follow_symlinks=False).hex()
                           for name in sorted(os.listxattr(source, follow_symlinks=False))}
        result[relative] = entry
    return result


def extract(source, offset, destination):
    run(["unsquashfs", "-no-progress", "-processors", "2", "-offset", str(offset), "-d", str(destination), str(source)])


def runtime_extract_verified(source, destination, expected):
    """Check the shipped runtime, not just the host's broader SquashFS support."""
    destination.mkdir()
    process = subprocess.run([str(source), "--appimage-extract"], cwd=destination,
                             env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                                  "HOME": str(destination), "TMPDIR": str(destination)},
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1200)
    if process.returncode != 0:
        return False
    decoded = destination / "squashfs-root"
    if not decoded.is_dir():
        return False
    actual = inventory(decoded)
    # AppImage extraction does not preserve mtimes/modes exactly; unsquashfs checks those separately.
    content = lambda entries: {name: {key: value for key, value in entry.items()
                                    if key in {"kind", "bytes", "sha256", "link"}}
                               for name, entry in entries.items()}
    require(content(actual) == content(expected), "AppImage runtime extraction changed file contents or links")
    require(all(actual[name]["mode"] & 0o100 for name, entry in expected.items()
                if entry["kind"] == "file" and entry["mode"] & 0o111),
            "AppImage runtime extraction lost executable permissions")
    return True



def optimize(source, output, evidence, replace=False):
    require(os.name == "posix" and shutil.which("unsquashfs") and shutil.which("mksquashfs"), "Linux squashfs-tools required")
    source = source.absolute()
    output = output.absolute()
    require(source.is_file() and not source.is_symlink(), "Regular AppImage input required")
    require(not evidence.exists(), "Refusing to overwrite compression evidence")
    require((replace and output == source) or (output != source and not output.exists()), "Refusing to overwrite output")
    offset, runtime, md5_section, original_format = layout(source)
    before_hash = digest_file(source).hex()
    before_size = source.stat().st_size
    report = {"appimageCompressionVerified": False, "sourceSha256": before_hash, "beforeBytes": before_size,
              "originalSquashfs": original_format, "attempts": [], "signedInputRejected": True}
    with tempfile.TemporaryDirectory(prefix="nickel-appimage-") as temporary:
        temporary = Path(temporary)
        original = temporary / "original"
        extract(source, offset, original)
        expected = inventory(original)
        require(expected.get("usr/bin/ni-launcher", {}).get("mode", 0) & 0o111, "Launcher executable missing")
        require(runtime_extract_verified(source, temporary / "runtime-original", expected),
                "Original AppImage runtime cannot extract its own payload")
        best = source
        best_size = before_size
        for name, options in [
            ("zstd-22-1MiB", ["-comp", "zstd", "-Xcompression-level", "22", "-b", "1048576"]),
            ("xz-1MiB-x86", ["-comp", "xz", "-Xbcj", "x86", "-b", "1048576"]),
        ]:
            squash = temporary / f"{name}.squashfs"
            run(["mksquashfs", str(original), str(squash), "-noappend", "-no-progress", "-processors", "2",
                 "-root-time", str(original.stat().st_mtime_ns // 1_000_000_000), *options])
            candidate = temporary / f"{name}.AppImage"
            prefix = bytearray(runtime)
            if md5_section:
                start, length = md5_section
                prefix[start:start + length] = b"\0" * length
            with candidate.open("wb") as writer, squash.open("rb") as reader:
                writer.write(prefix)
                shutil.copyfileobj(reader, writer)
            if md5_section:
                checksum = digest_file(candidate, "md5")
                with candidate.open("r+b") as writer:
                    writer.seek(md5_section[0])
                    writer.write(checksum)
            candidate.chmod(stat.S_IMODE(source.stat().st_mode) | 0o111)
            decoded = temporary / f"decoded-{name}"
            extract(candidate, offset, decoded)
            require(inventory(decoded) == expected, "Compression changed file contents, permissions, links or metadata")
            require(run([str(candidate), "--appimage-offset"]).strip() == str(offset), "AppImage runtime offset changed")
            compatible = runtime_extract_verified(candidate, temporary / f"runtime-{name}", expected)
            candidate_size = candidate.stat().st_size
            report["attempts"].append({"compression": name, "bytes": candidate_size,
                                        "allFileContentsPermissionsLinksAndXattrsMatch": True,
                                        "runtimeOffsetVerified": True, "runtimeExtractionVerified": compatible})
            if compatible and candidate_size < best_size:
                best, best_size = candidate, candidate_size
        require(digest_file(source).hex() == before_hash, "AppImage input changed during compression")
        output.parent.mkdir(parents=True, exist_ok=True)
        if output != source or best != source:
            with tempfile.NamedTemporaryFile(prefix=".nickel-appimage-", dir=output.parent, delete=False) as pending:
                pending_path = Path(pending.name)
                with best.open("rb") as reader:
                    shutil.copyfileobj(reader, pending)
            try:
                pending_path.chmod(stat.S_IMODE(source.stat().st_mode) | 0o111)
                require(digest_file(pending_path) == digest_file(best), "Compressed output copy mismatch")
                if replace:
                    os.replace(pending_path, output)
                else:
                    os.link(pending_path, output)
            finally:
                pending_path.unlink(missing_ok=True)
        selected_offset, selected_runtime, selected_md5, selected_format = layout(output)
        normalized_original = bytearray(runtime)
        if md5_section:
            start, length = md5_section
            normalized_original[start:start + length] = b"\0" * length
            selected_runtime[start:start + length] = b"\0" * length
        require(selected_runtime == normalized_original and selected_offset == offset, "AppImage runtime changed beyond its MD5 digest")
        report.update(appimageCompressionVerified=True, afterBytes=output.stat().st_size,
                      savedBytes=before_size - output.stat().st_size, reductionPercent=round((1 - output.stat().st_size / before_size) * 100, 3),
                      sha256=digest_file(output).hex(), selectedSquashfs=selected_format,
                      runtimeCodeUnchanged=True, runtimeExtractionVerified=True,
                      allFileContentsPermissionsLinksAndXattrsMatch=True, inputRetained=not replace, noFilesRemoved=True,
                      launcherSha256=expected["usr/bin/ni-launcher"]["sha256"], launcherBytes=expected["usr/bin/ni-launcher"]["bytes"],
                      fileCount=sum(value["kind"] == "file" for value in expected.values()))
        evidence.parent.mkdir(parents=True, exist_ok=True)
        with evidence.open("x", encoding="utf-8") as writer:
            json.dump(report, writer, indent=2)
            writer.write("\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--evidence", required=True, type=Path)
    parser.add_argument("--replace", action="store_true")
    args = parser.parse_args()
    try:
        output = args.input if args.replace else args.output
        require(output is not None, "Output required unless replacing input")
        report = optimize(args.input, output, args.evidence, args.replace)
        print(json.dumps(report))
    except Exception:
        print(json.dumps({"appimageCompressionVerified": False, "inputUnmodifiedOnValidationFailure": True}))
        raise SystemExit(1)


if __name__ == "__main__":
    main()
