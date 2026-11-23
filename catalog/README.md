# Reference catalog

The catalog records reusable hardware and firmware facts, keyed by identity.
Each fact has one owner. Consumers resolve references by ID; documents never
store another document's file path. Input file locations belong to callers.

| Layer | Location | Owns |
| --- | --- | --- |
| SoC | `soc/<id>/` | CPU/reset architecture, address/block maps, interrupt lines, clock domains and processor interconnects |
| Body | `bodies/<id>/` | SoC ID, NAND geometry, sensor, lens, controls/pins, display and physical wiring to SoC blocks |
| Firmware | `firmware/<body>/<version>/` | Body/version identity, update and ROM hashes, installation/decoding layout and generated facts consumed by published tools |
| Run configuration | `../configurations/emulator/<id>/` | Body/firmware selection, start inputs, execution mode, scene and small named overlays |

SoC and body records describe the hardware independently of an emulator.
Body records refer to SoC block IDs instead of copying block maps or IRQs.
Firmware records refer to their body ID instead of repeating physical wiring.
Reusable components live in `parts/<id>/` and are referenced by part ID, so
identical lens, sensor and control facts are not copied between bodies.
Firmware identity includes its body and version; existing explicit
firmware IDs remain selectors, never file locations.

## Processor blocks

Every processor uses the same record shape. A processor's hardware is one
`blocks` list, and each block is the only place its facts appear:

| Field | Holds |
| --- | --- |
| `id`, `kind`, `base`, `size` | Identity and the register window |
| `clock_id` | The clock domain that drives it |
| `interrupts` | Each named output and the interrupt-controller line it raises |
| `device` | Typed parameters of the device model for its `kind` |
| `registers` | Optional register layout: named registers at offsets, with width, reset value, access and named bit fields with their values |
| `evidence` | How the facts were established |

There are no separate peripheral, interrupt-input, per-block parameter or
register-map lists, and no per-profile window lists. A device model reads its
parameters from its block's `device` and its register behavior from its
block's `registers`; an untyped object is never a substitute for either.

`evidence` is typed, not prose. Its `kind` is one of:

- `measured`: read on a camera;
- `located`: found in firmware code or tables;
- `inferred`: deduced from the behavior of other facts;
- `stand-in`: a declared input with no known hardware value, such as a
  latency an emulator needs.

It names the firmware IDs and code or data addresses the fact rests on.
Explanations longer than a sentence belong in research notes, not in the
record.

## Bodies

A body record lists the chips on its buses, its wiring to processor blocks,
its panels, its DRAM and its controls. It adds to or changes a processor block
only through typed fields: `block_overrides` entries name a processor block ID
and replace individual `device` parameters or `registers` entries, merged field
by field over the processor's block. A body never redeclares a processor
block's window, and lists processor blocks the body leaves unconnected under
`absent_blocks`. Control and dial wiring, including command dials, lives in
`controls.json` connections, so every consumer reads one record.

## Firmware

A firmware directory holds its identity record and generated files only. It
declares no device windows, device models or register behavior. Facts that
agree across every firmware of a processor are generated once into
`soc/<id>/<name>.generated.json`; their generator runs over every catalogued
firmware of that processor and fails when they disagree, naming the
disagreeing firmwares.

Mods read register addresses and firmware numbers from C headers generated
from these records, never from hand-copied constants.

## Writing records

Hand-authored records and generated records never share a file. Hand-authored
records have one canonical formatting, written by the catalog formatter; CI
fails on a record that differs from its formatted form, so machine edits and
hand edits produce the same bytes.

Catalog validation refuses:

- a firmware record that declares a device window or device model;
- a body that redeclares a processor block instead of overriding its typed
  fields;
- a block whose `device` parameters do not match its `kind`;
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
