"""Independently inspect a retained synthetic error report with Python zipfile."""
import argparse
import hashlib
import json
import stat
from pathlib import Path
from zipfile import ZipFile


def inspect(archive):
    checks = []

    def check(name, condition):
        checks.append({"name": name, "passed": bool(condition)})
        if not condition:
            raise AssertionError(name)

    with ZipFile(archive) as zipped:
        names = zipped.namelist()
        expected = {
            "report.json", "READ-ME.txt", "failure.txt", "logs/latest.log",
            "logs/debug.log", "logs/nowhere-launcher.log.1",
            "crash-reports/crash-2026.txt",
        }
        check("exact source whitelist", set(names) == expected and len(names) == len(expected))
        check("CRC complete", zipped.testzip() is None)
        check("no path collisions", len({name.casefold() for name in names}) == len(names))
        check("relative safe paths", all(not name.startswith(("/", "\\")) and ".." not in Path(name).parts for name in names))
        check("no links or encrypted entries", all(not stat.S_ISLNK(info.external_attr >> 16) and not info.flag_bits & 1 for info in zipped.infolist()))
        latest = zipped.read("logs/latest.log").decode("utf-8")
        check("first and final log lines preserved", latest.startswith("BEGIN\n") and latest.endswith("END\n"))
        check("all twenty thousand ordinary lines preserved", latest.count("ordinary log line\n") == 20000)
        check("debug log complete", zipped.read("logs/debug.log") == b"all debug details\n")
        check("historical log complete", zipped.read("logs/nowhere-launcher.log.1") == b"previous session\n")
        crash = zipped.read("crash-reports/crash-2026.txt").decode("utf-8")
        check("crash stack retained", crash.startswith("full crash stack\n"))
        corpus = b"\n".join(zipped.read(name) for name in names)
        check("private fixture values removed", all(value not in corpus for value in [b"private-access-value", b"private-secret-value", b"private-password-value", b"PrivateName", b"do-not-export"]))
        check("private files excluded", not any(name.endswith(("accounts.json", "secrets.local.json")) for name in names))
        manifest = json.loads(zipped.read("report.json"))
        check("product release identity", manifest["product"] == "Nickel" and manifest["version"] == "1.0.2")
        check("complete manifest matches all collected sources", manifest["complete"] is True and set(manifest["files"]) == expected - {"report.json", "READ-ME.txt", "failure.txt"})
        check("manifest has no source host paths", not any(key in manifest for key in ["directory", "root", "account", "secrets"]))
        guide = zipped.read("READ-ME.txt").decode("utf-8")
        check("full ZIP help guidance", "ZIP" in guide and "\u622a\u56fe" in guide)
        check("failure reason retained", zipped.read("failure.txt") == b"OutOfMemoryError")
        return {
            "scope": "independent Python ZIP inspection; synthetic logs; not native IPC",
            "archive": str(archive), "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
            "archiveBytes": archive.stat().st_size, "uncompressedBytes": sum(info.file_size for info in zipped.infolist()),
            "entries": names, "ordinaryLines": 20000, "checks": checks, "passed": True,
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if args.output.exists():
        raise ValueError("Refusing to overwrite existing evidence")
    result = inspect(args.archive.resolve(strict=True))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, ensure_ascii=True, indent=2)
        stream.write("\n")
    print("PASS independent error report archive: " + str(args.output))


if __name__ == "__main__":
    main()
