# Credits

This file credits contributed work that is not covered by a license file
elsewhere in the repository.

## Taylor Chapman and Claude

Contributed by **Taylor Chapman (heytchap)** and **Claude (Anthropic)**,
from the X100F 2.12 SD card bring-up, 2026-11:

- The optional SD card timing model for the SDHCI host
  (`crates/emulator-ip/src/sdhci/latency.rs` and its wiring through the
  SDHCI host and the X-Processor Pro SD wrapper): seeded per-command latency,
  read access and per-block time, write program busy and stalls, and
  post-write garbage collection.
- In the Unicorn fork: the Alignment fault for NEON element and structure
  loads and stores whose alignment qualifier the address does not meet.

Any use, redistribution or derived work of these contributions must retain
this credit to Taylor Chapman (heytchap) and Claude (Anthropic).
