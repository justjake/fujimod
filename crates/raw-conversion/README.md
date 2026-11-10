# Tethered RAW conversion

`fujimod-raw-convert` has an X-Trans IV or V body (X-T4, X-S10, X-E4, X100V,
X-H2S, X-T5, X100VI, …) develop RAFs with its own processor, the way
FUJIFILM X RAW STUDIO does. It takes a job file. Each job names a RAF and
the settings to apply. The tool writes the camera's JPEGs unchanged and adds
one manifest line per job.

Set the camera to **CONNECTION SETTING > CONNECTION MODE > USB RAW
CONV./BACKUP RESTORE** before connecting it. The tool changes only the
conversion profile, starts conversions and deletes each converted image from
the camera's RAM after downloading it. It does not write to the SD card or
change any camera setting.

```sh
cargo run --release -p fujimod-raw-conversion --bin fujimod-raw-convert -- \
  --jobs jobs.json --out OUT --raf base=DSCF0001.RAF --raf iso3200=DSCF0005.RAF [--dry-run]
```

- `--dry-run` loads and checks the job file, then lists every job with its
  RAF, its settings and the profile words it writes (`index=value`). It also
  says whether `OUT` already has the job. It does not touch USB.
- A run skips jobs that `OUT/manifest.jsonl` already records, so after an
  interruption it continues from the first unfinished job. A converted job
  counts as finished only while its JPEG is still there at the recorded
  size. A session left open by an interrupted run is reused.
- Each RAF is uploaded once and converted for all of its consecutive jobs.
  `--upload-each` uploads again before every job.
- `--conversion-timeout SECONDS` (default 120) bounds the wait for each image.
- Camera selection follows FujiMod discovery. A single local Fujifilm device
  is used directly. With `FUJIMOD_USB_PROXY_URL` set to a share link
  ([SHARING.md](../usb-proxy/SHARING.md)), the shared camera is used
  instead. On macOS, quit Image Capture and other apps that claim PTP
  cameras on the computer the camera is plugged into.

## RAFs on the camera's SD card

In USB RAW CONV. mode the camera exposes no SD card files; conversions take
RAFs from the computer. To use RAFs shot on the body, first set CONNECTION
MODE to USB CARD READER, where the SD card is a PTP store, and read them:

```sh
fujimod-raw-convert card list
fujimod-raw-convert card fetch --dir RAFS base=DSCF0001.RAF iso3200=DSCF0005.RAF
```

`card list` prints each RAF's file name, ISO, capture date and size. The ISO
comes from the Exif of the RAF's embedded preview, read with two
GetPartialObject (`0x101B`) calls: the 92-byte RAF header holds the preview's
big-endian offset at 84 and length at 88 (LibRaw `parse_fuji`, dcraw). `?`
means the camera refused the partial read. `card fetch` downloads the named
files whose local copy is missing or the wrong size, then prints the `--raf`
flags for the conversion run. Neither command deletes or changes anything on
the SD card. Then set CONNECTION MODE back to USB RAW CONV./BACKUP RESTORE
to convert. Both mode changes are made on the camera.

## Job file

```json
{
  "base": {"white_balance_as_shot": "ON", "dynamic_range": 100, "color_space": "SRGB",
           "image_size": "L_3_2", "image_quality": "FINE", "grain": "OFF"},
  "jobs": [
    {"id": "provia", "raf": "base", "settings": {"film": "PROVIA"}},
    {"id": "acros-iso3200-nr-4", "raf": "iso3200", "settings": {"film": "ACROS", "noise_reduction": -4}}
  ]
}
```

Each job's settings are layered over `base`. Field names and values are listed
in `src/profile.rs` (`Settings`). Values are the camera's menu steps; for
example, `clarity: 3` is CLARITY +3. The tool rejects unknown fields,
out-of-range values, missing films and repeated or non-file-name ids before
it connects to a camera.

