# Findings and current-upstream porting notes

## Preserve the distinction between three boot paths

1. A resident profile maps a prepared image into DRAM before execution. It is
   useful for fast menu tests but does not prove ROM/SD loading.
2. Reference-ROM A starts at `0xfff00000` and boots the original NOR image.
3. Reference-ROM B starts at the same address, reads FFSYS/FFBOOT from virtual
   FAT32 through the guest SD controller, remaps DDR and enters the SD image.

The two ROM profiles differ only in a declared laboratory contact. It is not
a mapped X-Pro2 button. DDR training success, settling time, SD transfer timing,
virtual lens responses and the black sensor are declared model inputs. Their
functional behavior is not a measurement of a real body or XF lens.

## Specific fixes worth carrying forward

- **Boot wiring:** expose Boot contacts as logic-level resources and connect SD
  reset/interrupt gating through the same bank as normal reset gates. Do not
  synthesize RAM writes or skip guest instructions to satisfy startup.
- **DDR:** model a scheduled training completion, including retry/success status
  and checkpoint deadlines. Do not fabricate real training measurements.
- **Input:** use actual declared controls/ADC contacts. A GPIO signal supported
  by an EXR panel API is not automatically constructible on XPP.
- **Display:** preserve the XPP bitmap mode/packing and foreground semantics.
  Copying X70 bitmap mode 5 produced the wrong X-Pro2 output; the verified
  X-Pro2 command uses mode 7 and low/high 16-bit coordinate/size fields.
- **NOR overlays:** a recorded prefetch fault can be normal demand loading.
  The original NOR path used callback `0x00ba8294`, trampoline `0x00a51020`
  and WaitLoad at `0x00ba872c`. The observed target `0x0258bdec` executed;
  return at `0x01269e48` had R0=0. Fatal handler `0x00057b24` was not entered.
  Do not clear CP15 fault registers or label all nonzero IFSR values as crashes.
- **WebAssembly:** increasing the main heap maximum alone is insufficient.
  JIT memory imports that still cap shared memory at 2 GiB fail to link a
  3 GiB host memory. The reference emitter accepts the wasm32 maximum.
- **Persistence:** OPACITY.BIN version 3 holds the two opacities, theme and guide.
  The tested SDK open mode did not create an absent file; error 146 occurred.
  Profiles seed the 24-byte file. This is not a general file-creation fix.
- **Snapshot reuse:** observer PC watches affect execution wiring. A cold
  reconstruction must create a fresh compatible Session and restore only
  durable media; do not bypass checkpoint topology validation.

## Feature implementation map

| Feature | Principal runtime state / implementation |
| --- | --- |
| FujiMods folder | Native group 72, page `0x00d3a828`, caption `0x7007`. |
| Menu/indicator opacity | Properties `0x45a` / `0x45b`, independent settings. |
| Theme | Property `0x45c`; five selections and bounded color/cache adapters. |
| Guide | Property `0x109`; owned module `0x000a9000..0x000b9000`, shared native selector. |
| Preferences | Versioned OPACITY.BIN; save after confirmed selector exit, not every render. |
| Rename | Text pointer `0x000a961c` points to UTF-16 FujiMods at `0x000a9be0`; code/schema unchanged. |

Startup stack exclusions are essential. Zero bytes outside recipe regions/BSS
are not automatically safe allocations: reserve the IRQ/abort/undefined/system/
supervisor stacks for both CPUs. `reference-generators/xpro2_patch_layout.py`
contains the address checks used by the feature generators.

## Integrate with current upstream in reviewable steps

The inventory in `data/upstream-port-inventory.generated.json` compares each historical
engine path with the current public base. A clean text merge is not an API or
build validation. Do not replace current Cargo manifests, generated facts,
board constructors or service APIs wholesale with the old patch.

1. Preserve existing upstream behavior; identify fixes already covered there.
2. Port missing XPP device behavior with its focused Rust tests.
3. Add a research configuration for X-Pro2 with explicit virtual-device limits.
4. Reproduce original NOR boot and the handled overlay path.
5. Add reference-ROM SD boot and its exact handoff hash check.
6. Bring up menus before the feature hooks; then port one feature at a time.
7. Generate declarations/catalog artifacts through upstream generators.
8. Run the current branch's required checks. Historical test counts are not
   a claim that the same suite has passed on the current architecture.

Physical firmware installation is a separate project. The per-body X-Pro2
boot-ROM and installation catalog fields remain unqualified; do not promote
an X-T2 family assumption into a verified X-Pro2 fact.
