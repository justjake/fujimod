# Firmware download gates

`src/flags.ts` controls the release gates `allowFpUpdateDownload` and
`allowSdCardBootDownload`. Both default to `false`. With both disabled, a
recognized DAT launches the camera directly. With an available download enabled,
dropping or choosing a DAT offers that download, Launch camera, and Cancel.
Restart, local automatic boot and replay keep launching directly.

SD downloads use the verified release patch to create `sd-card.zip`, containing
`ffboot.bin` and `ffsys.bin` at its root.

FP downloads require a modified update supplied by the release patch; the
original dropped DAT is never offered as a modified update. Existing releases
only contain SD outputs. A release can additionally declare `outputs.fpupdate`
with its byte count and SHA-256 in FJMODP01 metadata, and use command target 2
for that output (targets 0 and 1 remain ffboot and ffsys). The interpreter verifies
the source, patch program and every output before download. The FP gate alone
does not create an installable update; the release producer must provide one.
The browser downloads that output as `FPUPDATE.DAT`.
