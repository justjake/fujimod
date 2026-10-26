"""Build the Python extension and install it only after it loads.

Writing the extension in place would let a failed, partial or reduced-feature
build break every process that shares the checkout. This builds it with Cargo's
release profile (the profile every optimized local build shares), loads it in a
fresh interpreter, requires every default-feature module, and then renames it
over the installed file. A build that fails any step leaves the installed
extension untouched. The running environment gets the same `fujimod.pth`
source-path file an editable install writes.

    uv run --locked python scripts/python_extension.py
"""

import json
import os
import shutil
import site
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGE = ROOT / "bindings/python/python/fujimod"
NAME = "_native.abi3.so"
MODULES = ("catalog", "corpus", "emulator", "firmware", "recording", "remote", "usb")
CHECK = """
import importlib.util, sys
spec = importlib.util.spec_from_file_location('fujimod._native', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
missing = [name for name in sys.argv[2:] if not hasattr(module, name)]
if missing:
    raise SystemExit('extension lacks modules: ' + ', '.join(missing))
"""


def build() -> Path:
    """Build the extension library and return its path."""
    built = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            "bindings/python/module/Cargo.toml",
            "--lib",
            "--message-format=json-render-diagnostics",
        ],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    for line in built.stdout.splitlines():
        message = json.loads(line)
        if (
            message.get("reason") == "compiler-artifact"
            and "cdylib" in message["target"]["crate_types"]
        ):
            return Path(
                next(f for f in message["filenames"] if f.endswith((".dylib", ".so")))
            )
    raise SystemExit("cargo built no extension library")


def main() -> None:
    library = build()
    candidate = PACKAGE / f"._native.new-{os.getpid()}.abi3.so"
    try:
        shutil.copyfile(library, candidate)
        candidate.chmod(0o755)
        subprocess.run(
            [sys.executable, "-c", CHECK, str(candidate), *MODULES], check=True
        )
        os.replace(candidate, PACKAGE / NAME)
    finally:
        candidate.unlink(missing_ok=True)
    (Path(site.getsitepackages()[0]) / "fujimod.pth").write_text(
        str(PACKAGE.parent) + "\n"
    )
    print(f"Installed {PACKAGE / NAME}")


if __name__ == "__main__":
    main()
