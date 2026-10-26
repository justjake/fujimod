"""Shared native/Pyodide direct service lens contract."""

from contract_assertions import expect
from fujimod.usb import app80


def check_lens_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("lens_select", 5),
        ("lens_operation2", 1),
        ("lens_operation3", 1),
        ("lens_operation4", 0),
        ("lens_operation5", 0),
    ]
    slots = {name: [(i * 5 + 7) % 32 for i in range(count)] for name, count in layouts}
    doc = {
        "schema": 1,
        "firmware": {
            "id": "lens-b",
            "model": "Synthetic",
            "version": "2",
            "sha256": "ab" * 32,
        },
        "mailbox": {"page": 3, "offset": 576, "configuration_limit": 512},
        "entries": [
            {
                "code": 180 + n,
                "label": codec,
                "codec": codec,
                "slots": slots[codec],
                "enabled_modes": 4,
                "next_mode": 0,
                "firmware_kind": 1,
                "handler": 0,
                "parameter_table": 0,
            }
            for n, (codec, _) in enumerate(layouts)
        ],
    }
    service = a.Service(
        catalog.service.Definition.from_json(
            json.dumps(doc), catalog.FirmwareIdentity("lens-b", "ab" * 32)
        )
    )
    modes = catalog.service.ServiceModes(4)

    def prepare(
        command: app80.Command, codec: str, expected: list[int]
    ) -> app80.Invocation:
        p = service.prepare(modes, command)
        assert [p.raw[1 + i] for i in slots[codec]] == expected
        return p

    for raw in [160, 255]:
        try:
            a.LensSelectionIndex(raw)
        except a.CommandError as error:
            assert isinstance(error.reason, a.CommandFailure.Argument)
            assert error.reason.kind == a.ArgumentKind.LensSelectionIndex
        else:
            raise AssertionError("invalid lens index accepted")
    for target, wire in [(a.LensTarget.Initialize(), 0)] + [
        (a.LensTarget.Index(index=a.LensSelectionIndex(n)), n + 1)
        for n in [0, 159, 161, 254]
    ]:
        p = prepare(
            a.Command.LensSelect(target=target), "lens_select", [wire, 0, 255, 255, 255]
        )
        for raw, cls in [
            (0, a.LensSelectionRange.NotRejected),
            (1, a.LensSelectionRange.OutOfRange),
            (255, a.LensSelectionRange.Unknown),
        ]:
            data = bytearray(p.raw)
            for i, value in [(2, raw), (3, 0)]:
                data[1 + slots["lens_select"][i]] = value
            value = p.decode(data).value
            assert isinstance(value, a.ReplyValue.LensSelection) and isinstance(
                value.range, cls
            )
            assert bytes(value.raw_detail) == bytes([0, 255])
            if raw == 255:
                assert (
                    expect(value.range, a.LensSelectionRange.Unknown).raw_status == 255
                )
    for command, codec in [
        (a.Command.LensOperation2(), "lens_operation2"),
        (a.Command.LensOperation3(), "lens_operation3"),
    ]:
        p = prepare(command, codec, [255])
        for raw in [0, 1, 255]:
            data = bytearray(p.raw)
            data[1 + slots[codec][0]] = raw
            value = p.decode(data).value
            assert (
                isinstance(value, a.ReplyValue.LensOperation) and value.raw_value == raw
            )
    for command, codec in [
        (a.Command.LensOperation4(), "lens_operation4"),
        (a.Command.LensOperation5(), "lens_operation5"),
    ]:
        p = prepare(command, codec, [])
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for create in [
        lambda: a.Command.LensSelect(target=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.LensTarget.Index(index=a.TransferPayloadByte(1)),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("foreign lens target/index accepted")
    return True
