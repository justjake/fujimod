# Browser camera library

Each supported firmware upload creates a separate virtual camera record. Firmware
lives in one library keyed by SHA-256: cameras made from the same DAT share it,
and Recovery lists and adds to the same library.
The idle emulator shows `Launcher.tsx` without importing the emulator or loading
its runtime: first use explains where firmware comes from and links each
emulated body's Fujifilm download; later visits offer Resume for each record
(most recently used first) and New for each stored firmware. Either reads the
DAT from IndexedDB and verifies it through the ordinary catalog path. Save &
Exit in the running viewer saves the camera and returns to the launcher.

`store.ts` owns the versioned `fujimod-cameras` database. Camera metadata, library
metadata, library bytes and save descriptors use separate stores. Version 2
moved version 1's per-camera firmware copies into the library. Durable bytes use immutable OPFS files,
with IndexedDB bytes as a fallback where OPFS writing is unavailable. A file
closes before an optimistic metadata transaction publishes its reference. Failed
writes preserve the previous successful save; stale revisions discard only the
unpublished candidate. Replaced generations are removed after publication.

The native engine provides live execution, owner-bound Undo and durable media
CBOR. Reopening restores committed SD/NAND storage, external power state and
persistent control positions before any guest instruction. CPU/RAM/device
contexts stay volatile. Camera firmware is retained separately in IndexedDB.

`tests/engine-storage.browser.ts` checks real worker OPFS publication, stale
revisions, quota failures, generation cleanup and reload in Chromium and WebKit.

Run store and component tests with `npm test`. Check lazy startup independently
in Chromium and WebKit with a development server:

```sh
FUJIMOD_WEB_URL=https://your-host.example/ node src/cameras/startup.browser.ts
```

This refreshes empty and populated libraries three times, asserting that no
firmware is selected, no emulator starts and records remain unchanged.
Screenshots go under ignored `build/camera-library/`.
