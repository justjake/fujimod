"""Paused CPU inspection for host debuggers and isolated driver callers."""

import json
import weakref
from enum import Enum, auto
from typing import TYPE_CHECKING

from . import registers as regs

if TYPE_CHECKING:
    from .board import Board


RegisterIdentifier = int | str | dict[str, object]


class InstructionAction(Enum):
    """Explicit host control after a passive instruction observation."""

    CONTINUE = auto()
    PAUSE = auto()


def _register(identifier: RegisterIdentifier) -> RegisterIdentifier:
    if isinstance(identifier, (str, dict)):
        return identifier
    if identifier in regs.GENERAL:
        result: dict[str, object] = {"Arm32": {"R": regs.GENERAL.index(identifier)}}
        return result
    registers: dict[int, RegisterIdentifier] = {
        regs.PC: "Pc",
        regs.SP: "Sp",
        regs.LR: {"Arm32": {"R": 14}},
        regs.CPSR: "Pstate",
        regs.SPSR: {"Arm32": "Spsr"},
        regs.CPACR: {"Arm32": "Cpacr"},
        regs.FPEXC: {"Arm32": "Fpexc"},
        regs.FPSCR: {"Arm32": "Fpscr"},
        regs.FPSID: {"Arm32": "Fpsid"},
    }
    return registers[identifier]


class Processor:
    def __init__(self, board: Board, core: int) -> None:
        self.board: Board = weakref.proxy(board)
        self.core = core

    def reg_read(self, register: RegisterIdentifier) -> int:
        return self.board.native.reg_read(self.core, json.dumps(_register(register)))

    def reg_write(self, register: RegisterIdentifier, value: int) -> None:
        self.board.native.reg_write(self.core, json.dumps(_register(register)), value)

    def cpr_read(
        self, cp: int, opc1: int, crn: int, crm: int, opc2: int, secure: int, wide: bool
    ) -> int:
        return self.board.native.coprocessor(
            self.core,
            json.dumps(
                dict(
                    cp=cp,
                    opc1=opc1,
                    crn=crn,
                    crm=crm,
                    opc2=opc2,
                    secure=secure,
                    wide=wide,
                )
            ),
            None,
        )

    def cpr_write(
        self,
        cp: int,
        opc1: int,
        crn: int,
        crm: int,
        opc2: int,
        secure: int,
        wide: bool,
        value: int,
    ) -> int:
        return self.board.native.coprocessor(
            self.core,
            json.dumps(
                dict(
                    cp=cp,
                    opc1=opc1,
                    crn=crn,
                    crm=crm,
                    opc2=opc2,
                    secure=secure,
                    wide=wide,
                )
            ),
            value,
        )

    def mem_read(self, address: int, size: int) -> bytes:
        return self.board.native.mem_read(self.core, address, size)

    def mem_write(self, address: int, data: bytes | bytearray | memoryview) -> None:
        self.board.native.mem_write(self.core, address, bytes(data))


def rprofile_boundary(board: Board) -> tuple[object, ...]:
    """Paused native CPU, IRQ and scheduler observation for R-profile clients."""
    import copy

    native = board.native
    state = json.loads(native.inspection_observation())
    registers = tuple(
        tuple(
            native.reg_read(core, register)
            for register in ('"Pc"', '{"Arm32":{"R":13}}', '"Pstate"')
        )
        for core in range(len(state["instructions"]))
    )
    controllers = tuple(state["interrupt_controllers"])
    return (
        sum(state["instructions"]),
        native.now,
        copy.deepcopy(board.failure),
        registers,
        controllers,
        state["timeline"],
        state["released"],
        state["waiting"],
        state["events"],
        state["next_core"],
        state["slice_left"],
    )
