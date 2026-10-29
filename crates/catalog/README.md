# FujiMod catalogs

The catalog crates load validated firmware, camera, service and emulator
catalog data without USB or execution dependencies. Each domain is its own
crate, so a consumer rebuilds only when the documents it reads change:

- `fujimod-catalog-identity`: the identifier and digest checks every document
  uses.
- `fujimod-catalog-assets`: the content-addressed documents and assets a
  caller's provider supplies, and the file provider that reads them.
- `fujimod-catalog-firmware-identity`: firmware identities, which service and
  service workflow contracts qualify against.
- `fujimod-catalog-workflows`: service workflow contracts.
- `fujimod-catalog-service`: App80 service definitions, checked against a
  firmware identity.
- `fujimod-catalog-construction`: exported device construction topology.
- `fujimod-catalog-controls`: physical controls, firmware control IDs and, in
  `facts`, the generated firmware input and observation facts they use.
- `fujimod-catalog-remote`: the firmware layouts and build declarations remote
  control consumes, decoded from the reference catalog's stored JSON documents.
- `fujimod-catalog-settings`: firmware settings layouts and, in
  `persistent_inputs`, the persistent inputs a run supplies.
- `fujimod-catalog-bulk`: stock diagnostic bulk-read facts.
- `fujimod-catalog` (this crate): the bundled reference catalog of SoCs,
  parts, bodies and firmware and their hardware profiles. It keeps the control
  and remote documents as JSON values so edits to their typed decoders do not
  rebuild the reference catalog.
- `fujimod-catalog-configuration`: the bundled emulator run configurations,
  overlays and their hardware, firmware and native inputs. It does not depend
  on the reference catalog.
- `fujimod-catalog-resolution`: run configurations resolved against the
  reference catalog, with their overlays applied and every block, connection,
  part and profile identity they use checked against the selected hardware.
- `fujimod-catalog-schemas`: the JSON Schema generator for every catalog
  document.

The facade exposes `fujimod-catalog` as `fujimod::catalog`,
`fujimod-catalog-configuration` as `fujimod::catalog_configuration` and
`fujimod-catalog-resolution` as `fujimod::catalog_resolution`; enabling
`app80` exposes `fujimod-catalog-service` as `fujimod::catalog_service`,
`workflows` exposes `fujimod-catalog-workflows` as `fujimod::catalog_workflows`, and
both expose `fujimod-catalog-firmware-identity` as
`fujimod::catalog_firmware_identity`. Definitions stay in
JSON rather than becoming another set of Rust constants. Callers supply the
selected identity and SHA-256 when loading a definition. Matching those values
qualifies the data; identifying a connected camera and checking execution
eligibility remain responsibilities of its workflow.

The first schema is `fujimod_catalog_service::Definition` for App80. It contains firmware
identity, an explicit ff80 mailbox location/limit, and the ordered service table.
Each entry preserves its command byte, label, callback-kind word, handler and
parameter-table addresses, mode availability and table-declared transition.
Duplicate command bytes remain separate rows in firmware order. A mode rule
can be reviewed, disabled, or explicitly unreviewed. Unreviewed rows are visible
but cannot be selected for execution through this catalog.

`Codec` selects an implemented argument/result layout. Its slot array gives
zero-based positions within the 32-byte argument mailbox; invalid, overlapping
or incomplete layouts are rejected. Labels are display text and never select
behavior. `Research` retains a command whose argument semantics have not been
qualified for a typed codec. Unknown callback-kind words remain data and never
become host completion instructions.

The JSON schema is version 1. It requires:

- `firmware`: `id`, `model`, `version`, `sha256`;
- `mailbox`: `page`, `offset`, `configuration_limit`;
- `entries`: `code`, `label`, `codec`, `slots`, `enabled_modes`, `next_mode`,
  `firmware_kind`, `handler`, `parameter_table`;
- optional `source`: `format` (`dispatch24` or `declarations12`), `records`,
  `table`, `table_sha256`, `dispatcher`.

`enabled_modes` is a nonzero mask, zero for a disabled row, or null for an
unreviewed rule. The public API represents those cases with `ModeAvailability`.
`next_mode` is the firmware table's pre-dispatch update, represented by
`ModeTransition`; handlers can have additional effects. All numeric JSON fields
are unsigned quantities. No body, command number, mailbox or firmware is chosen
by default. Callers supply definitions in this schema to USB tooling, emulators
and SDK generation.

