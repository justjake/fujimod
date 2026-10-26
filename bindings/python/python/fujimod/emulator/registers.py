"""ARM core register identifiers for the native board, without importing unicorn.

Client IDs use Unicorn 2.1.4's number table; the binding decodes them to typed
register names before calling a CPU backend. ESR is an executor diagnostic
included in the explicit checkpoint, not an R5F hardware register.
IDs are accepted by ``cpu.reg_read``
and ``cpu.reg_write`` on ``Board.cpus``. ``NAMES`` maps lower-case names
(``r0``..``r12``, ``sp``, ``lr``, ``pc``, ``cpsr``, ``spsr``, ``fpexc``,
``cpacr``) to those identifiers; ``sp``/``lr``/``spsr`` select the bank of the
current CPSR mode.

Unicorn has no constants for the banked registers of the other modes, so
``BANKED`` names them (``sp_abt``, ``lr_irq``, ``spsr_und`` ...) as
``(mode, base register)``. ``read_register``/``write_register`` reach them by
selecting the mode through CPSR and restoring the original CPSR, which is only
valid on a paused core (never from inside a breakpoint handler). USR and SYS
share one bank (``sp_usr``/``sp_sys``, ``lr_usr``/``lr_sys``); neither has an SPSR.
"""

from typing import Protocol

# Stable Unicorn 2.1.4 ARM register identifiers. Kept local so importing the
# board and checkpoint code does not require the optional Python package.
CPACR, CPSR, FPEXC = 111, 3, 4
MEM_READ = 16
MEM_WRITE = 17
HOOK_MEM_READ, HOOK_MEM_WRITE = 1024, 2048
PROT_READ, PROT_WRITE, PROT_EXEC, PROT_ALL = 1, 2, 4, 7
LR, PC, SP, SPSR = 10, 11, 12, 13
R0, R1, R2, R3, R4, R5, R6, R7 = range(66, 74)
R8, R9, R10, R11, R12 = range(74, 79)
ESR, FPSCR, FPSID, Q0 = 140, 6, 8, 50


GENERAL = (R0, R1, R2, R3, R4, R5, R6, R7, R8, R9, R10, R11, R12)
NAMES = {
    **{f"r{index}": register for index, register in enumerate(GENERAL)},
    "sp": SP,
    "lr": LR,
    "pc": PC,
    "cpsr": CPSR,
    "spsr": SPSR,
    "fpexc": FPEXC,
    "cpacr": CPACR,
}

MODES = {
    "usr": 0x10,
    "fiq": 0x11,
    "irq": 0x12,
    "svc": 0x13,
    "abt": 0x17,
    "und": 0x1B,
    "sys": 0x1F,
}


def _banked_registers() -> dict[str, tuple[int, int]]:
    banked: dict[str, tuple[int, int]] = {}
    for mode, number in MODES.items():
        banked[f"sp_{mode}"] = (number, SP)
        banked[f"lr_{mode}"] = (number, LR)
        if mode not in ("usr", "sys"):
            banked[f"spsr_{mode}"] = (number, SPSR)
    return banked


BANKED = _banked_registers()
del _banked_registers


class _RegisterAccess(Protocol):
    def reg_read(self, register: int) -> int: ...

    def reg_write(self, register: int, value: int) -> None: ...


def read_register(cpu: _RegisterAccess, name: str) -> int:
    """Read a plain or banked register on a paused core."""
    if name in NAMES:
        return cpu.reg_read(NAMES[name])
    if name not in BANKED:
        raise ValueError("Unknown register: " + str(name))
    mode, register = BANKED[name]
    cpsr = cpu.reg_read(CPSR)
    if cpsr & 31 == mode:
        return cpu.reg_read(register)
    cpu.reg_write(CPSR, (cpsr & ~31) | mode)
    try:
        return cpu.reg_read(register)
    finally:
        cpu.reg_write(CPSR, cpsr)


def write_register(cpu: _RegisterAccess, name: str, value: int) -> None:
    """Write a plain or banked register on a paused core; CPSR is restored."""
    if name in NAMES:
        cpu.reg_write(NAMES[name], value)
        return
    if name not in BANKED:
        raise ValueError("Unknown register: " + str(name))
    mode, register = BANKED[name]
    cpsr = cpu.reg_read(CPSR)
    if cpsr & 31 == mode:
        cpu.reg_write(register, value)
        return
    cpu.reg_write(CPSR, (cpsr & ~31) | mode)
    try:
        cpu.reg_write(register, value)
    finally:
        cpu.reg_write(CPSR, cpsr)


__all__ = [
    "R0",
    "R1",
    "R2",
    "R3",
    "R4",
    "R5",
    "R6",
    "R7",
    "R8",
    "R9",
    "R10",
    "R11",
    "R12",
    "SP",
    "LR",
    "PC",
    "CPSR",
    "SPSR",
    "FPEXC",
    "CPACR",
    "ESR",
    "FPSCR",
    "FPSID",
    "Q0",
    "MEM_READ",
    "MEM_WRITE",
    "HOOK_MEM_READ",
    "HOOK_MEM_WRITE",
    "PROT_READ",
    "PROT_EXEC",
    "PROT_ALL",
    "GENERAL",
    "NAMES",
    "MODES",
    "BANKED",
    "read_register",
    "write_register",
]
