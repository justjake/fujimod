# fujimod-corpus

Generic ARM32 (A32) and AArch64 decoding, function features, combined firmware
families, and locator queries. Boundaries, mapped bytes, pointer windows, names,
types, readable C, notes, and external reference information are caller inputs.
This crate contains no camera maps or firmware-derived fixtures.

`align_sequences(left, right)` aligns arbitrary string tokens using the same
longest-block implementation as family call alignment. It preserves every
token, uses no popularity/junk heuristic, and resolves ties at the earliest
left position then the earliest right position. Results pair optional indexes;
an absent index marks an insertion or deletion. Python exposes the same function
through `fujimod.corpus`, releasing the GIL during Rust alignment.

`Index::add_unit(UnitInput, Vec<Region>)` accepts any boundary provider; all byte
ranges are half-open. The JSON input contract is
[`schema/unit.schema.json`](schema/unit.schema.json). Regions are passed separately
as addresses and byte arrays. Overlapping mappings require distinct units.
Unmapped function bodies and duplicate units are errors. ARM32 means A32;
Thumb boundaries require a future explicit instruction-set selector.

`Index::build()` indexes all supplied units together. Unique normalized code
(at least four instructions per function) seeds families across firmwares.
Duplicated code waits for aligned call-site evidence. Calls resolve first in
the caller's unit, then at a unique address in its firmware. Ambiguous targets
remain unresolved. A family has at most one member per firmware; conflicting
proposals are left unmatched. This deliberately prefers incomplete families to
silently merging two different functions.

Matching retains branch shapes, conditions, field offsets and constants, but
removes register allocation, branch destinations, literal addresses, stack
slots and split address immediates. A32 token spelling and no-junk token-diff
alignment preserve the earlier clone pairer's behavior. Features include
normalized SHA-256, instruction trigrams, referenced strings, constants, data
references and call edges. Rare strings plus high trigram agreement and shared
constants supply additional seeds. Cross-ISA feature seeds require two shared
strings and two nontrivial constants, with shared strings covering at least a
quarter of each function's strings; these are heuristic call-site-grade
matches, never equal-code evidence. Weak/dissimilar call-site candidates are
retained when two independent caller families nominate them without conflicting
votes across discovery rounds. They carry graded names but never merge families
or propagate calls. Instruction-token call alignment runs only within one ISA.

`pairs(reference, other)` projects the combined families into `identical`,
`same-code`, `call-site`, and `weak` evidence. `identical` means equal normalized
code with consistently resolved callees; it does **not** mean byte equality.
`same-code` has equal normalized code but incomplete callee evidence.

Locators can find by name/address, select family members in a firmware, follow
calls and data references (including reverse references), read family
annotations, and search mapped units with byte masks. `names` retains names
and donor provenance; callers can require equal normalized code. Metadata
annotations use explicit `kind`, `text`, and `source` fields.

`Index::save` writes a versioned SQLite store; its schema is internal, not a
portable interchange contract. `Store::open` reads only the SQLite header.
Lookups (`Lookup`: stats, find, family, members, names, calls, features,
references, strings, reads, byte searches and symbols) run as SQL queries
against the store and share one implementation with the in-memory `Index`.
Graph-wide work (`build`, `merge`, `candidates`, `pairs`, `name_support`,
`sample_links`, `data_witnesses`) runs on an `Index`: `Store::load` reads all of
it, and `Store::subset` reads only the selected firmwares before rebuilding
families. Rebuilding/addition changes numeric function/family ids; persist
`(firmware, unit, address)` keys in locator reports. Rebuild after adding units,
or `merge` indexes and rebuild automatically.

Python uses `from fujimod.corpus import Index`, with the same operations and
JSON-compatible dictionaries. `Index.open` answers lookups from the store and
loads the complete index once, on the first graph query or change. Build the extension through the existing
`bindings/python` package. Test output goes under `target/corpus-tests`.

Analysis names propagate through every grade, including weak links. Each donor
returned by `names` or `find(name).name_donors` includes its grade and full
`(firmware, unit, address)` identity. Its `sources` list retains the caller's
`name-source` annotations for that name, so reports can select a donor source
without conflating names attached to the same function. Weak names can cross one
weak edge between families; weak edges never merge families, chain with one
another, or authorize call propagation. `normalized_only` limits donors to
identical/same-code links.
Consumers that generate executable symbol maps must use that restriction or
independently verified execution evidence.

`candidates(function_id, firmware)` returns all family members, one-hop weak
links and relocation-shape nominations, retaining unit identity and origins.
Shape-only nominations have weak grade. They account for conditional split
address immediates and MOV/MOVW low halves followed by MOVT, without merging
families or transferring names. Callers verify actual instructions, references
and ambiguity before accepting a locator result. This query uses an in-memory
cache and does not change the saved store.

`sample_links(count, seed)` draws deterministic bottom-k samples of every grade
over the complete pair population, with cross-ISA links separated. `features`,
`body_sha256`, `data_witnesses`, `string_frequencies`, and `name_support` report
observational corroboration. A positive witness is not ground-truth proof;
corroboration never blocks names in analysis or locator queries.

Address aliases are explicit one-hop byte mappings in `address_aliases` of the
unit input. A unit keeps its layout and bytes; decoding consumes its boundaries.
They affect reads; pointer classification remains a separate caller-supplied
window policy. Adjacent A32 MOVW/MOVT and AArch64 ADRP/ADD materializations expose
complete data/string references rather than address fragments.

Named data ranges use `DataSymbol` and
[`schema/data-symbol.schema.json`](schema/data-symbol.schema.json). `add_symbol`
checks the complete mapped range; `symbols` queries it independently of function
families. `add_name` records donor provenance and survives subset/merge/save.
