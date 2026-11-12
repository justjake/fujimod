# Release manifest tool

`fujimod-release-manifest` writes the files served for an OTA release:
`latest.json` and one content-addressed store file per distinct file. It
takes the firmware id, the release version and `path=file` pairs:

```sh
cargo run -p fujimod-release-manifest -- \
  --firmware example-cam --version 1.2.3 --output release \
  /etc/ssl/cert.pem=cert.pem \
  /firmware.patch=firmware.patch
```

The output directory is the release directory published at
`/ota/<firmware-id>/`. It receives `latest.json` and each store file; it is
created when absent. The first `=` separates the manifest path from the host
file, so a host file name may contain `=`.

## Manifest

`latest.json` names the firmware, the version and every published path with
its byte size and lowercase SHA-256:

```json
{
  "firmware": "example-cam",
  "version": "1.2.3",
  "date": "2026-11-10",
  "commit": "abcdef0123456789abcdef0123456789abcdef01",
  "notes": ["First release note", "Second release note"],
  "fs": {
    "/etc/ssl/cert.pem": {
      "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      "size": 0
    },
    "/firmware.patch": {
      "sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
      "size": 3
    }
  }
}
```

The `date`, `commit` and `notes` fields are optional. Notes contain at most
eight strings, each no longer than 64 UTF-8 bytes.

Paths are absolute, use `/` separators and contain no empty, `.` or `..`
segments and no trailing slash. Entries are sorted by path so the same inputs
produce the same bytes. The firmware id starts with an ASCII letter or digit
and contains only letters, digits, `-`, `_` or `.`.

## Store names

A store file is named from the first 11 uppercase hexadecimal digits of its
SHA-256: eight digits, a dot and three more (`XXXXXXXX.YYY`). The camera's
update layer uses the same rule; the tests pin its vectors:

| Content | SHA-256 | Store name |
| --- | --- | --- |
| empty | `e3b0c442…b855` | `E3B0C442.98F` |
| `abc` | `ba7816bf…15ad` | `BA7816BF.8F0` |

Two files whose store names match but whose full digests differ make the
command fail before it writes anything, because the camera would see two
different contents under one name. Identical content listed under several
paths writes one store file.

## Writes

Each source file is hashed while it is read, then copied through a temporary
name in the output directory and renamed into place. A store file already
present is reused only when its size and SHA-256 match; different contents
stop the command. `latest.json` is written last, through a temporary file,
so a manifest never names a store file that failed to appear. If a source
changes between hashing and copying, the copy is discarded.

## Tests

```sh
cargo test -p fujimod-release-manifest
```

Unit tests cover the store-name vectors, the manifest bytes, collisions and
input validation. Integration tests run the command and inspect its output.
