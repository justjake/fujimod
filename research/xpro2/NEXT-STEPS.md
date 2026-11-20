# Continuing the work

## Physical X-Pro2 qualification remains open

The next physical milestone is identification, not flashing. No body was
connected during the work represented by this handoff. Do not mistake the
reference X-T2 ROM, a modified SD image or the original DAT for a proven
X-Pro2 recovery path.

1. Identify the model and firmware using the maintained service tools.
2. Obtain two bounded reads of the candidate 32 KiB ROM window, with reviewed
   debug prerequisites and explicit user authorization for any configuration
   change. The ordinary fffw RAM command enables a RAM debug flag first; it is
   not equivalent to an acquisition that never changes that flag.
3. Compare both reads and repeat after a separate cold cycle. An identical
   prefix does not by itself establish the complete X-Pro2 ROM size.
4. Establish physical NOR/SD selection and button release behavior. The model's
   laboratory contact is not a discovered button map.
5. Verify original boot with absent SD, missing/truncated/corrupt boot files,
   both slots and failed modified application startup.
6. Establish integrity checks before SD execution and a recovery path independent
   of the modified application. Host-side hashes alone do not provide this.
7. Prefer native SD boot with original NOR preserved if the hardware permits it.
   A NOR patch requires separate layout, integrity, preservation and recovery
   evidence before producing an installation candidate.

In the reference model, original NOR A still booted without FFSYS/FFBOOT.
Selecting SD B with those files missing powered off before handoff. That is
**not** proof of an automatic fallback to A or physical recovery.

The per-body X-Pro2 catalog fields for ROM and installation must remain
unqualified until evidence from that body exists. Do not reuse the historical
64 KiB family hash: issue #355 explains the peripheral/SRAM over-read.

## Feature backlog

The laboratory implemented the FujiMods container, two opacity controls, five
menu themes and ten guide choices. Validate each ported feature before adding
the next. Remaining requested work includes:

- Highlight photometry and RAW highlight viewing.
- Manual snap focus and additional control assignments.
- Additional aspect ratios and surround view.
- JPEG quality/noise reduction changes.
- Anamorphic/desqueeze support and HDMI behavior.
- F-Log, film simulations, film editor and recipes.
- Date-based card names, daily folders and SD diagnostics.
- AF-S/phase-AF improvements, after the other selected functions.

These entries are research goals, not claims of available or compatible
upstream source. Menu Tap was excluded because the X-Pro2 has no touchscreen.

## AF findings to preserve

No AF or power improvement has been established. Static and synthetic tests
must not be advertised as real lens speed or optical accuracy. Stock NOR loads
code overlays on demand; a future AF timing profile should separate this cost
from estimator work. Useful candidate landmarks from the analysis include the
worker at `0x016fe204` and validation at `0x016fd3d0`; inferred roles and synthetic
input behavior require runtime confirmation. Lens travel, phase statistics,
exposure/SNR and final focus acceptance must be modeled or measured separately.

Keep future captures out of Git, especially serials, calibration, full memory,
photographs, author-machine paths and unreviewed bug reports. Commit selected,
redacted findings and the commands needed to reproduce them instead.

For already-acquired files, the offline comparison command is:

```sh
python3 compare-rom.py first-32k.bin second-32k.bin --output comparison.json
```

It refuses oversized windows, preserves existing reports, and never upgrades
an offline match into physical boot or flash qualification.