Remote control consumes `fujimod_catalog_remote::Facts`: `PhysicalControls`,
`FirmwareControlIds`, `FirmwareFacts`, and an optional `BuildMenuManifest` for
the installed build. `Facts::from_selection` decodes and validates the stored
documents against the resolved selection. Firmware records expose remote
inputs through `generated_facts.remote`; stock operation requires no build
manifest.

Control documents own debug key names, physical bindings, scanner witnesses,
and electrical predicates. ADC intervals explicitly distinguish literal sample
bounds from offsets into loaded configuration bytes.

`FirmwareFacts` declares readiness
addresses, menu node/row/cursor layouts and root routes, scanout, and text/glyph
resource layouts. Each section has a generated JSON Schema. These are layouts
and roots: consumers read menu rows, captions and cursor values from live RAM.
`BuildMenuManifest` binds owner identities, labels, placement, native
keys, surfaces, stable named options, and compiler-derived observation metadata
to hashed build artifacts. Open catalogs can leave options unnamed; consumers
must refuse an unresolved named option instead of inventing a name.

Schemas are generated from these Rust types with
`cargo run -p fujimod-catalog-schemas --features schema --bin catalog-schemas -- --write`.
New remote schema filenames include `.generated`; `--check` verifies committed
copies. Only the `schema` feature compiles the JSON Schema implementations, so
emulator and binding builds skip them; the `schema` features of this crate,
the configuration and resolution crates and the schema generator enable the
same features of the crates below them: derive with
`#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]` and write
schema attributes as `#[cfg_attr(feature = "schema", schemars(...))]`.
TypeScript declarations are derived only for the browser's wasm32 target, the
one whose bindings read them, so native builds skip them: derive with
`#[cfg_attr(all(target_arch = "wasm32", not(target_os = "emscripten")), derive(tsify::Tsify))]`
and write tsify attributes under the same condition.

Hardware profiles preserve qualified device graphs and controller settings while
reference records continue to own identity, memory, and physical attachments.
`fujimod_catalog_construction::HardwareSelection` selects one entry from `Soc.hardware.profiles`
and one from `Body.hardware.profiles`. Profile IDs distinguish REC, service,
and discovery hardware configurations. Resolution refuses unknown profiles.
Profiles contain no input file locations. A device graph records closed device
kinds, named references, alternatives, and separate construction and attachment
orders; both orders must cover its declared nodes exactly once. Run presets own
execution policy and reset/scene inputs independently of these profiles.

`fujimod_catalog_construction::ConstructionProfile` retains an exported native topology with
closed model variants, ordered nodes, windows, dependencies, boot eligibility,
and typed endpoint parameters. Run presets own each node's execution/reset
options. Firmware `layouts` retain qualified boot plans, immutable table
identities, and persistent record formats. Reusable persistent asset/placement
types are shared rather than declaring types for individual named assets.

`provenance::CatalogAnnotations` and `rehearsal::Rehearsal` describe explicit
caller-supplied qualification records. They keep source citations, manifest and
input-file locations, verified input identities, and synthetic record seeds
separate from reference records. These are data schemas; loading them does not
execute firmware, apply a guard, or authorize a camera operation.

## Capture facts

`catalog/generation.generated.json` records each firmware's table outcomes and
specific generation errors, including firmware without a body or execution
profile. `generation::GenerationResults` validates the report, and JavaScript
exposes it through `catalog.generationResults()`. An unavailable table retains
its failed operation and reason; an available table has no errors. Consumers
use these outcomes to explain absent artifacts beside the affected controls.

`ResolvedSelection.flash` loads `flash.generated.json`: the installed device,
CPU-visible window, update-image mapping, reader alignment, checked code spans,
live driver preconditions and initial comparison read. The installed JEDEC ID
is hardware input verified against the driver's supported-device table.

`ResolvedSelection.state` loads `state.generated.json`. Each region has `name`,
`address`, `size` and `constant`. A constant region must be read twice with equal
bytes. Only established readable extents belong in this document; retained ROM
pointers alone do not establish a readable ROM extent. Device registers with
read side effects are excluded.

Both formats are generated facts, have JSON Schemas derived from their Rust
value types, and are checked when the catalog is loaded.

Parallel NAND facts use a byte-offset source window and a page-based reader
ABI. `reader.nand` records the 512-byte or 2 KiB page contract, destination
alignment, correction and polling modes, timeout and driver lock pair. Calls read one
main-area page with a zero-initialized progress word and a NULL observer;
the caller holds the driver lock. Unresolved installed part identities are
absent, and an unresolved execution transport is `null`.
