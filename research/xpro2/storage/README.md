# X-Pro2 5.11 storage and shutter research

This update provides source and compact evidence for follow-up work on camera
storage. The existing handoff still reproduces the earlier FujiMods build.

## Reproduce the DATE FOLDERS probe

Use Python with `unicorn` and `keystone-engine` installed in a local virtual
environment. Run this command from the repository root with the official
X-Pro2 5.11 update listed in the [reproduction guide](../REPRODUCING.md).

```sh
python3 research/xpro2/storage/probe-date-folders.py \
  --dat /path/to/FWUP0008.DAT > date-folders-result.json
cmp date-folders-result.json research/xpro2/storage/native-path-syntax.json
```

The probe also accepts the exact stock image via `--resident`. Both inputs
are hash-checked. The probe executes the two stock FAT modules in Unicorn
with controlled RTC and mounted-index fixtures. It runs 59 checks covering
valid and invalid dates, both slot paths, native rollover, folder-number
exhaustion and stock FAT directory timestamps. The owned ARM helper emits
`_MMDD`, a year-inclusive date key and the matching FAT date. No sectors
are written by this probe.

## Shutter inputs and capture status

`add-shutter-contacts.py INPUT_GRAPH OUTPUT_GRAPH` adds active-low shutter
contacts to an X-Pro2 5.11 lab graph with an XPP GPIO block and a contact
driver. The graph must run with XPP support for the shared `GpioInputs`
capability. The historical engine patch predates that change. Review
[shared GPIO inputs](https://github.com/justjake/fujimod/commit/da01636b)
and [XPP panel support](https://github.com/justjake/fujimod/commit/0e13f7da).

The stock key scanner acknowledges half and full presses. With the stock
SHOOT WITHOUT LENS option on and electronic shutter selected, holding
half-press before full-press reaches REC handler `0x148ee38`. The watched
DCF constructor and directory creation routines remain unused and neither
virtual SD changes. Earlier calls to the DCF selector came from a count of
remaining file names. Trace summaries are stored alongside this guide.

Continue from the REC call to `0x14cb9bc` following `0x148ee38`. Once a
stock capture reaches the save path, the remaining port work is applying the
date at the scoped write call, resolving the active card's directory date
after restart, and adding the FujiMods menu toggle.

## FORMAT CARD NAME and ROM A/B results

The authored RTC and volume-label source is included here. In the local
resident guest, FORMAT wrote the date label and recreated FujiMods preferences.
A cold restart then loaded the saved choice. The native model was also
checked with removed and write-protected virtual cards. The write-protection
warning's displayed slot number still requires verification.

On the X-T2 reference ROM, the later laboratory candidate passed 33 A/B,
menu and persistence checks. Mode A loaded the original NOR path and mode B
loaded the FujiMods SD path. With a truncated SD file, mode B handed off to an
incomplete image and produced no usable frame. Forcing mode A still booted.
These results cover the emulator. Physical X-Pro2 ROM identity, selection
and recovery remain open. This update includes the reusable storage sources
and results. The full FORMAT candidate integration and its replay runner
remain local to the laboratory.
