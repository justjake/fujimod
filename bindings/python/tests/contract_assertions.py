"""Runtime narrowing for assertions about tagged service replies."""


def expect[T](value: object, cls: type[T]) -> T:
    assert isinstance(value, cls), (type(value), cls)
    return value
