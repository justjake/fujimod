# Emulator speed

The goal is three times camera speed in release builds: an X70 that boots to
REC in about one second on the camera boots in 0.33 s here, and every later
interaction keeps the same ratio. Speed is always camera wall time for an event
divided by the emulator's time for the same event. Emulated (virtual) time is
not a speed; when a milestone takes more emulated time than the camera takes,
the emulator does extra work and that is a correctness bug.

## Measurements

Two numbers matter, and neither is wall time on a shared machine. The host
instruction count is stable across machines; engine CPU time is stable enough
to compare back-to-back runs. Every table below was taken with the CPU
contended, so wall seconds are omitted.

The engine keeps its own accounting. `fujimod.perf` begins a guide, then
reports per-core `calls` and `elapsed_ns`, the guest instructions and ticks
of every phase, and the CPU and R-profile exit counters:

    from fujimod import perf
    perf.begin_guide("ev-dial")
    ...boot and run the guide...
    summary = perf.end_guide()   # summary["engines"]: [{core, calls, elapsed_ns}]

For attribution, let the engine name every generated block and sample the
process:

    FUJIMOD_PERF_MAP=1 samply record --save-only --output profile.json.gz -- <emulator command>

Set `FUJIMOD_PERF_MAP` to a path or to `1` for `/tmp/perf-PID.map`. Each
installed translation gets one line, `HOSTSTART SIZE guest_<pc>`, so Samply
attributes samples inside generated code to the guest address that generated
them.

The reference workload, `controls/hero/ev_dial` with a saved card and clock,
is 1,853,033,658 guest instructions and 159,279,534 virtual ticks. Core 0
makes 293,421 engine calls and core 1 273,143; core 0 translates 93,380 blocks
and invalidates 4,113. Of the block entries, 806,825 on core 0 and 766,915 on
core 1 leave translated code for the Rust callback.

## Where the time goes

Attribution on a scratch ARM program, because a walkthrough mixes engine cost
with Python observation, PNG decoding and scene rendering. The program is the
shape from the engine design notes: `bl` to a two-instruction leaf that
returns with `bx lr`, then `subs` and `bne` — five guest instructions in
three blocks. 1,073 samples of a 200M-instruction run, self samples:

| Cost | Share |
|---|---:|
| `helper_lookup_tb_ptr` (includes `cpu_get_tb_cpu_state`) | 46.9% |
| leaf block (`add`, `bx lr`) | 24.4% |
| caller block (`bl`) | 15.8% |
| loop block (`subs`, `bne`) | 12.3% |

Only the `bx lr` exit entered C: `bl` and `bne` are direct branches that
use QEMU's patched `goto_tb` chain. Recomputing the block state and probing
`tb_jmp_cache` on every return cost more than the guest instructions it
separated, which is why the engine now remembers the last indirect target per
processor and drops it whenever a translation dies.

With that cache the same loop runs 326.5 million guest instructions per engine
CPU second in interactive policy (median of five 30M-instruction passes,
thread CPU time) against 256.2 before it. A straight-line load/store loop
reaches 1.25-1.37 billion. These rates follow the machine: at a load average
of 45 the same binary reports 232-299 on the call loop, so compare runs taken
back to back.

Coarse accounting has one implementation: the block debits the budget in
generated code. A fast block enters the block helper only when the budget runs
out or a pause arrives, and exact accounting keeps its ring write there.

## Plan

In expected order of return, each measured by CPU time to input readiness and
host instructions retired (load barely changes them):

1. Polled status registers as fast reads. Each register a boot loop polls is
   published as an atomic the CPU reads without entering the device. Then let
   interactive runs skip a polling loop whose only input is such a register
   until the device's next deadline, as exact runs already do.
2. Block hook calls. Debiting the budget in translated code instead of the
   hook helper left host instructions unchanged (30.9G for three boots either
   way): most blocks reach the hook because the interactive budget is short or
   the accounting mode changed. Find why, and widen the interactive window.
   Translated accounting is now the default and is worth 2.2x on short-block
   code, but a whole guide still spends nearly a callback per thousand guest
   instructions, so the interactive window is still the thing to widen.
