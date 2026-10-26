"""Native/Pyodide contract for adjustment status, exposure and packet commands."""

from contract_assertions import expect
from fujimod.usb import app80


def check_exposure_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    layouts = [
        ("adjustment_status_option", 1),
        ("adjustment_exposure", 4),
        ("adjustment_packet", 11),
        ("adjustment_ready_option", 1),
        ("report_service_success", 0),
    ]
    slots = {name: [(i * 5 + 7) % 32 for i in range(count)] for name, count in layouts}
    doc = {
        "schema": 1,
        "firmware": {
            "id": "exposure-b",
            "model": "Synthetic",
            "version": "2",
            "sha256": "ab" * 32,
        },
        "mailbox": {"page": 3, "offset": 576, "configuration_limit": 512},
        "entries": [
            {
                "code": 0x90 + n,
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
            json.dumps(doc), catalog.FirmwareIdentity("exposure-b", "ab" * 32)
        )
    )
    modes = catalog.service.ServiceModes(4)

    def prepare(
        command: app80.Command, codec: str, expected: list[int]
    ) -> app80.Invocation:
        prepared = service.prepare(modes, command)
        assert [prepared.raw[1 + i] for i in slots[codec]] == expected
        return prepared

    def decode(
        prepared: app80.Invocation, codec: str, values: list[int]
    ) -> app80.ReplyValue:
        data = bytearray(prepared.raw)
        for slot, value in zip(slots[codec], values, strict=True):
            data[1 + slot] = value
        reply = prepared.decode(data)
        assert bytes(reply.raw) == bytes(data)
        return reply.value

    status = prepare(
        a.Command.ReadAdjustmentStatusOption(), "adjustment_status_option", [255]
    )
    for raw, cls in [
        (0, a.ExposureStatusOption.Option0),
        (1, a.ExposureStatusOption.Option1),
        (255, a.ExposureStatusOption.Unknown),
    ]:
        value = decode(status, "adjustment_status_option", [raw])
        assert isinstance(value, a.ReplyValue.AdjustmentStatusOption)
        assert isinstance(value.value, cls)
        if raw == 255:
            assert expect(value.value, a.ExposureStatusOption.Unknown).value == 255

    ready = prepare(
        a.Command.ReadAdjustmentReadyOption(), "adjustment_ready_option", [255]
    )
    for raw, cls in [
        (0, a.ExposureReadiness.NotReady),
        (1, a.ExposureReadiness.Ready),
        (255, a.ExposureReadiness.Unknown),
    ]:
        value = decode(ready, "adjustment_ready_option", [raw])
        assert isinstance(value, a.ReplyValue.AdjustmentReadyOption)
        assert isinstance(value.value, cls)
        if raw == 255:
            assert expect(value.value, a.ExposureReadiness.Unknown).value == 255

    for mode, mode_byte in [
        (a.ExposureModeOption.Default, 0),
        (a.ExposureModeOption.Mode1, 1),
        (a.ExposureModeOption.Mode2, 2),
    ]:
        for option, option_byte in [
            (a.ExposureOption.Disabled, 0),
            (a.ExposureOption.Enabled, 1),
        ]:
            for duration_value in [0, 1, 0x1234, 0xFFFF]:
                duration = a.ExposureDuration(duration_value)
                assert duration.value == duration_value
                prepared = prepare(
                    a.Command.RequestExposure(
                        mode=mode, duration=duration, option=option
                    ),
                    "adjustment_exposure",
                    [
                        mode_byte,
                        duration_value >> 8,
                        duration_value & 0xFF,
                        option_byte,
                    ],
                )
                assert isinstance(
                    prepared.decode(prepared.raw).value, a.ReplyValue.NoPayload
                )

    writes = [
        (
            a.AdjustmentPacketWrite.Kind0(
                payload=a.AdjustmentPacketPayload8(bytes([1, 2, 3, 4, 5, 6, 7, 8]))
            ),
            0,
            [1, 2, 3, 4, 5, 6, 7, 8],
        ),
        (a.AdjustmentPacketWrite.Kind1(value=0xA1), 1, [0xA1]),
        (
            a.AdjustmentPacketWrite.Kind2(
                payload=a.AdjustmentPacketPayload3(bytes([0xB1, 0xB2, 0xB3]))
            ),
            2,
            [0xB1, 0xB2, 0xB3],
        ),
        (a.AdjustmentPacketWrite.Kind3(value=0xC1), 3, [0xC1]),
        (
            a.AdjustmentPacketWrite.Kind4(
                payload=a.AdjustmentPacketPayload2(bytes([0xD1, 0xD2]))
            ),
            4,
            [0xD1, 0xD2],
        ),
    ]
    for packet, kind, payload in writes:
        expected = [kind] + payload + [0] * (10 - len(payload))
        prepared = prepare(
            a.Command.AdjustmentPacket(
                action=a.AdjustmentPacketAction.Write(packet=packet)
            ),
            "adjustment_packet",
            expected,
        )
        assert isinstance(prepared.decode(prepared.raw).value, a.ReplyValue.NoPayload)

    reads = [
        (
            a.AdjustmentPacketRead.Kind20,
            0x20,
            [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0, 0, 0],
            a.AdjustmentPacketReadData.Kind20,
            [0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17],
        ),
        (
            a.AdjustmentPacketRead.Kind21,
            0x21,
            [0x21, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            a.AdjustmentPacketReadData.Kind21,
            0x21,
        ),
        (
            a.AdjustmentPacketRead.Kind22,
            0x22,
            [0x31, 0x32, 0x33, 0, 0, 0, 0, 0, 0, 0],
            a.AdjustmentPacketReadData.Kind22,
            [0x31, 0x32, 0x33],
        ),
        (
            a.AdjustmentPacketRead.Kind23,
            0x23,
            [0x41, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            a.AdjustmentPacketReadData.Kind23,
            0x41,
        ),
    ]
    for kind_enum, kind, output, cls, expected_data in reads:
        prepared = prepare(
            a.Command.AdjustmentPacket(
                action=a.AdjustmentPacketAction.Read(kind=kind_enum)
            ),
            "adjustment_packet",
            [kind] + [255] * 10,
        )
        value = decode(prepared, "adjustment_packet", [kind] + output)
        assert isinstance(value, a.ReplyValue.AdjustmentPacketRead)
        assert isinstance(value.data, cls)
        actual = value.data.data if hasattr(value.data, "data") else value.data.value
        if isinstance(expected_data, list):
            assert isinstance(actual, (bytes, tuple))
            assert list(actual) == expected_data
        else:
            assert actual == expected_data

    report = prepare(a.Command.ReportServiceSuccess(), "report_service_success", [])
    assert isinstance(report.decode(report.raw).value, a.ReplyValue.NoPayload)

    for wrapper, valid_length in [
        (a.AdjustmentPacketPayload8, 8),
        (a.AdjustmentPacketPayload3, 3),
        (a.AdjustmentPacketPayload2, 2),
    ]:
        wrapper(bytes([0x5A] * valid_length))
        try:
            wrapper(bytes([0x5A] * (valid_length - 1)))
        except a.CommandError as error:
            assert isinstance(error.reason, a.CommandFailure.Argument)
            assert error.reason.kind == a.ArgumentKind.AdjustmentPacketLength
        else:
            raise AssertionError("incorrect packet payload length accepted")

    for command in [
        lambda: a.Command.RequestExposure(
            mode=0,  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
            duration=a.ExposureDuration(1),
            option=a.ExposureOption.Enabled,
        ),
        lambda: a.AdjustmentPacketAction.Write(packet=1),  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
    ]:
        try:
            command()
        except TypeError:
            pass
        else:
            raise AssertionError("untyped exposure or packet command accepted")
    return True
