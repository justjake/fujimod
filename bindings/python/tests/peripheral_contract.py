"""The same public value contract runs on native Python and real Pyodide."""

import json
from collections.abc import Callable

from fujimod import catalog
from fujimod.catalog import service as peripheral_definitions
from fujimod.usb import app80


def check_peripheral_values() -> bool:
    slots = {}
    entries = []
    for index, (codec, count) in enumerate([
        ("register_control", 7),
        ("i2c", 14),
        ("accumulator", 7),
        ("peripheral_report", 8),
        ("battery_control", 2),
        ("initialize_display", 1),
        ("display_pattern", 5),
    ]):
        slots[codec] = [(n * 5 + 7) % 32 for n in range(count)]
        entries.append(
            dict(
                code=200 + index,
                label=codec,
                codec=codec,
                slots=slots[codec],
                enabled_modes=4,
                next_mode=0,
                firmware_kind=19,
                handler=0,
                parameter_table=0,
            )
        )
    document = dict(
        schema=1,
        firmware=dict(
            id="synthetic-peripheral-v2",
            model="Synthetic",
            version="2",
            sha256="ab" * 32,
        ),
        mailbox=dict(page=3, offset=576, configuration_limit=512),
        entries=entries,
    )
    definition = peripheral_definitions.Definition.from_json(
        json.dumps(document),
        catalog.FirmwareIdentity("synthetic-peripheral-v2", "ab" * 32),
    )
    service = app80.Service(definition)
    modes = peripheral_definitions.ServiceModes(4)

    def prepare(command: app80.Command) -> app80.Invocation:
        return service.prepare(modes, command)

    def logical(invocation: app80.Invocation, codec: str) -> list[int]:
        return [invocation.raw[1 + s] for s in slots[codec]]

    def decode(
        invocation: app80.Invocation, codec: str, values: list[int]
    ) -> app80.ReplyValue:
        raw = bytearray(invocation.raw)
        for slot, value in zip(slots[codec], values, strict=True):
            raw[1 + slot] = value
        reply = invocation.decode(raw)
        assert reply.raw == raw
        return reply.value

    def failure(function: Callable[[], object]) -> None:
        try:
            function()
        except app80.CommandError as error:
            assert isinstance(error.reason, app80.CommandFailure.Argument)
            assert error.reason.kind == app80.ArgumentKind.I2cLength
        else:
            raise AssertionError("invalid I2C length accepted")

    R = app80.RegisterAction
    for action, expected in [
        (R.AudioValue(value=0x56), [0, 0, 0, 0, 0x56, 0, 0]),
        (R.DisplayFirst(selector=7, value=0x1234), [1, 0, 7, 0x12, 0x34, 0, 0]),
        (R.DisplayStub(selector=7, value=0x1234), [2, 0, 7, 0x12, 0x34, 0, 0]),
        (R.DisplaySecond(selector=7, value=0x1234), [3, 0, 7, 0x12, 0x34, 0, 0]),
        (R.PowerWrite(register=0x1234, value=0x56), [4, 0x12, 0x34, 0, 0x56, 0, 0]),
        (
            R.PositionPair(first=0x1234, second=0x5678),
            [5, 0x12, 0x34, 0x56, 0x78, 0, 0],
        ),
        (R.NoopFirst(), [7, 0, 0, 0, 0, 0, 0]),
        (R.NoopSecond(), [8, 0, 0, 0, 0, 0, 0]),
    ]:
        invocation = prepare(app80.Command.RegisterControl(action=action))
        assert invocation.raw[0] == 200
        assert logical(invocation, "register_control") == expected
        assert isinstance(
            invocation.decode(invocation.raw).value, app80.ReplyValue.NoPayload
        )

    for length in [0, 11]:
        failure(lambda length=length: app80.I2cLength(length))
        failure(lambda length=length: app80.I2cPayload(bytes(length)))
    assert app80.I2cLength(10).value == 10
    assert len(app80.I2cPayload(bytes(10)).data) == 10
    assert not hasattr(app80.I2cPort, "Port3")
    for port, selector in [
        (app80.I2cPort.Port0, 0),
        (app80.I2cPort.Port1, 1),
        (app80.I2cPort.Port2, 2),
        (app80.I2cPort.Port4, 4),
        (app80.I2cPort.Port5, 5),
    ]:
        action = app80.I2cAction.Read(
            port=port, command=0xA5, length=app80.I2cLength(3)
        )
        invocation = prepare(app80.Command.I2c(action=action))
        assert invocation.raw[0] == 201
        assert logical(invocation, "i2c") == [0, selector, 3, 0xA5] + [0] * 10
        value = decode(invocation, "i2c", [0, selector, 3, 0xA5, 1, 2, 3] + [99] * 7)
        assert (
            isinstance(value, app80.ReplyValue.I2cData)
            and value.data == b"\x01\x02\x03"
        )
        original = bytearray([4, 5, 6])
        payload = app80.I2cPayload(original)
        original[:] = bytes(3)
        assert payload.data == b"\x04\x05\x06"
        action = app80.I2cAction.Write(port=port, command=0x12, payload=payload)
        invocation = prepare(app80.Command.I2c(action=action))
        assert logical(invocation, "i2c") == [1, selector, 3, 0x12, 4, 5, 6] + [0] * 7
        assert isinstance(
            invocation.decode(invocation.raw).value, app80.ReplyValue.NoPayload
        )
    invocation = prepare(
        app80.Command.I2c(action=app80.I2cAction.WriteSingle(command=0xA5, value=0x7F))
    )
    assert logical(invocation, "i2c") == [1, 3, 1, 0xA5, 0x7F] + [0] * 9
    assert isinstance(
        invocation.decode(invocation.raw).value, app80.ReplyValue.NoPayload
    )
    try:
        app80.I2cAction.Read(port=3, command=0xA5, length=app80.I2cLength(1))  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    except TypeError:
        pass
    else:
        raise AssertionError("raw integer port accepted")

    start = prepare(app80.Command.Accumulator(action=app80.AccumulatorAction.Start))
    assert logical(start, "accumulator")[0] == 1
    stop = prepare(
        app80.Command.Accumulator(action=app80.AccumulatorAction.StopAndRead)
    )
    assert logical(stop, "accumulator")[0] == 0
    value = decode(stop, "accumulator", [0, 0x12, 0x34, 0xAB, 0xCD, 0xFF, 0xFE])
    assert isinstance(value, app80.ReplyValue.Accumulated)
    assert value.values == (0x1234, 0xABCD, 0xFFFE)
    report = prepare(app80.Command.PeripheralReport())
    value = decode(report, "peripheral_report", [1, 2, 3, 4, 5, 6, 7, 8])
    assert isinstance(value, app80.ReplyValue.PeripheralReport)
    assert value.data == bytes([1, 2, 3, 4, 5, 6, 7, 8])
    for action, selector in [
        (app80.BatteryAction.ResetPending, 0),
        (app80.BatteryAction.CallbackWait, 1),
    ]:
        invocation = prepare(app80.Command.BatteryControl(action=action))
        assert logical(invocation, "battery_control") == [selector, 0]
        assert isinstance(
            decode(invocation, "battery_control", [255, 255]),
            app80.ReplyValue.NoPayload,
        )
    invocation = prepare(app80.Command.InitializeDisplay())
    assert logical(invocation, "initialize_display") == [0]
    assert isinstance(
        invocation.decode(invocation.raw).value, app80.ReplyValue.NoPayload
    )
    for action, expected in [
        (app80.DisplayPatternAction.ResetPreset(), [0, 0, 0, 0, 0]),
        (app80.DisplayPatternAction.Bars(), [1, 0, 0, 0, 0]),
        (
            app80.DisplayPatternAction.Solid(repeated=18, middle=52, low=86),
            [2, 0, 18, 86, 52],
        ),
        (
            app80.DisplayPatternAction.Solid(repeated=255, middle=0, low=255),
            [2, 0, 255, 255, 0],
        ),
    ]:
        invocation = prepare(app80.Command.DisplayPattern(action=action))
        assert logical(invocation, "display_pattern") == expected
        assert isinstance(
            decode(invocation, "display_pattern", [255] * 5), app80.ReplyValue.NoPayload
        )
    for create in [
        lambda: app80.Command.BatteryControl(action=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: app80.Command.DisplayPattern(action=app80.BatteryAction.ResetPending),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("wrong battery/display action type accepted")
    return True