The camera fills the profile from the uploaded RAF. A job overwrites only the
words its settings produce. Every other word stays as the camera reported it.
Words 22 and 25 hold MONOCHROMATIC COLOR on monochrome films and white-balance
data on color films. They are written only for MONOCHROME and ACROS jobs, with 0
when the job leaves them unset.

## Manifest

`manifest.jsonl` gets one JSON object per finished job: its id, the RAF's
file name and SHA-256, the merged settings, and the camera's manufacturer,
model and firmware version from GetDeviceInfo. It also lists profile files
in `profiles/`: `as_shot` is what the camera reported after the upload, `sent`
is what the job wrote, and `camera` is the read-back when it differs. The
output entry gives the JPEG's file, SHA-256, size, frame dimensions and the
integer tags of its Fujifilm MakerNote, such as film mode `0x1401`. The JPEG
is saved before it is read, so if it cannot be parsed the file is still
kept, and `inspect_error` says why.

`status` is `converted` or `unsupported`. A job is `unsupported`, and nothing
is converted for it, in three cases: the body's profile has no word for a
setting the job names (`missing`), the camera refuses the profile (`reason`),
or the read-back differs from what was sent for a setting the job names
(`adjusted`). Missing or changed base settings are recorded and the job still
converts.

## Protocol

PTP over the camera's bulk endpoints (PIMA 15740, USB Still Image Capture
Device Definition). A data phase or container that fills its last packet ends
with a zero-length packet. Fujifilm-specific parts:

| What | Value | Source |
|---|---|---|
| Upload ObjectInfo | vendor operation `0x900C`, params `0, 0, 0`; ObjectInfo with format `0xF802`, the RAF size and file name `FUP_FILE.dat` | libfuji `lib/fuji_usb.c` `fuji_send_raf`, `fuji_send_object_info_ex`; `lib/fujiptp.h` |
| Upload data | vendor operation `0x900D`, no params, the whole RAF | same |
| Conversion profile | device property `0xD185`, read with GetDevicePropValue after the upload and written with SetDevicePropValue | libfuji `fuji_process_raf`; `lib/fujiptp.h` `PTP_DPC_FUJI_RawConvProfile` |
| Start conversion | device property `0xD183` set to UINT16 `0` | libfuji `fuji_process_raf`; `PTP_DPC_FUJI_StartRawConversion` |
| Result | a new object handle in GetObjectHandles(`0xFFFFFFFF`, 0, 0), downloaded with GetObject, then DeleteObject | libfuji `fuji_process_raf` |
| Mode check | DeviceInfo lists `0xD183` and `0xD185` | libfuji reads USB mode `0xD16E` = 6 for RAW CONV. |
| Profile layout | UINT16 word count, UINT8 length and UTF-16 IOP code, zero padding to `0x201`, then one UINT32 per property | libfuji `fp/src/d185.c` `fp_parse_d185`, `fp_create_d185` |
| Property order and encodings | `Field` and `Settings::words` in `src/profile.rs` | libfuji `fp/src/d185.c` `enum D185Props`, `fp/src/fp.h`, `fp/src/data.c` |

libgphoto2 `camlibs/ptp2/ptp.h` independently names `0xD184` IOPCode and
`0xD186`/`0xD187` TetherRawConditionCode/CompatibilityCode beside them.
libfuji is MIT-licensed, by Daniel Cook (petabyt), commit
`7a421a7690e271326c0c1cae0755dcba0f8b94a5`. Its results come from black-box
reversing.

Not yet confirmed on a camera:

- libfuji marks film codes 18 ETERNA BLEACH BYPASS, 19 NOSTALGIC Neg. and
  20 REALA ACE as guesses. Check the output's MakerNote film mode, `0x1401`.
- MONOCHROMATIC COLOR uses step × 10, like the other ±steps. libfuji's tables
  allow ±18 steps; menus show ±9.
- libfuji comments that `0xD183` = 1 gives full-resolution JPEGs, but it
  sends 0. The manifest's frame size shows which this is.
- That USB CARD READER is PTP, not USB mass storage, on each body.
- Whether a loaded RAF converts more than once. `--upload-each` is the
  fallback if the second job on a RAF produces no image.
