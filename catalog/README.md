# Reference catalog

The catalog records reusable hardware and firmware facts, keyed by identity.
Each fact has one owner. Consumers resolve references by ID; documents never
store another document's file path. Input file locations belong to callers.

Each fact lives with the thing it describes. To place a fact, ask what would
have to differ in the world for its value to differ:

| The value differs with | Layer | Location |
| --- | --- | --- |
| an IP design used by several processors | IP | `ip/<id>.json` |
| the processor design | Processor | `soc/<id>/` |
| an attached chip, panel, sensor or lens | Part | `parts/<id>/` |
| the circuit board | Body | `bodies/<id>/` |
| the firmware build | Firmware | `firmware/<body>/<version>/` |
| our emulator's choices, not the hardware | Emulation | `../configurations/emulator/processors/<soc>.json` |
| one run: selection, start inputs, overlays | Run configuration | `../configurations/emulator/<id>/` |

A fact is recorded in exactly one place and referenced by ID everywhere else,
even where an earlier layout happened to hold a copy. A stand-in for an
unmeasured value (a latency, a conversion time) belongs to the device it
stands in for, marked as a stand-in, not to the emulation layer.

## IP

An IP record describes a block design that several processors contain, such
as an interrupt controller, an SD host, an I2C or DMA controller or a GPIO
bank: its device model, its register layout and the parameters an instance
must supply. A processor block that contains the IP names it, and records only
what differs per instance: window, interrupts, clock and instance parameters.
A register layout is written once, in the IP record, however many processors
use it.

## Processors

`soc/<id>/soc.json` holds the processor's cores, memory regions, address and
DMA mappings, decoders, clocks and boot ROM behavior. Each core holds its own
architecture, CPU model, identification registers, caches, reset state,
affinity and timer interrupt lines. An alias of DRAM or another region at a
second address is an address mapping, not a block.

Each hardware block is one file, `soc/<id>/blocks/<block-id>.json`, and is the
only place its facts appear:

| Field | Holds |
| --- | --- |
| `id`, `kind`, `base`, `size` | Identity and the register window |
| `ip` | The IP record the block instantiates, when it is shared |
| `clock_id` | The clock domain that drives it |
| `views` | Per-core addresses where a core sees the block somewhere other than `base` |
| `interrupts` | Each named output, the interrupt-controller line it raises and, for fixed routing, the core |
| `device` | The device model, tagged by `model`, with its typed parameters |
| `registers` | The register layout of a block without an IP record: named registers at offsets, with width, reset value, access and named bit fields with their values |
| `evidence` | How the facts were established |

An interrupt controller's line count and partner interfaces are its block's
`device` parameters. A companion processor is a core, and its mailbox is a
block. A register that lives inside another block is an entry in that block's
layout; a device that uses it names the block and register. There are no
separate peripheral, interrupt-route, interrupt-input, interrupt-controller,
per-block parameter or register-map lists, and no processor-level window lists.

The board builder selects each block's implementation by its `device` model;
the model reads its parameters from `device` and its register behavior from
the IP's or block's layout. An untyped object is never a substitute for
either, and each `kind` allows a fixed set of models.

`evidence` is typed, not prose. Its `kind` is one of:

- `measured`: read on a camera;
- `located`: found in firmware code or tables;
- `inferred`: deduced from the behavior of other facts;
- `stand-in`: a declared input with no known hardware value.

It names the firmware IDs and code or data addresses the fact rests on.
Explanations longer than a sentence belong in research notes, not in the
record. Cores, mappings and body chips carry evidence the same way.

A processor fact found by reading firmware is generated into
`soc/<id>/<name>.generated.json` by a generator that runs over every
catalogued firmware of that processor and fails, naming them, when firmwares
disagree. The catalog loader joins generated processor facts to their blocks
and cores by ID; a field is either generated or hand-authored, never both.

## Parts and bodies

A part record describes a chip, panel, sensor or lens that several bodies use:
its behavior and the pins and bus addresses it answers on. A body record
(`bodies/<id>/body.json`) holds the processor ID and what this circuit board
connects: each chip as a part reference with its bus block and address, wiring
from processor pins and blocks to part pins, panels, DRAM size and regions
(including the power-on RAM contents when measured), NAND geometry, card
slots and power control. Processor blocks the board leaves unconnected are
listed under `absent_blocks`. `bodies/<id>/controls.json` holds buttons, dials
and switches: their positions, the pins or ADC channels they drive, including
command dials, and startup combinations.

A body changes a processor block only through `block_overrides`, which names a
block ID and replaces its `device` with another value of the same model or
replaces individual `registers` entries by name. The same processor behaves the
same in every body, so an override is rare: a per-body difference is usually an
attached part or an incomplete processor model, and is recorded there instead.

