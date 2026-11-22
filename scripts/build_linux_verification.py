"""Local WSL release build using the already-built frontend; not a publish gate."""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
CLI = Path(os.environ.get("NICKEL_TAURI_CLI", "/tmp/nickel-tauri-cli-2.12.4/node_modules/@tauri-apps/cli/tauri.js"))
TARGET = ROOT / "src-tauri/target/linux-verification"
LOG = TARGET / "build.log"
STATUS = TARGET / "build-status.json"


def main():
    if sys.platform != "linux" or not CLI.is_file() or not (ROOT / "dist/index.html").is_file():
        raise SystemExit("This verifier requires the isolated Linux Tauri CLI (set NICKEL_TAURI_CLI) and pnpm build output")
    environment = os.environ.copy()
    # PATH is inherited from the caller; never hardcode a developer's home directory.
    environment["CARGO_TARGET_DIR"] = str(TARGET)
    environment["CARGO_BUILD_JOBS"] = "2"
    environment["NICKEL_CHANNEL"] = "production"
    environment["NICKEL_REQUIRE_RELEASE_SECRETS"] = "0"
    environment["APPIMAGE_EXTRACT_AND_RUN"] = "1"
    environment["NO_STRIP"] = "1"
    environment.pop("RUSTC_WRAPPER", None)
    TARGET.mkdir(parents=True, exist_ok=True)
    # Reuse pnpm build from Windows without modifying the source config or shared node_modules.
    command = ["node", str(CLI), "build", "--bundles", "deb,appimage", "--ci", "--config",
               json.dumps({"build": {"beforeBuildCommand": ""}}), "--", "--locked", "--offline"]
    with LOG.open("w", encoding="utf-8") as log:
        process = subprocess.run(command, cwd=ROOT, env=environment, stdout=log, stderr=subprocess.STDOUT)
    result = {"nativeLinuxReleaseBuildPassed": process.returncode == 0,
              "publicationConfigurationGateEnabled": False, "realCurseforgeCredentialVerified": False}
    STATUS.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result))
    raise SystemExit(process.returncode)


if __name__ == "__main__":
    main()
