"""Backend-independent CPU execution faults."""

INVALID_INSTRUCTION = "invalid-instruction"


class NativeExecutionError(Exception):
    """A CPU execution fault, distinct from a host callback failure."""

    def __init__(self, kind: str, message: str) -> None:
        self.kind = str(kind)
        super().__init__(str(message))
