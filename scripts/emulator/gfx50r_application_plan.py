"""Build a loader-start application plan for the GFX 50R 3.11 emulator run.

Usage: python3 scripts/emulator/gfx50r_application_plan.py GXUP0002.DAT OUTDIR

Writes, from the official update file:

- ``flash.bin``: the update bytes from offset 604, XOR 0xff, padded with 0xff
  to 64 MiB (the first 32 KiB match the catalog's flash control digest);
- ``loader.bin``: the 256 KiB stage-2 loader at update offset 131676, linked at
  address 0 with its entry at 0x100;
- ``stub.bin`` and ``plan.json``.

Core 1 must not start at the same instant as core 0. Core 0 clears the loader's
BSS (0xf1a0..0x1ba80), which holds the boot handoff that core 1 fills in,
including the power-on cause at +0x140; started together, core 0 wipes the cause
core 1 has just written and the firmware powers itself off at its boot-reason
check. Core 1 therefore runs a short delay loop at 0x800000 (``stub.bin``) and
then jumps to the loader entry. The delay length is an assumption, not measured.

Then: ``fujimod-emulator boot --configuration gfx50r-311 --update GXUP0002.DAT
--application-plan OUTDIR/plan.json --seconds 40 --screenshot lcd.png``.
"""

import hashlib
import json
import struct
import sys
from pathlib import Path

UPDATE_SHA256 = "fdee7728"
FLASH_OFFSET = 604
FLASH_BYTES = 64 * 1024 * 1024
LOADER_OFFSET = 131676
LOADER_BYTES = 256 * 1024
LOADER_ENTRY = 0x100
STUB_ADDRESS = 0x800000
STUB_DELAY = 2_000_000
SVC_MODE_IRQ_FIQ_MASKED = 0xD3

# ldr r0, [pc, #8]; subs r0, r0, #1; bne -4; ldr pc, [pc]; .word delay; .word entry
STUB_WORDS = (0xE59F0008, 0xE2500001, 0x1AFFFFFD, 0xE59FF000, STUB_DELAY, LOADER_ENTRY)


def inverted(data: bytes) -> bytes:
    return bytes(byte ^ 0xFF for byte in data)


def build(update: bytes, out: Path) -> None:
    if not hashlib.sha256(update).hexdigest().startswith(UPDATE_SHA256):
        raise SystemExit("not the GFX 50R 3.11 GXUP0002.DAT")
    flash = inverted(update[FLASH_OFFSET:])
    flash += b"\xff" * (FLASH_BYTES - len(flash))
    loader = inverted(update[LOADER_OFFSET : LOADER_OFFSET + LOADER_BYTES])
    stub = struct.pack("<6I", *STUB_WORDS)
    out.mkdir(parents=True, exist_ok=True)
    (out / "flash.bin").write_bytes(flash)
    (out / "loader.bin").write_bytes(loader)
    (out / "stub.bin").write_bytes(stub)
    plan = {
        "configuration_id": "gfx50r-311",
        "flash": "flash.bin",
        "images": [
            {
                "address": 0,
                "file": "loader.bin",
                "sha256": hashlib.sha256(loader).hexdigest(),
            },
            {
                "address": STUB_ADDRESS,
                "file": "stub.bin",
                "sha256": hashlib.sha256(stub).hexdigest(),
            },
        ],
        "board": {
            "kind": "xpp",
            "registers": [
                [["Pc", LOADER_ENTRY], ["Pstate", SVC_MODE_IRQ_FIQ_MASKED]],
                [["Pc", STUB_ADDRESS], ["Pstate", SVC_MODE_IRQ_FIQ_MASKED]],
            ],
        },
    }
    (out / "plan.json").write_text(json.dumps(plan, indent=1) + "\n")


def main(argv: list[str]) -> None:
    if len(argv) != 3:
        raise SystemExit(__doc__)
    build(Path(argv[1]).read_bytes(), Path(argv[2]))


if __name__ == "__main__":
    main(sys.argv)
