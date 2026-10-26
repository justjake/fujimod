"""Paused isolated-caller services over an already constructed Board."""

import json

from fujimod._native import emulator as _native

from . import registers as regs
from .board import Board
from .processor import Processor


class RoutineBoard:
    """Use the board's existing native Session for bounded ARM calls.

    Device construction finishes before this adapter is created. Identity
    words and input metadata are explicit construction facts, recorded in
    each call result. Host stop reasons are local caller instrumentation.
    """

    def __init__(
        self,
        board: Board,
        *,
        cpu_identity: tuple[int, ...],
        device_graph: dict[str, object],
        input_manifest: dict[str, object],
    ) -> None:
        if len(cpu_identity) != len(board.cpus) or any(
            type(value) is not int or not 0 <= value < 1 << 32 for value in cpu_identity
        ):
            raise ValueError("Declare one unsigned ARM identity word per processor")
        self._board = board
        self.identity = list(cpu_identity)
        self.cpus = board.cpus
        self.quantum = board.quantum
        self.selected_core = 0
        self.transaction_limit = 4096
        self.executing_cpu = None
        self.failure = None
        self.transactions = []
        self.device_graph = json.loads(json.dumps(device_graph))
        self.input_manifest = json.loads(json.dumps(input_manifest))
        if set(self.input_manifest) - {"scene", "assets"}:
            raise ValueError("Caller input manifest accepts scene and asset identities")

    @property
    def native(self) -> _native.Board:
        return self._board.native

    @property
    def instructions(self) -> int:
        return self._board.instructions

    @property
    def virtual_ticks(self) -> int:
        return self._board.virtual_ticks

    def read_memory(self, address: int, size: int, cpu_id: int = 0) -> bytes:
        return self._board.read_memory(address, size, cpu_id)

    def write_memory(self, address: int, data: bytes, cpu_id: int = 0) -> None:
        self._board.write_memory(address, data, cpu_id)

    def state(self) -> dict[str, object]:
        return self._board.state()

    def attach(self, device: object) -> None:
        raise ValueError(
            "Declare all native devices in the resolved graph before constructing the Board"
        )

    def _stop(self, cpu: Processor, core: int, message: str, **details: object) -> None:
        if self.failure is None:
            self.failure = dict(
                message=message, cpu=core, pc=hex(cpu.reg_read(regs.PC)), **details
            )

    def close(self) -> None:
        self._board.close()
