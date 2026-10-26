# AVC codec

`AvcCodec` consumes aligned register writes and packed Y0,Y1,Cb,Cr
input DMA. It averages adjacent chroma rows to planar 8-bit 4:2:0. The
required `options.backend` selects exactly one compression implementation:

- `videotoolbox`: macOS VideoToolbox with hardware acceleration **required**,
  real-time encoding and frame reordering disabled. Native pixel buffers
  receive the same luma/chroma samples as software. Hardware initialization
  and encode failures refuse; there is no software fallback.
- `software`: the BSD-2-Clause [rusty_h264 encoder](https://github.com/Remade-With-Rust/rusty_h264),
  with scalar Rust, libm and explicit streaming Baseline configuration.
  Default SIMD, environment tuning and process-wide allocator features are
  disabled. The same implementation builds for native hosts and Wasm.

The native X70 run configuration selects this through
`preset.factory_options.video_encoder_backend`. Its local platform default
is `videotoolbox` on macOS and `software` elsewhere. CI, Wasm exports, sealed
rehearsals and recorded walkthroughs select `software` explicitly before
board construction. The resolved graph and device receipts retain the backend.

Both implementations share the input conversion, admission checks,
bitstream-ring writes, status fields, virtual deadlines and interrupt logic.
Only compression differs. Backend output is normalized to initial Annex B
SPS/PPS and one coded slice per frame. Identical parameter sets repeated at
later IDRs are omitted; changed parameter sets refuse. Optional SEI, AUD and
filler are omitted. Unsupported parameter-set sizes, multiple slices and
skipped frames refuse.

| Offset | Modeled meaning |
| --- | --- |
| `400` | Packed input DMA address, tightly packed two bytes per luma sample |
| `490/494`, `498/49C` | First/second bitstream base and byte capacity |
| `4A4` bits 2/3/4 | Release first/second output bank; common output enable |
| `4A4` bit 5 | Current output bank, read-only |
| `4AC` | Running byte offset in the current bank |
| `C00` | `800` initialize; `8000` encode; `1000` pause; `2000` finish |
| `C20` bits 2–3 | Primary operation mode (1) after completion |
| `C24` | W1C status: bank start bit 9, frame bit 12, pause bit 14, finish bit 15 |
| `D88/D8C` | Low/high halves of the actual frame bit count |
| `E00/E04` | Luma width and padded height; progressive packed input only |
| `E08` | Software target rate in 1024 bits/s units |
| `E0C` | Frame rate (`23/29/59` select `24/30/60`) |
| `E10` | Maximum software quantizer |
| `E34` | Maximum GOP length |

The target-rate, quantizer and GOP mappings define the software compression
approximation; they do not reproduce silicon rate control or identical
compressed bytes. The supported register mode is checked before input is
read. Crop/field encoding modes, overlapping/unmapped DMA,
invalid geometry and frames larger than a ring bank refuse. Other coefficient
or program uploads do not authorize an encoding mode.

Each bank begins available after release. Filling it consumes its availability;
releasing that bank leaves the other bank available for the in-flight stream.
An access unit that would enter an unreleased bank refuses before output DMA.

All input is latched at START. Compression runs without guest effects. A due
virtual deadline writes the encoded bytes, zero-pads only to a word boundary,
updates the cursor and exact bit count, then raises the configured IRQ. A
frame can cross from either bank into the other; crossing sets the bank-start
status. The interrupt level remains high while any status bit is pending, so
the controller re-pends it after a partial acknowledgment and end of interrupt.
Acknowledging the last bit lowers the line. Early advancement is inert. The declared
throughput is 64 luma samples per virtual tick; setup, pause and finish take
two ticks. These are model clocks, independent of host elapsed time.

GOPs start with a fresh IDR and constant SPS/PPS IDs. Software checkpoints
share immutable binary input blobs for only the current GOP (at most 120
frames), plus any pending encoded payload. Restore lazily rebuilds the host
codec by replaying those inputs, without reading guest memory or replaying
guest effects. Active VideoToolbox sequences refuse exact checkpoints and
fingerprints: identical replay requires selecting software at construction.
Completed sequences release host codec state, so later checkpoints can proceed.

The colocated video tests decode an eight-frame DMA clip across P frames and
GOP restarts, check decoded luma/chroma, compare restored successor bytes,
cross ring boundaries, check exact deadlines and DMA-before-IRQ order, and
refuse invalid modes and spans without publishing a completion.

The local hardware regression is explicitly selected with
`cargo test -p fujimod-emulator-exr2-isp-output --features videotoolbox-tests --lib tests::avc`.
Ordinary CI runs the software tests. The `h264_fingerprint` example and
`scripts/ci/test-h264-wasm.ts` compare all 256 SHA bits of a generated eight-frame
software stream between native and Wasm; its optional native output can also
be decoded by an independent AVC decoder.

## Alternate decoding

`C00=4000` consumes one Baseline progressive I/P picture from the addressed
Annex B stream banks. Dimensions and cropping come from a bounded SPS, with
at most 1920×1088 coded samples, one slice per picture and no slice groups.
The native host codec is OpenH264 with error concealment disabled. Unsupported
targets, syntax, modes and malformed streams refuse before output DMA.

`400..40C` select frame destinations, `F1C=3` selects three rotating slots,
and `F14/F18=F7F9/D8FF` select the supported syntax fields. Input banks may
alias for a preview; output slots may alias. Input/output overlap refuses.
Decoded output uses tightly packed I420 transport: full-resolution Y followed
by quarter-resolution Cb and Cr planes. The output footprint is exactly
`width*height*3/2`, with luma stride `width`; source/output overlap checks use
that footprint. The paired image-transfer device expands this to packed422
before downstream sampling. I420 plane order is the model's declared transport,
not a claim of measured silicon plane ordering or tiling.

At the virtual deadline, frame DMA completes before `C20` publishes mode 2,
`C24` publishes frame bit 11 and `FC0` selects the completed slot. Exhausting
input bank 0/1 publishes refill bit 9/10; an initial picture does not issue a
refill. `4AC` and the bank selector track consumed Annex B boundaries.
The [H.264 Annex B B.1.1](https://www.itu.int/rec/T-REC-H.264) byte-stream
delimiter rule ends a NAL at `000000` or `000001`. A zero run therefore ends
coded payload even when unrelated nonzero bytes remain later in the DMA bank.
Padding before a following start code enters the consumed span; otherwise the
receipt ends at the coded NAL. Emulation-prevention bytes preserve escaped
zero sequences, and missing required escapes refuse.
Pause and finish issue their separate bit 14/13 notices. W1C and level IRQ75
behavior is shared with encoding.

Checkpoints retain binary compressed GOP history and pending output, then
reconstruct the host decoder without replaying guest effects. The completed
input receipt, compressed diagnostic buffer, output hash and frame counter
publish atomically with DMA. Each output carries its input hash; a subsequent
pending picture cannot relabel earlier pixels. Replay is bounded to 120 frames
and 16 MiB per GOP. Native decoding is available; Wasm decode starts refuse.