3. Done: interactive runs test exits at block entry with QEMU's inline test
   and no check after guest loads and stores.
4. Done: exclusives without leaving translated code (below).
5. Firmware decompression in whole tokens and slices instead of per byte.
6. Translation cost: fewer generated helper calls per block (items 2 and 3)
   shrink every block; then measure what remains.
7. Device access bookkeeping: wake only the dependents an access can affect and
   update deadlines incrementally.

The ISP statistics producer and image output channels, which dominate live view,
are optimized with the scene and ISP work.

## Walkthroughs

`controls/hero/ev_dial` on the X70 (11.2 camera seconds, 1.03G guest
instructions after boot, exact policy) is the reference guide. On the shared
machine it ran at 0.12–0.15x camera speed and failed the 60 CPU second limit.
Each Python wait poll ran the board once with that poll's read set as store
watches, and installing and removing about 500 completed-store hooks per run
cost 1.5–2 ms against 3 us for an unwatched empty window. 96% of consecutive
polls watch the same set, so a continuation now keeps its hooks for the next
one (`board/continuation.rs`): the empty watched window costs 25 us and the
guide passes at 11.2 camera seconds in 22.0 CPU seconds (0.51x).

Engine time in that guide, as shares of the whole run sampled after boot (the
remainder is Python observation, PNG decoding and live-view scene rendering):

| Cost | Share |
|---|---|
| Stores on the slow path (`store_helper`), including state restores for MMIO inside a block | 16% |
| Translating guest code (`tb_gen_code`) | 13% |
| Instruction callbacks: exclusives 5%, failed loop proofs 2%, register reads that restore state | 11% |
| Per-access exit checks (`helper_check_exit_request`) | 7% |
| Per-block hook helper (`helper_uc_tracecode`) | 7% |

Core 0 left translated code 1.73M times ("check exit, no sync"), once per
600 guest instructions: an emulated LDREX, STREX or CP14 identity read wrote
the PC from its callback, which leaves `cpu_exec`. The translation filter now
replaces those ARM instructions (word exclusives, CLREX, the CP14 identity
read and PMU transfers) with their instruction observer
(`InstructionDisposition::Replace`): the block carries the hook and no body,
ends after it, and chains to the next block. The observer writes the PC only
when a stop was requested while it ran. A successful software STREX also
publishes to completed-store observers, as a native store does. Thumb has no
translation filter and keeps the PC write.

Boot exits ("check exit, no sync" on core 0 over the first 65M instructions
in exact policy) fell from 112,725 to 1,951, but host instructions barely
moved: about 1% for ev_dial, inside run-to-run noise. Leaving `cpu_exec` is
cheap next to the rest.

Translation churn came from the translation cache. At 16 MiB it filled every
22.7K blocks (about 740 host bytes each) and was flushed, and the walkthrough
translated its working set again: 21 flushes and 440K translations in the
first 60K runs. Its translated code in use is about 63 MiB, so 64 MiB still
cycled. Each processor gets a fixed 128 MiB translation cache. This holds the
63 MiB working set about twice over. On native hosts the cache is a reservation;
pages are committed only as code is generated, so concurrent emulators pay for
their translated code working sets. ev_dial then translates 85K blocks with no
capacity flush.

Browsers use a fixed 32 MiB per processor. Emscripten serves the cache from
Wasm heap, which is committed when allocated and never returned, and iPhone
Safari kills a tab whose memory passes its limit: with two X70 processors at
128 MiB the tab grew by 224 MiB over the earlier 16 MiB and was killed after
boot. Board construction fails with an error when the remaining heap cannot
hold every processor's cache plus 64 MiB of headroom. The translation counters ("translations",
"limited_translations", "translation_flushes", "translation_invalidations")
are in the CPU exit counters. Each invalidation also increments one
"translation_invalidations_*" counter naming its caller: restore or execution
of a one-shot translation, I/O recompile, a guest store to a code page, an
emulation exit address, the embedder's translation removal, a watchpoint, an
embedder instruction restart, or a physical write to code.

