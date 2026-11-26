# Reproducing the X-Pro2 laboratory

## 1. Obtain the three inputs locally

Create an ignored input directory outside this source handoff. Obtain files from
these primary sources; do not commit or publish the downloaded files:

| Input | Source | Required SHA-256 |
| --- | --- | --- |
| X-Pro2 5.11 `FWUP0008.DAT` | [Fujifilm support](https://fujifilm-x.com/global/support/download/firmware/cameras/x-pro2/) | `6f9792e53a776ee116732a6df4425b0f5393ad2f53655f573924afe9c61dc54d` |
| X70 reference `x70-110.fmpatch` | [FujiMod release 0.0.1](https://github.com/justjake/fujimod/releases/download/0.0.1/x70-110.fmpatch) | `ffdf9108f9ec49bd495d01990edbc91ed161f60549feb8ee08c4e483f1c89486` |
| X-T2 ROM attachment | [Issue #355](https://github.com/justjake/fujimod/issues/355), [attachment](https://github.com/user-attachments/files/34869039/xt2-441-bootrom.zip) | The 64 KiB member hashes to `38682a355f67eccd98989e812ff094e88c44cde1172c3d351c7f29c4c15d0876`; only the first 32 KiB are used. |

The required 32 KiB ROM prefix hashes to
`632445259ac679bf36281ea3f707b127759e0802bd507ba2b8d5b926492fd59f`.
`prepare.py` accepts either the public zip attachment or that exact prefix.
It never reads a camera. The extra bytes in the public attachment include
peripheral/SRAM contents and are discarded, not treated as immutable ROM.
This reference was acquired on X-T2 bodies; X-Pro2 identity remains unverified.

The X70 input supplies only the reference-owned module slices needed by the
laboratory recipe. The tool validates its container/program and extracts the
module from literal commands; it does not execute the X70 image or install it.
The derived module hash is
`c7413cecadedb8cf66282d0b403af6f6395c72dae1136ceb31266112867dc200`.

## 2. Prepare the historical engine

Use a dedicated scratch checkout, keeping this source handoff outside it.
For example, let `KIT` point to a copy of this directory and `LAB_REPO` point
to the scratch FujiMod checkout. Shell variables below are ordinary local paths.

```sh
git -C "$LAB_REPO" fetch https://github.com/trogadero/fujimod.git refs/heads/contrib/xpro2-engine-reference
python3 "$KIT/verify-engine-reference.py" --repo "$LAB_REPO"
git -C "$LAB_REPO" checkout --detach 136ae193fa3f2366d3a28dba334db427e97f1f62
git -C "$LAB_REPO" submodule update --init --recursive
cd "$LAB_REPO"
mise install
```

Follow that revision's README for JavaScript dependencies and toolchain setup.
The native examples use Rust, a C/C++ toolchain, CMake/Ninja, pkg-config and the
pinned Unicorn submodule. The input builder itself needs only Python 3.10+ and
the standard library. Do not copy an existing developer virtual environment.

```sh
cargo test -p fujimod-emulator-xpp --lib
cargo build --release -p fujimod-emulator-javascript \
  --example xpro2_rom_boot --example xpro2_checkpoint
```

## 3. Reconstruct the exact laboratory input

Use a new output directory for every preparation. Inputs are never overwritten.

```sh
python3 "$KIT/prepare.py" \
  --dat "$INPUTS/FWUP0008.DAT" \
  --reference-patch "$INPUTS/x70-110.fmpatch" \
  --rom "$INPUTS/xt2-441-bootrom.zip" \
  --out "$LAB_REPO/analysis/xpro2-reproduction"
```

The stock resident image must hash to
`ed400cde0abfb07bd54a9aeb9a99f1d993843c47d29f3c71732edd0ed0d5f172`.
The rebuilt FujiMods SD image must hash to
`09ef2a0f2719635d1b7597424035bcaae6c605e39b163946311e4854ee45ef56`.
A mismatch stops preparation. Output `inputs.json` records all identities.
`firmware.input` is a simulator container, **not an installable DAT**.

## 4. Execute the ROM paths

```sh
cd "$LAB_REPO"
XPRO2_ROM_BOOT_SOURCE=nor target/release/examples/xpro2_rom_boot \
  "$LAB_REPO" analysis/xpro2-reproduction/graph-A.json \
  analysis/xpro2-reproduction/firmware.input \
  analysis/xpro2-reproduction/boot-A 120000000
XPRO2_ROM_BOOT_SOURCE=sd target/release/examples/xpro2_rom_boot \
  "$LAB_REPO" analysis/xpro2-reproduction/graph-B.json \
  analysis/xpro2-reproduction/firmware.input \
  analysis/xpro2-reproduction/boot-B 360000000
```

Both start with blank DRAM and execute ROM instructions. At the first guest
PC=0 on B, the observer hashes all 40,235,008 bytes transferred by the guest;
it must equal the SD image hash above. It observes memory and does not replace
ROM instructions. A follows the NOR branch. Check the JSON reports, guest
frames and errors, not just process creation or an HTTP 200 response.

## 5. Replay menu navigation quickly

```sh
python3 "$KIT/replay-ui.py" --repo "$LAB_REPO" \
  --lab "$LAB_REPO/analysis/xpro2-reproduction"
```

This separate resident profile skips initial ROM/SD transfer for quick UI
iteration. It confirms language, leaves the date screen, opens SETUP, selects
the last folder, enters FujiMods, returns and re-enters. It checks the text
pointer, ten folder rows and preference error status. Inspect the saved guest
frames for visible spelling and clipping. This is not a replacement for step 4.

## 6. Optional browser build

From `bindings/javascript`, after installing the revision's pinned Emscripten
and Node toolchains:

```sh
node emulator/build.mts --incremental --rust-simd128 --lto thin \
  --opt-level 3 --codegen-units 16 --maximum-memory 3221225472
```

The standalone engine needs a 3 GiB maximum for these full profiles. The JIT
imports must accept the same shared-memory maximum. The historical local web
buttons, hardcoded server mounts and personal scene are intentionally excluded.
Wire the prepared profile through the applicable web runtime interface when
porting to current upstream; this handoff does not claim that wiring is done.

## 7. Repeat cheap checks first

```sh
python3 -m unittest discover -s "$KIT" -p 'test_*.py'
python3 "$KIT/audit-public.py"
python3 "$KIT/verify-engine-reference.py" --repo "$LAB_REPO"
```

Never reuse snapshots across different input hashes, device graphs or observer
wiring. A fresh-session cold test should restore only durable media, then let
the guest load preferences. Use 100 ms press/release contacts and the recorded
settle times; the 50 ms experiment lost inputs. Functional model timing is not
a camera performance or power benchmark.

To regenerate the checked-in sparse source from the local reconstructed outputs:

```sh
python3 "$KIT/generate-delta.py" \
  --stock "$LAB_REPO/analysis/xpro2-reproduction/stock-resident.bin" \
  --modified "$LAB_REPO/analysis/xpro2-reproduction/ffboot.bin" \
  --reference-patch "$INPUTS/x70-110.fmpatch" \
  --output /tmp/xpro2-delta-check.S
cmp "$KIT/legacy/firmware-delta.generated.S" /tmp/xpro2-delta-check.S
```

Use a new output filename. This generation/check cycle was exercised during
preparation; generated source is not edited by hand.
