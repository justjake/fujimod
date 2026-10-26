# fujimod-firmware

Body-independent update-file mechanisms, with no filesystem or device access.
Native Rust and `wasm32-unknown-unknown` use the same decoder.

- `invert` applies whole-file XOR with `0xff`.
- `decompress` validates a method-4 stream header and decodes bounded literal
  runs and overlapping back references, returning compression statistics.
- `inner_header` reads 32-bit or 64-bit initialized mappings.
- `scan` finds and validates candidate streams in an encoded update file.
- `raw` verifies explicit raw extents, hashes, optional inner headers and entry
  addresses. `overlays` reads bounded 9-word section and 6-word overlay tables,
  checking the supplied flash/DAT bias against compressed mappings.
- `outer_header` reads type 4/6/8 headers and locates a type-8 ECDSA DER trailer.
  Identity records and uninterpreted header fields remain opaque. Parsing a
  trailer does not authenticate a file or verify an update checksum.
- `decode` takes explicit `Mapping` values and returns an `Image` with the
  original file's SHA-256 and named `Unit` values (base, bytes, executable).
  Overlay banks are independent units and may share addresses. Mapping sizes
  bound initialized stream bytes; they do not create zero-filled BSS.

Callers supply source identity, expected hashes, mapping selection, expected
overlay records and output paths. No camera model or firmware address
is built into this library. These Rust types are not a serialized file format.

The Python binding exposes `fujimod.firmware` with the same operations. Its
`decode(source, mappings)` takes tuples `(name, DAT_offset, initialized_size,
base, executable, method4)` and returns an `Image`. Header and diagnostic
results are ordinary Python dictionaries; raw payloads are `bytes`.

Section and overlay addresses are byte offsets, not hardware erase-block
indices. The descriptor parser does not impose a processor's flash-block
alignment; it checks table bounds, group sizes, payload extents and the supplied
flash/DAT bias against every executable compressed stream.