| Change | ev_dial host instructions | Boot host instructions |
|---|---|---|
| Before | 213.1–213.7G | 19.1–19.2G |
| Replaced exclusives and identity reads | 207–213G | 19.1–19.2G |
| 256 MiB translation cache | 196.1–197.6G | 18.4G |

ev_dial host instructions are the runner's per-guide records
(`host_instructions`); boot counts the whole process for construction and 65M
instructions. CPU seconds follow machine load too closely to compare (the
same build took 16.3 and 21.6 CPU seconds); with the 256 MiB cache ev_dial
takes 14.7–15.2. Guest instruction counts (888,235,092 for ev_dial), board
fingerprints and live-view frames are identical in every row.

`highlight-meter/compensation/raw_highlight_compensation` used to stop at the
60 CPU second limit in step 12 (695–793G host instructions). With the cache it
runs all steps in 53.8 CPU seconds (818G, 1.82G guest instructions) and then
fails its own light-ratio assertion: −1 to 0 EV measures 2.94 times the light,
outside 1.6–2.5. Its `photometry/ready` prerequisite fell from 308–313G to
258G host instructions.

### Loop candidates and block hooks (ev_dial at 0.95x)

Exit counters now name each loop-proof failure and each reason a block takes
the hooked accounting path. In ev_dial every loop proof failed and none
accelerated: about 1.36M of 1.45M entries were loops whose iteration model the
accelerator does not support, a verdict that depends only on their code. Those
verdicts are cached with the checked code bytes and the branch is translated as
a plain instruction, so candidate entries fall to about 12K (11 distinct
loops) and engine time from 8.5-9.0 s to 7.8 s, with identical final
instructions, ticks and fingerprint. ev_dial runs 16.3 camera seconds in 17.1
CPU seconds.

The remaining hooked block entries (about 255K of 334K) are blocks larger than
the instructions left before the next device deadline, about once every 6K
instructions: exact stops that must split the block. Bounding every admitted
span at the next guest tick instead stays exact but splits nearly every block
(115M hooked entries, 44 CPU seconds), so that approach is rejected.

## GPU input recordings

To capture one run, set `FUJIMOD_VECTOR_GPU_RECORD` in the environment that
launches the walkthrough. The path must name a new file. The capture starts
when the vector GPU is constructed and records register accesses, virtual
ticks, IRQ routing samples, every DMA read and its bytes, plus output addresses
and digests. Close the walkthrough runner normally to finish the file.

For the `controls/hero/ev_dial` walkthrough, run this in the same shell that
starts the usual walkthrough command:

```sh
export FUJIMOD_VECTOR_GPU_RECORD="${TMPDIR:-/tmp}/ev-dial.vgpu"
# Start controls/hero/ev_dial with the usual walkthrough runner.
```

Replay the resulting recording with:

```sh
cargo run -p fujimod-emulator-exr2-gpu --bin fujimod-gpu-replay -- "$FUJIMOD_VECTOR_GPU_RECORD"
```

The replay reports elapsed model and rasterizer time per submission and
register access, then prints the digest of every output write and verifies it
against the capture.

## Host GPU path

Evaluate a host GPU backend as a separate step after profiling the CPU
rasterizer. Keep the CPU implementation as the bit-exact reference and as the
fallback on every target.

A host GPU operation is eligible only when its complete output can match the
CPU reference on every supported implementation. The shader must use integer
arithmetic with explicitly defined overflow, rounding and division. Pixel
packing and unpacking must preserve every bit, and blending must implement the
same integer equations and rounding as the CPU path. Texture access must use
exact texel reads with no filtering, implicit conversion, color-space transform
or implementation-dependent precision. Readback and synchronization must also
preserve the exact output bytes and ordering visible to the emulated device.

Qualify each operation against the CPU reference on every supported host API
and device, using output digests from real GPU input recordings. A mismatch or
an API/device without the required integer formats keeps that operation on the
CPU. Fixed-function OpenGL ES 1.1 blending, filtering and format conversion do
not provide the controls needed to guarantee these integer results, so that
pipeline is not a suitable exact backend. A shader-based path is a later
candidate only for operations whose arithmetic and memory behavior meet these
conditions; it is not part of the current implementation.
