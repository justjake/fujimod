from _fujimod_service_values import ArgumentKind

from ._variants import Sum
from .catalog.service import Codec

class CommandFailure(Sum):
    class Unsupported(CommandFailure):
        codec: Codec
        def __init__(self, codec: Codec) -> None: ...

    class Ambiguous(CommandFailure):
        codec: Codec
        def __init__(self, codec: Codec) -> None: ...

    class Unavailable(CommandFailure):
        code: int
        def __init__(self, code: int) -> None: ...

    class Argument(CommandFailure):
        kind: ArgumentKind
        def __init__(self, kind: ArgumentKind) -> None: ...

    class OversizeArguments(CommandFailure): ...
