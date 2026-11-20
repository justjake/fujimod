# Reproduce and document X-Pro2 5.11 emulation research

X-Pro2 experiments had accumulated in a private laboratory layout, making the
results difficult to reproduce or continue. This adds an English, source-only
research handoff with checked input identities, deterministic profile/image
preparation, exact historical engine changes and an explicit current-upstream
port inventory.

The reproduced reference-ROM laboratory runs the original NOR path and an SD
image containing the FujiMods SETUP folder, independent menu/indicator opacity,
menu themes and extended framing guides. It does not qualify installation,
button mapping, ROM identity or recovery on a physical X-Pro2, and does not
claim AF speed or power improvements.

The branch intentionally adds research material rather than replacing current
engine APIs with historical versions. The engine reference patch is pinned to
its validated base. Firmware, ROM, reference-module bytes, personal images,
serials, raw captures and private Git history are excluded. Readers supply the
three hash-bound public inputs locally.

Validation and exact coverage are recorded in `research/xpro2/VALIDATION.md` and
`research/xpro2/data/validation.json`. Integration conflicts and paths already
covered upstream are listed separately; historical successes are not presented
as current-runtime test results.

Related research: #355 (stable ROM prefix vs peripheral/SRAM over-read) and
#321 (service/reset workflow). Review of the input boundaries, model assumptions
and proposed runtime-port sequence would be particularly useful.
