# XP4 board assembly

`Definition` constructs a serialized ARMv7-A cluster with declared RAM,
processor, interrupt and device inputs. Unsupported operations refuse rather
than assigning successful peripheral responses.

Each processor owns the pinned Cortex-A7 backend. Accepted shared translation
maintenance is delivered to participating peers before another processor runs.
A failed delivery remains sticky; public mutations cannot bypass it. Diagnostic
snapshots expose accepted events and peer outcomes without advancing execution.

Optional devices are explicit constructor inputs. Initial GIC registers are
validated against the modeled interface; immutable NOR prefixes retain a
read/execute-only canonical allocation; byte I2C controllers require declared
peers and timing. Duplicate physical window ownership refuses before mappings
are installed. Multiple devices with the same name retain separate diagnostic
keys instead of overwriting one another.

The optional transmit receiver is discovery-only. Its bounded offer records
accepted bytes and does not identify an attached physical endpoint. Strict
boards reject it.

Isolated driver guards admit device registers and model-initiated RAM
transactions only through an explicit per-call device policy (see
`crates/emulator/src/board/harness/arm32.md`). These guards remain distinct
from the ordinary whole-board execution path.

Tests beside the implementation cover coordination, physical-memory ownership,
frozen NOR execution, constructor/refusal paths and receiver admission. These component checks do not establish
a complete stock firmware boot, a saved photo or hardware timing fidelity.
