# Firmware downloads

The homepage loads the current GitHub release through the same-origin release
proxy. Each firmware's `downloads.sd_boot_v1` and `downloads.sd_boot_v1_installable` switches
control its download rows and firmware files in the emulator's card browser.
Both switches default to false. Installable downloads also require a verified
`fpupdate` output in the release patch and the entry's `update_filename`.

Choosing a DAT adds it to the browser firmware library. The release panel offers
available downloads separately from the emulator's Run actions. Run resumes the
latest saved camera with that firmware hash and FujiMod setting; Start fresh
creates an independent camera.

SD boot ZIPs contain `ffboot.bin` and `ffsys.bin`. Installable ZIPs also include
the update DAT under the body's `update_filename` (`FPUPDATE.DAT`,
`FWUP0009.DAT`, …). Archives follow the naming format in
[RELEASE-DOWNLOADS.md](RELEASE-DOWNLOADS.md#download-filenames). Files inside each
archive keep the names the camera requires.

The FJMODP01 interpreter verifies the source, patch program and every output
before download. Command targets 0, 1 and 2 identify `ffboot`, `ffsys` and
`fpupdate`. The original DAT is never substituted for a modified update.
