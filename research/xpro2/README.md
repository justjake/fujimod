# X-Pro2 5.11 research handoff

This directory preserves a reproducible X-Pro2 emulation experiment and the
information needed to continue it. It is a research contribution, not an
installable camera modification or a completed port to today's runtime APIs.


The [storage and shutter update](storage/README.md) documents the latest X-Pro2
5.11 experiments and how to reproduce the DATE FOLDERS probe.

## What is included

- A [Git branch with the historical engine commits](https://github.com/trogadero/fujimod/tree/contrib/xpro2-engine-reference), including
  XPP display, input, SD, clock/reset, DDR and reference-ROM boot support.
- Declarative NOR/SD laboratory profiles and a deterministic input builder.
- A sparse, readable patch recipe that reproduces the exact FujiMods SD image.
- Authored feature-generator references, validation summaries, known failures,
  a current-upstream port inventory and a physical-qualification checklist.

No DAT, ROM, memory snapshot, photograph, RAF scene, extracted upstream module,
compiled runtime, device serial number, calibration dump or private workspace
path is included. Required third-party inputs are supplied locally by the reader
and checked by hash. Reference-owned palettes/code/vector assets are read from
the upstream patch on the reader's machine, not redistributed here.

## Features developed in the laboratory

| Feature | Observed result |
| --- | --- |
| **FujiMods** | Last SETUP folder; earlier experiments called it F1. |
| Menu opacity | 25–100%; saved through the preference service. |
| Indicator opacity | 0–100%; independently saved. |
| Menu themes | Five selections; preference persistence exercised. |
| Framing guidelines | Ten selections, including off, grids, HD, golden ratio, four spirals and dynamic symmetry. |
| Reference-ROM dual boot | Original NOR path A and modified SD path B in the declared emulator. |

The original six connection settings remain in the reused SETUP group 72.
AF-S speed, phase-AF accuracy and energy use have **not** been improved or measured
on a physical camera. Touch-only Menu Tap is out of scope for the X-Pro2.

## Read next

1. [Reproduce the experiment](REPRODUCING.md).
2. [Understand the findings and integration work](PORTING.md).
3. [Review validation and its limits](VALIDATION.md).
4. [Continue physical qualification and feature work](NEXT-STEPS.md).
5. [Publish only the reviewed branch](PUBLISHING.md).

## Relationship to current upstream

The contribution branch carries this handoff on top of upstream. The three
[engine commits](https://github.com/trogadero/fujimod/compare/62e47c090eb8a65f4ffce48d93702673498a6419...contrib/xpro2-engine-reference)
start at the validated historical revision `62e47c090eb8`. The path-by-path
inventory in `data/upstream-port-inventory.generated.json` identifies remaining
integration work. Current upstream has changed the device and browser APIs
used by the historical checkout.

The sparse firmware recipe is the canonical byte-exact replay path. Files in
`reference-generators/` preserve readable feature algorithms; their original
intermediate-workspace dependencies are not required by `prepare.py`. They are
not an independently portable second build pipeline. Use `XPRO2_LAB_ROOT` only
when deliberately adapting those references.

## Provenance and license

This work combines local experiments with FujiMod and fffw research. It is not
a claim that all emulator changes are novel or solely authored here. Some fixes
are already present or have evolved independently upstream. Retain FujiMod's
GPL-2.0 license and existing notices; this handoff's original additions are
provided under the same license. Third-party input files retain their own
terms and are deliberately excluded. Public sources and input hashes are in
[REPRODUCING.md](REPRODUCING.md).
