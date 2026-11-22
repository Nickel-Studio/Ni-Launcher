"""Select the public GitHub Release asset set from a verified collect_release.py output.

collect_release.py keeps every per-target evidence file (release manifests, native and
remote configuration audits, AppImage compression proof) so the Nickel-complete-release
Actions artifact stays auditable. The public Release only carries installers, portable
archives, the tag's source archive and a SHA256SUMS.txt that lists exactly those files.
"""
import argparse
import json
import os
import re
import shutil
import tempfile
from pathlib import Path

from package_portable import ROOT, TARGETS, require, sha256

SUMS = "SHA256SUMS.txt"
# Allow-list rather than "drop *.json": a new evidence type must never leak into the
# Release, and a new binary type must fail loudly instead of silently disappearing.
PUBLIC_SUFFIXES = (".msi", "-setup.exe", ".dmg", ".deb", ".AppImage")
EVIDENCE_SUFFIX = ".json"


def read_sums(path):
    entries = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^/\\]+)", line)
        require(match is not None, "Invalid collected checksum line")
        require(match[2] not in entries, "Duplicate collected checksum entry")
        entries[match[2]] = match[1]
    return entries


def select(assets, source, output, root=ROOT):
    version = json.loads((root / "package.json").read_text(encoding="utf-8"))["version"]
    require(not output.exists(), "Refusing to overwrite public release output")
    require(assets.is_dir() and (assets / SUMS).is_file(), "Collected release assets missing")
    recorded = read_sums(assets / SUMS)
    names = {entry.name for entry in assets.iterdir()} - {SUMS}
    # The collector's checksum file is the chain of custody: every file it staged is
    # listed, so anything extra or missing means the directory changed after collection.
    require(set(recorded) == names, "Collected checksums do not match the asset set")
    portables = {f"Nickel_{version}_{label}_portable{suffix}" for label, _, suffix in TARGETS.values()}
    public, excluded = [], 0
    for name in sorted(names):
        path = assets / name
        require(path.is_file() and not path.is_symlink(), "Unexpected release asset type")
        if name in portables or (name.startswith(f"Nickel_{version}_") and name.endswith(PUBLIC_SUFFIXES)):
            require(sha256(path) == recorded[name], "Release asset changed after collection")
            public.append(path)
        else:
            require(name.endswith(EVIDENCE_SUFFIX), "Unclassified release asset")
            excluded += 1
    require(portables <= {path.name for path in public}, "Portable archive set incomplete")
    require(source.name == f"Nickel-{version}-source.zip" and source.is_file() and not source.is_symlink(),
            "Source archive missing or misnamed")
    public.append(source)
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".nickel-public-", dir=output.parent) as temporary:
        staged = Path(temporary) / "assets"
        staged.mkdir()
        for original in public:
            shutil.copy2(original, staged / original.name)
            require(sha256(original) == sha256(staged / original.name), "Release asset changed while staging")
        sums = "".join(f"{sha256(file)}  {file.name}\n" for file in sorted(staged.iterdir()))
        (staged / SUMS).write_text(sums, encoding="utf-8")
        os.rename(staged, output)
    return {"publicAssets": len(public), "evidenceExcluded": excluded, "version": version}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", required=True, type=Path)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(select(args.assets, args.source, args.output)))
    except Exception:
        print(json.dumps({"publicReleaseAssetsSelected": False, "uploadBlocked": True}))
        raise SystemExit(1)


if __name__ == "__main__":
    main()