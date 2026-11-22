"""Validate all four staged target sets before assembling GitHub Release assets."""
import argparse
import os
import re
import tempfile
import json
from pathlib import Path
import shutil

from package_portable import ROOT, TARGETS, require, sha256, validate_archive


def collect(inputs, output, tag, root=ROOT):
    version = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
    require(tag == f"v{version}", "Release tag must match source version")
    require(not output.exists(), "Refusing to overwrite release asset output")
    sets = list(inputs.glob("*/release-manifest.json"))
    require(len(sets) == len(TARGETS), "All four target manifests are required before upload")
    require({entry for entry in inputs.iterdir()} == {source.parent for source in sets}, "Unexpected input artifact directory")
    targets, names = set(), set()
    pending = []
    for source in sets:
        require(source.is_file() and not source.is_symlink() and not source.parent.is_symlink(), "Linked release manifest")
        manifest = json.loads(source.read_text(encoding="utf-8"))
        target = manifest.get("target")
        require(target in TARGETS and target not in targets and manifest.get("version") == version and
                manifest.get("releaseUploadSetVerified") is True, "Invalid release target manifest")
        targets.add(target)
        directory = source.parent
        files = manifest.get("files")
        require(isinstance(files, list) and files, "Release file evidence missing")
        declared = set()
        for entry in files:
            require(isinstance(entry, dict) and set(entry) == {"name", "bytes", "sha256"}, "Invalid release file evidence")
            name = entry["name"]
            require(isinstance(name, str) and name not in {"", ".", "..", "release-manifest.json", "SHA256SUMS.txt"} and
                    Path(name).name == name and not re.search(r"[\\/:\x00-\x1f\x7f]", name) and name not in declared,
                    "Invalid or duplicate release filename")
            declared.add(name)
            candidate = directory / name
            require(type(entry["bytes"]) is int and entry["bytes"] > 0 and
                    isinstance(entry["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]) and
                    candidate.is_file() and not candidate.is_symlink() and candidate.stat().st_size == entry["bytes"] and
                    sha256(candidate) == entry["sha256"], "Release artifact hash mismatch")
        require({entry.name for entry in directory.iterdir()} == declared | {"release-manifest.json", "SHA256SUMS.txt"},
                "Unexpected artifact content")
        sums = directory / "SHA256SUMS.txt"
        require(sums.is_file() and not sums.is_symlink() and sums.read_text(encoding="utf-8") ==
                "".join(f"{entry['sha256']}  {entry['name']}\n" for entry in files), "Target checksum evidence mismatch")
        archive_name = manifest.get("portable")
        require(archive_name in declared and isinstance(archive_name, str), "Portable artifact evidence missing")
        proof_name = archive_name.removesuffix(".tar.gz").removesuffix(".zip") + ".json"
        require({proof_name, "native-config-audit.json", "remote-config-audit.json"} <= declared, "Required release evidence missing")
        proof = json.loads((directory / proof_name).read_text(encoding="utf-8"))
        require(isinstance(proof.get("entries"), dict) and proof.get("portableArchiveVerified") is True, "Portable inventory evidence missing")
        portable = validate_archive(directory / archive_name, root, proof["entries"])
        require(portable["target"] == target and portable["version"] == version and
                portable["sha256"] == proof.get("sha256") and portable["bytes"] == proof.get("bytes"), "Portable target evidence mismatch")
        audit = json.loads((directory / "native-config-audit.json").read_text(encoding="utf-8"))
        require(all(audit.get(name) is True for name in ["actualNativeExecutableScanned", "releaseConfigurationComplete",
                "embeddedEnvelopeMatchesAuthorizedWhitelist", "curseforgeLiteralAbsent", "microsoftClientLiteralAbsent",
                "legacyPlaintextBuildOutputAbsent", "releaseReadyForPublication"]), "Native configuration audit is incomplete")
        remote = json.loads((directory / "remote-config-audit.json").read_text(encoding="utf-8"))
        require(all(remote.get(name) is True for name in ["releaseConfigurationComplete", "curseforgeRemoteCredentialAccepted",
                "microsoftDeviceCodeClientAccepted"]), "Official configuration acceptance evidence missing")
        installers = manifest.get("installers")
        require(isinstance(installers, list) and len(set(installers)) == len(installers) and set(installers) <= declared,
                "Installer evidence missing or duplicate")
        if "windows" in target:
            require(len(installers) == 2 and sum(name.endswith(".msi") for name in installers) == 1 and
                    sum(name.endswith("-setup.exe") for name in installers) == 1, "Windows MSI and NSIS required")
        elif "apple" in target:
            require(len(installers) == 1 and installers[0].endswith(".dmg"), "macOS DMG required")
        else:
            require(len(installers) == 2 and sum(name.endswith(".deb") for name in installers) == 1 and
                    sum(name.endswith(".AppImage") for name in installers) == 1, "Linux DEB and AppImage required")
        launcher = "Nickel.app/Contents/MacOS/ni-launcher" if "apple" in target else "Nickel.exe"
        if "linux" in target:
            require("appimage-compression.json" in declared, "AppImage compression evidence missing")
            compression = json.loads((directory / "appimage-compression.json").read_text(encoding="utf-8"))
            require(all(compression.get(name) is True for name in ["appimageCompressionVerified", "noFilesRemoved",
                    "runtimeCodeUnchanged", "runtimeExtractionVerified", "allFileContentsPermissionsLinksAndXattrsMatch"]),
                    "AppImage compression evidence incomplete")
            appimage = directory / next(name for name in installers if name.endswith(".AppImage"))
            require(sha256(appimage) == compression.get("sha256") and appimage.stat().st_size == compression.get("afterBytes") and
                    0 < compression["afterBytes"] <= compression["beforeBytes"] and
                    portable["entries"]["Nickel.AppImage"]["sha256"] == compression["sha256"] and
                    audit.get("sha256") == compression.get("launcherSha256") and audit.get("bytes") == compression.get("launcherBytes"),
                    "AppImage compression or portable provenance mismatch")
        else:
            require(portable["entries"][launcher]["sha256"] == audit.get("sha256") and
                    portable["entries"][launcher]["bytes"] == audit.get("bytes"), "Portable launcher is not the audited binary")
        expected = set(installers) | {archive_name, proof_name, "native-config-audit.json", "remote-config-audit.json"}
        if "linux" in target:
            expected.add("appimage-compression.json")
        require(declared == expected, "Unexpected declared release file")
        label = TARGETS[target][0]
        for original in [*(directory / entry["name"] for entry in files), source]:
            name = original.name if original.suffix != ".json" else f"{label}-{original.name}"
            require(name not in names, "Release asset names collide")
            names.add(name)
            pending.append((original, name))
    require(targets == set(TARGETS), "Release target set incomplete")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".nickel-collect-", dir=output.parent) as temporary:
        staged = Path(temporary) / "assets"
        staged.mkdir()
        for original, name in pending:
            shutil.copy2(original, staged / name)
            require(sha256(original) == sha256(staged / name), "Release asset changed while staging")
        sums = "".join(f"{sha256(file)}  {file.name}\n" for file in sorted(staged.iterdir()))
        (staged / "SHA256SUMS.txt").write_text(sums, encoding="utf-8")
        os.rename(staged, output)
    return {"allFourReleaseTargetsVerified": True, "portableForEveryTarget": True, "version": version, "assets": len(pending)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--tag", required=True)
    args = parser.parse_args()
    try:
        print(json.dumps(collect(args.input, args.output, args.tag)))
    except Exception:
        print(json.dumps({"allFourReleaseTargetsVerified": False, "uploadBlocked": True}))
        raise SystemExit(1)


if __name__ == "__main__":
    main()
