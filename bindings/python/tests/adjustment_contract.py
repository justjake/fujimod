"""Native/Pyodide contract for direct adjustment requests and snapshots."""

from contract_assertions import expect
from fujimod.usb import app80


def check_adjustment_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("lens_operation6", 0),
        ("publish_lens_completion_parameter", 2),
        ("prepare_adjustment_output", 0),
        ("prepare_alternate_adjustment_output", 0),
        ("lens_bounded_position", 3),
        ("lens_fixed_position", 1),
        ("invoke_primary_parameter_callback", 2),
        ("invoke_secondary_parameter_callback", 2),
        ("query_adjustment_option", 4),
    ]
    slots = {name: [(i * 5 + 7) % 32 for i in range(count)] for name, count in layouts}
    doc = {
        "schema": 1,
        "firmware": {
            "id": "adjustment-b",
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
            json.dumps(doc), catalog.FirmwareIdentity("adjustment-b", "ab" * 32)
        )
    )
    modes = catalog.service.ServiceModes(4)

    def prepare(
        command: app80.Command, codec: str, expected: list[int]
    ) -> app80.Invocation:
        p = service.prepare(modes, command)
        assert [p.raw[1 + i] for i in slots[codec]] == expected
        return p

    def decode(p: app80.Invocation, codec: str, values: list[int]) -> app80.ReplyValue:
        data = bytearray(p.raw)
        for slot, byte in zip(slots[codec], values, strict=True):
            data[1 + slot] = byte
        reply = p.decode(data)
        assert reply.raw == data
        return reply.value

    for command, codec in [
        (a.Command.LensOperation6(), "lens_operation6"),
        (a.Command.PrepareAdjustmentOutput(), "prepare_adjustment_output"),
        (
            a.Command.PrepareAlternateAdjustmentOutput(),
            "prepare_alternate_adjustment_output",
        ),
    ]:
        p = prepare(command, codec, [])
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    p = prepare(
        a.Command.PublishLensCompletionParameter(),
        "publish_lens_completion_parameter",
        [255, 255],
    )
    for raw, cls in [
        (0, a.LensCompletionParameterFlag.Other),
        (1, a.LensCompletionParameterFlag.Parameter80),
        (255, a.LensCompletionParameterFlag.Unknown),
    ]:
        value = decode(p, "publish_lens_completion_parameter", [raw, 255])
        assert isinstance(value, a.ReplyValue.LensCompletionParameter) and isinstance(
            value.flag, cls
        )
        assert value.encoded_value == 255
        if raw == 255:
            assert (
                expect(value.flag, a.LensCompletionParameterFlag.Unknown).raw_status
                == 255
            )
    for position, argument in [(0, 255), (255, 0), (18, 52)]:
        p = prepare(
            a.Command.LensBoundedPosition(position=position, value=argument),
            "lens_bounded_position",
            [position, argument, 255],
        )
        for raw, cls in [
            (0, a.LensPositionRange.NotRejected),
            (1, a.LensPositionRange.OutOfRange),
            (255, a.LensPositionRange.Unknown),
        ]:
            value = decode(p, "lens_bounded_position", [position, argument, raw])
            assert isinstance(value, a.ReplyValue.LensPosition) and isinstance(
                value.range, cls
            )
            if raw == 255:
                assert (
                    expect(value.range, a.LensPositionRange.Unknown).raw_status == 255
                )
    p = prepare(a.Command.LensFixedPosition(), "lens_fixed_position", [255])
    assert (
        expect(
            decode(p, "lens_fixed_position", [0]), a.ReplyValue.LensOperation
        ).raw_value
        == 0
    )
    for command, codec in [
        (
            a.Command.InvokePrimaryParameterCallback(parameter=255),
            "invoke_primary_parameter_callback",
        ),
        (
            a.Command.InvokeSecondaryParameterCallback(parameter=255),
            "invoke_secondary_parameter_callback",
        ),
    ]:
        p = prepare(command, codec, [255, 255])
        for raw in [0, 1, 255]:
            value = decode(p, codec, [255, raw])
            assert (
                isinstance(value, a.ReplyValue.AdjustmentCallback)
                and value.raw_value == raw
            )
    option = a.AdjustmentOptionByte(255)
    assert option.value == 255
    p = prepare(
        a.Command.QueryAdjustmentOption(option=option),
        "query_adjustment_option",
        [255, 255, 255, 255],
    )
    for raw, cls in [
        (0, a.AdjustmentOptionStatus.ReportedSuccess),
        (1, a.AdjustmentOptionStatus.ReportedFailure),
        (255, a.AdjustmentOptionStatus.Unknown),
    ]:
        value = decode(p, "query_adjustment_option", [255, 137, 171, raw])
        assert isinstance(value, a.ReplyValue.AdjustmentOption) and isinstance(
            value.status, cls
        )
        assert value.raw_value == 0x89AB
        if raw == 255:
            assert (
                expect(value.status, a.AdjustmentOptionStatus.Unknown).raw_status == 255
            )
    for create in [
        lambda: a.Command.QueryAdjustmentOption(option=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        lambda: a.Command.QueryAdjustmentOption(option=a.TransferPayloadByte(1)),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            create()
        except TypeError:
            pass
        else:
            raise AssertionError("untyped/foreign option byte accepted")
    return True
