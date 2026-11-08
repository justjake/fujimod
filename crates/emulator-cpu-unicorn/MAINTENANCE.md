# Cortex-A7 translation maintenance and immutable backing

The Cortex-A7 backend implements ACTLR reset and access permissions, including
Non-secure SMP writes gated by NSACR. This is architectural register behavior,
not a cache timing or physical coherence model.

An optional translation owner attaches before the first execution. Accepted
TLBIALLIS instructions record their execution-time regime in a bounded queue.
The board owner selects recipients, performs local peer invalidations and
acknowledges the exact event only after delivery. Attachment alone does not
provide cross-core delivery. Unsupported shared operations poison the owner;
pending or failed work cannot be bypassed by execution or CPU-only snapshots.
Standalone processors retain their ordinary behavior.

The owner must serialize participating processors and settle accepted events
before running a peer. Public host mutations are refused during attached
execution; private architectural callbacks retain their required register
updates. Events describe accepted guest instructions rather than raw host
coprocessor setup. Queue saturation yields after the accepted instruction.

Frozen backing mappings retain their allocation policy across unmaps. Host
writes, permission escalation, ordinary aliases and unvalidated CPU restores
refuse. Mapping failure with ambiguous backing poisons the CPU. Allocation
owners must retain storage until every attached processor is destroyed and
must never mutate its raw pointer after publication.

## Tests

`cargo test -p fujimod-emulator-cpu-unicorn --lib` covers ACTLR security and
trap controls, translation owner attachment and event delivery boundaries,
and frozen mapping, alias and failure controls. Cross-core delivery is tested
by the XP4 board.