## Firmware

A firmware directory holds `firmware.json` (body, version, update and ROM
hashes, and the layout for decoding the update) and generated files only, one
per generator. It declares no device windows, device models, register behavior
or stubs. Firmware identity includes its body and version; firmware IDs are
selectors, never file locations.

## Emulation

`../configurations/emulator/processors/<soc>.json` holds what the emulator
chooses for a processor rather than what the hardware is: execution engines,
the CPU model standing in for an unavailable one, instruction and tick rates
and the runtime ABI. Run configurations select it with the hardware records;
it holds no hardware facts.

Mods read register addresses and firmware numbers from C headers generated
from these records, never from hand-copied constants.

## Writing records

Hand-authored records and generated records never share a file. Hand-authored
records have one canonical formatting, written by the catalog formatter; CI
fails on a record that differs from its formatted form, so machine edits and
hand edits produce the same bytes.

Catalog validation refuses:

- a firmware record that declares a device window, device model or stub;
- a body that redeclares a processor block instead of overriding its typed
  fields;
- a field set both by hand and by a generator;
- a register layout on a block whose IP record already has one;
- a block whose `device` model its `kind` does not allow;
- a block without `evidence`.

## Board construction

`fujimod-emulator-board-graph` assembles every processor's board from its
processor, body and firmware records, selecting each block's device model by
its `kind`. Callers supply only the update file, an optional boot ROM and
persistent images, and a run configuration; they never supply register
addresses, device lists or wiring. `fujimod-emulator-boot` opens every
processor as the same `Camera`, with the same session, checkpoint and control
interface.

`firmware-versions.generated.json` maps service-interface product/internal
versions to public release versions and official update pages. Each mapping is
qualified by its update's SHA-256. The map covers known updates independently
of hardware, emulation and bulk-read profiles and is exposed by
`ReferenceCatalog::firmware_versions` and JavaScript's `catalog.firmwareVersions()`.
Bulk profiles are keyed by that release identity and can be published without a
body or execution profile. They are exposed by `ReferenceCatalog::bulk` and
JavaScript's `catalog.bulkProfiles()`.
Native internal versions use BCD decimal notation. LEN builds retain identifiers
such as `C1234-vA0`; HCE versions include their revision, such as `2.0006`.
Internal versions are ordered only within the same product and representation;
catalog validation rejects decreasing or incomparable internal identities across
known public releases.

A body directory may contain only `controls.json`, without `body.json` or a
hardware definition. `ReferenceCatalog::physical_controls` retrieves these
standalone manuals. Documented positions and pushability remain available
independently of executable behavior. Unlocated input bindings, assertion
levels, spring-return behavior and analog resolution are null. Controls-only
documents validate their own identities and manual declarations; connections
or located bindings require the corresponding hardware records.

An optional `boot` map in `controls.json` records confirmed `sd`,
`firmware_update` and `service` startup combinations. Each entry contains
`hold` inputs and an optional `usb` cable condition. Held IDs use the existing
control action names, with an optional shutter `stage`.

Run configurations are executable preparation policy outside the reference
catalog. A configuration selects immutable reference identities and explicit
external inputs. REC with USB, service sessions and SD boot are small typed
overlays on a base configuration. An overlay changes declared connection/start
inputs or selects an implementation; it does not duplicate a body or firmware,
rewrite reference facts, or introduce a general patch/workflow language.

Firmware update files, real boot ROM dumps and optional persistent images are
supplied by explicit caller paths and verified against the selected identities.
No reference record locates or downloads those inputs. A real ROM reset entry
remains distinct from a loader or SD entry.

Rust serde structs in `fujimod-catalog` are the source of truth for SoC, part,
body, firmware, run-configuration and overlay documents. They deny unknown
fields, and their doc comments become descriptions in schemas generated with
schemars. Catalog schemas are generated, never authored separately.

CI checks committed schemas against newly generated output, validates every
document, and checks identity uniqueness and cross-layer references without
network resolution. Schema IDs are stable URIs resolved to this checkout.
TypeScript and Python catalog binding types are generated from the same Rust
definitions. Device/model variants have explicit typed fields checked again by
their constructors; an untyped object is not a substitute for a schema.

Catalog resolution is shared by Rust and the language bindings. Applications
select IDs and supply inputs; they do not reconstruct paths or maintain parallel
copies of facts. Changes migrate all consumers together and remove superseded
layouts, selectors and duplicated mode records.

Catalog organization does not change Rust module paths. The `exr2`, `xp4` and
`xp5` hardware modules remain at their existing crate roots.
