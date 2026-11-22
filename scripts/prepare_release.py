"""Prepare an audited installer + portable upload set. Never uploads build caches."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from package_portable import ROOT, TARGETS, package, require, sha256, validate_archive


def exactly_one(paths, message):
    values = sorted(set(paths))
    require(len(values) == 1, message)
    return values[0]


def prepare(target, release, output, root=ROOT):
    require(target in TARGETS and release.name == "release", "Explicit native Release directory required")
    require(not output.exists(), "Refusing to overwrite existing release upload set")
    environment = os.environ.copy()
    # Presence alone is not usability: the official endpoints must accept the real configuration.
    process = subprocess.run(["node", str(root / "scripts/release-secrets.mjs"), "--remote"],
                             cwd=root, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
    require(process.returncode == 0, "Official release configuration preflight failed; values withheld")
    remote = json.loads(process.stdout)
    require(remote == {"releaseConfigurationComplete": True, "curseforgeRemoteCredentialAccepted": True,
                       "microsoftDeviceCodeClientAccepted": True}, "Incomplete official configuration evidence")
    bundle = release / "bundle"
    if "windows" in target:
        source = release / "ni-launcher.exe"
        installers = [exactly_one(bundle.glob("msi/*.msi"), "Exactly one MSI required"),
                      exactly_one(bundle.glob("nsis/*-setup.exe"), "Exactly one NSIS installer required")]
    elif "apple" in target:
        source = exactly_one(bundle.glob("macos/*.app"), "Exactly one complete macOS app required")
        installers = [exactly_one(bundle.glob("dmg/*.dmg"), "Exactly one DMG required")]
    else:
        source = exactly_one(bundle.glob("appimage/*.AppImage"), "Exactly one AppImage required")
        installers = [exactly_one(bundle.glob("deb/*.deb"), "Exactly one DEB required"), source]
    require(all(value.is_file() and not value.is_symlink() and value.stat().st_size > 0 for value in installers), "Missing installer input")
    require(len({value.name for value in installers}) == len(installers), "Installer names collide")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".nickel-release-", dir=output.parent) as temporary:
        staged = Path(temporary) / "upload"
        staged.mkdir()
        if "linux" in target:
            from optimize_appimage import extract, layout, optimize
            original = source
            source = staged / original.name
            compression = optimize(original, source, staged / "appimage-compression.json")
            installers = [value for value in installers if value != original] + [source]
            extracted = Path(temporary) / "native-appimage"
            extract(source, layout(source)[0], extracted)
            binary = extracted / "usr/bin/ni-launcher"
            require(sha256(binary) == compression["launcherSha256"], "AppImage launcher provenance mismatch")
        elif "apple" in target:
            # Scan the final signed bundle binary, not an unbundled pre-signing Cargo executable.
            binary = source / "Contents/MacOS/ni-launcher"
        else:
            binary = source
        for original in installers:
            copied = staged / original.name
            if copied != original:
                shutil.copy2(original, copied)
                require(sha256(original) == sha256(copied), "Installer copy changed content")
        report = package(target, source, staged, root)
        require(validate_archive(staged / report["archive"], root, report["entries"])["target"] == target, "Portable archive target mismatch")
        evidence = staged / "native-config-audit.json"
        process = subprocess.run(["node", str(root / "scripts/audit-release.mjs"), "--binary", str(binary),
                                  "--release-dir", str(release), "--evidence", str(evidence)],
                                 cwd=root, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
        require(process.returncode == 0 and evidence.is_file(), "Native credential/whitelist audit failed; values withheld")
        credentials = json.loads(evidence.read_text(encoding="utf-8"))
        require(credentials.get("releaseReadyForPublication") is True and credentials.get("sha256") == sha256(binary),
                "Incomplete native release configuration evidence")
        portable_binary = "Nickel.app/Contents/MacOS/ni-launcher" if "apple" in target else "Nickel.exe"
        if "linux" not in target:
            require(report["entries"][portable_binary]["sha256"] == credentials["sha256"], "Portable launcher differs from audited binary")
        (staged / "remote-config-audit.json").write_text(json.dumps({**remote, "completedAccountLoginVerified": False}, indent=2) + "\n", encoding="utf-8")
        allowed = {value.name for value in installers} | {report["archive"],
                   report["archive"].removesuffix(".tar.gz").removesuffix(".zip") + ".json",
                   "native-config-audit.json", "remote-config-audit.json"}
        if "linux" in target:
            allowed.add("appimage-compression.json")
        require({value.name for value in staged.iterdir()} == allowed, "Unexpected release upload file")
        version = report["version"]
        files = [{"name": value.name, "bytes": value.stat().st_size, "sha256": sha256(value)} for value in sorted(staged.iterdir())]
        manifest = {"releaseUploadSetVerified": True, "target": target, "version": version,
                    "portable": report["archive"], "installers": [value.name for value in installers], "files": files}
        (staged / "release-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        (staged / "SHA256SUMS.txt").write_text("".join(f"{entry['sha256']}  {entry['name']}\n" for entry in files), encoding="utf-8")
        # Nothing is exposed to upload-artifact until the complete set passes.
        os.rename(staged, output)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=TARGETS)
    parser.add_argument("--release-dir", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        result = prepare(args.target, args.release_dir.absolute(), args.output.absolute())
        print(json.dumps({key: result[key] for key in ["releaseUploadSetVerified", "target", "version", "portable"]}))
    except Exception:
        print(json.dumps({"releaseUploadSetVerified": False, "credentialValuesWithheld": True}))
        raise SystemExit(1)


if __name__ == "__main__":
    main()
