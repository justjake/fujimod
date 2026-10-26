# Saved cameras over simulated USB

`CameraModel.connectSimulated(savedCamera)` opens the selected `CameraStore`
record in a standalone Wasm worker. It stays on the Dev Tools page. Firmware
size and SHA-256 must match the catalog; the worker restores the record's
committed media before cold boot. Its worker and Rust USB owner last until the
camera is removed or the page ends. Repeated concurrent connects reuse the
same camera.

The catalog declares a separately prepared USB-connected graph as
`emulate.usb.bundle`, conventionally `runtime/<firmware-id>-usb/`. Its ABI 1
`wasm.json` must use session construction and declare `usb: {kind: 'exr2', node}`. Optional
`bootInputs` and `initialPositions` inside `usb` select physical contacts;
these do not supply firmware state. Cable, endpoint geometry, IRQ wiring and
cold media inputs belong to the prepared graph. No graph or debug setting is
silently added at connection time. Saved media retain their settings; firmware
that does not expose the diagnostic interface refuses connection.

`Exr2UsbBus` sends reset/setup/OUT/IN-token/IN operations through the worker's
serialized queue. Status is observational; `usb_step` explicitly advances one
millisecond of guest time. Standard descriptors are read from guest packets and
parsed into `EmulatedUsbDevice`. Vendor payloads remain opaque. Empty FIFOs
wait for guest execution; timeouts and unsupported operations reject. The
adapter never fabricates descriptors, identities or completion packets.

`usb.attachWebUsbDevice(webusb, id)` opens the adapter through nusb and returns
the normal Rust-owned `Device`. Repeated attachment of the same object reuses
its owner. Physical USB objects must use discovery or the picker.
`usb.detachWebUsbDevice(device)` publishes normal loss events when the worker
fails. It does not replace `navigator.usb` or add the simulation to physical
discovery. Local sessions, remote controllers and decoded log events use the
same Rust ownership lock as physical cameras. Sharing attaches peers; it does
not transfer device ownership. External host transactions cannot be rewound,
so USB execution refuses Undo/history and does not create automatic checkpoints.

Builds expose `connectSimulated` only when their catalog includes a USB runtime.
The site staging script copies its module, binary, manifest and notices. The
browser check `simulated-camera.browser.ts` requires a USB bundle for every
emulated catalog firmware and a matching `/local/firmware/<id>.dat` mount. For
each entry it creates a saved camera in a fresh browser context, selects its
tile, checks the guest model through the Rust owner and its decoded log, verifies
no navigation, and removes it. Missing bundles and failed connections fail the
check. `FUJIMOD_TEST_FIRMWARE=<id>` selects one entry during bring-up; omit it
for full catalog verification. Per-firmware receipts and screenshots, plus the
complete `results.json`, go to `build/simulated-camera/`.
