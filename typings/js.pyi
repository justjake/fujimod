from collections.abc import Awaitable, Callable

from _fujimod_js import JsBytes

class Uint8Array:
    @staticmethod
    def new(value: object) -> JsBytes: ...

class Promise:
    @staticmethod
    def new(
        executor: Callable[[Callable[[object], None], Callable[[object], None]], None],
    ) -> Awaitable[object]: ...
