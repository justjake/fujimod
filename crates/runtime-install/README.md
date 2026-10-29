# Runtime installer

`fujimod-runtime-install` places a caller-built image in an allocation, reads
back its upload, synchronizes executable memory, applies page permissions,
and invokes a caller-supplied control stub to install hooks and run initializers.
Tal Shiri's [X-Turbo](https://github.com/tals/X-Turbo) supplied the AArch64
relocation validation, ported with permission.

`RuntimeImage` is a versioned metadata object plus a separate byte buffer.
Serialize its metadata with serde and store the bytes in a binary file.
Relocations identify their destinations and resolved values explicitly. Unsupported
kinds, duplicate or overlapping destinations, opcode mismatches, unaligned
operands, and branch range overflow fail before an image can be uploaded.
`Placement::Relink` calls a builder at the allocated base; `Placement::Relocate`
uses the image's records. Hook words can carry ARM MOVW/MOVT pointer pairs.

Implement `Body` with the selected allocator, transport, cache operations for
all executing cores, permission changes, and callback entry mechanism. Use
`fujimod_usb::operations::callback::invoke` for guarded allocator and control
callbacks. `protect` may return success when the allocation is already executable.
`placed` lets the caller persist the exact placed bytes and lease before upload.

`Installer::install` prepares the image, publishes hooks with INSTALL, and runs
INIT. `prepare` and `publish` expose the same steps separately for callers that
need a policy check before publication. `status`, `rollback`, `init`, `set`, and
`read` reuse the lease. Every control operation verifies immutable code, the
cookie, a fresh caller token, the mailbox, and hook readback. Supply distinct
nonzero tokens to distinguish replies from previous operations.

Preparation failure frees the allocation. Once INSTALL is attempted, including
unknown completion, the region is retained even after rollback. Lease records
are serializable; their persistence and eventual reclamation belong to the
caller. Dropping an in-flight operation does not initiate asynchronous cleanup.
The caller must retain its recorded allocation when completion is unknown.

Python uses `fujimod.runtime_install.Installer`. Its awaitable `Body` supplies
`allocate`, `free`, `read`, `write`, `synchronize`, `protect`, `control`, and
`placed`; see that module's docstring for signatures. The native cooperative
adapter runs the same Rust installer and yields each body request for Python to
await. No Python callback runs while Rust owns a USB session.

Pass `relink=builder` to the Python installer to rebuild synchronously at the
allocated base. The callback receives the base and returns a `usb.RuntimeImage`
with the same allocation capacity and nonce. Omit it to relocate the prepared
image. The relinker runs between body operations, without owning a USB session.
