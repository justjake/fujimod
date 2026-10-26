#!/usr/bin/env python3
"""Build and sign source-authored Hypervisor.framework CPU checks."""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import tempfile
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        parser.error("HVF CPU checks require macOS ARM64")
    crate = Path(__file__).resolve().parent
    target = Path(os.environ.get("CARGO_TARGET_DIR", crate.parents[1] / "target"))
    out = (args.out or target / "hvf-check").resolve()
    out.mkdir(parents=True, exist_ok=True)
    log = out / "build.log"
    with log.open("w") as stream:
        built = subprocess.run(
            [
                "cargo",
                "build",
                "--manifest-path",
                str(crate / "Cargo.toml"),
                "--locked",
                "--example",
                "check",
                "--message-format=json-render-diagnostics",
            ],
            cwd=crate,
            stdout=stream,
            stderr=subprocess.STDOUT,
        )
    if built.returncode:
        print(log.read_text()[-10000:])
        return built.returncode
    executable = None
    for line in log.read_text().splitlines():
        try:
            item = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            item.get("reason") == "compiler-artifact"
            and item.get("target", {}).get("name") == "check"
        ):
            executable = item.get("executable")
    if not executable:
        raise RuntimeError(f"Cargo did not report the check executable; see {log}")
    with tempfile.TemporaryDirectory(prefix="signed-", dir=out) as directory:
        path = Path(directory)
        binary = path / "check"
        # A fresh inode avoids cached macOS signatures of replaced binaries.
        shutil.copy2(executable, binary)
        entitlements = path / "entitlements.plist"
        entitlements.write_text("""<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>com.apple.security.hypervisor</key><true/></dict></plist>
""")
        subprocess.run(
            [
                "codesign",
                "--force",
                "--sign",
                "-",
                "--entitlements",
                str(entitlements),
                str(binary),
            ],
            check=True,
        )
        with (out / "hardware.log").open("w") as stream:
            result = subprocess.run(
                [str(binary)],
                cwd=crate,
                stdout=stream,
                stderr=subprocess.STDOUT,
                timeout=45,
            )
        print((out / "hardware.log").read_text(), end="")
        return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())
