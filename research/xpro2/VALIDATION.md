# Validation and evidence boundaries

Machine-readable results are in `data/validation.json`. Raw reports and images
stay in the private/ignored output directory; the public summary retains only
selected reproducible facts and hashes.

## Checks run for this handoff

- Fifteen synthetic preparation/privacy tests passed without firmware or USB.
- The historical engine patch applied to all 52 declared source paths, produced
  the exact expected text, and reversed to the original base including removal
  of added files. This is not a fresh compiler run of the filtered patch.
- From the three local public inputs, `prepare.py` reconstructed the exact stock
  resident and FujiMods SD images. It also reconstructed the combined laboratory
  input with SHA-256
  `fa638e91fd6dd9790815e031298a3150ff287129d05ebb888ad93c94ce6f7fb7`.
- Both ROM profiles were executed again using the existing native binary built
  from the validated laboratory component snapshot. A reached the guest display
  through the original NOR branch. B reached the display through ROM/SD; its
  first handoff hash equalled
  `09ef2a0f2719635d1b7597424035bcaae6c605e39b163946311e4854ee45ef56`.
- The published UI replay exercised twelve follow-up controls: last SETUP folder,
  FujiMods entry, return and re-entry, ten folder rows and preferences loaded
  without error. Both cores had zero recorded abort fields on that SD path.
- The first UI-runner attempt reached the correct screens but requested an
  export outside the native harness's allowed snapshot directory. That runner
  error was fixed by removing the unnecessary export. The corrected command
  was rerun successfully; failed attempts are not counted as passes.

## Historical checks retained as context

The full laboratory had 111 XPP component tests, 11 builder tests, 636 framing
ARM checks, 21 guide UI/cold checks, 40 raster checks and 43 reference-ROM
qualification checks passing at their recorded stage. Menu/indicator opacity,
theme and guide persistence were exercised with cold reconstruction. Those
counts do not mean every historical test was rerun for this handoff or passed
against today's upstream architecture.

The FujiMods rename changed ten effective data bytes in the owned module and
kept code, IDs and preference schema unchanged. The canonical replay includes
that final rename. The older 43-check ROM qualification used the preceding F1
caption image; the fresh ROM replay above covers the rebuilt FujiMods image.

## What has not been demonstrated

- A current-upstream runtime build with the historical changes integrated.
- Physical X-Pro2 ROM identity, boot-button wiring, installation or recovery.
- Flash-programming coherence of the read-only NOR XIP laboratory mirror.
- Measured lens response, AF speed/accuracy, sensor behavior or camera energy.
- Arbitrary firmware compatibility. Builders are deliberately hash-bound.

The original NOR path's handled overlay prefetch exception is described in
[PORTING.md](PORTING.md). A nonzero historical IFSR alone does not mean the guest
failed; conversely, an error-free emulator is not a proof of physical safety.

## Review points

Inspect the sparse recipe and input hashes, the source patch attribution, model
assumptions, current-upstream inventory and omitted materials before publishing.
The automated public audit checks file types, UTF-8 text, size limits, common
credential formats, personal paths and oversized binary JSON fields. It cannot
prove that every possible secret or ownership issue has been identified.
