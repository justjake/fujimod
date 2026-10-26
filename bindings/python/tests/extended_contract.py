"""Shared native/Pyodide boundary checks for signed and extended service fields."""


def check_extended_values() -> bool:
    import json

    from fujimod import catalog
    from fujimod.usb import app80 as a

    rows = [
        (96, "set_lens_position", [31, 17, 4]),
        (97, "request_lens_position", [4]),
        (98, "read_lens_parameter", [31, 17]),
        (229, "device9_exchange", list(reversed(range(33)))),
    ]
    rows += [(code, "pipeline_request_wait", []) for code in [165, 167, 169]]
    rows += [(166, "pipeline_request_value", [7]), (72, "read_scan", list(range(33)))]
    doc = {
        "schema": 1,
        "firmware": {
            "id": "extended",
            "model": "Synthetic",
            "version": "1",
            "sha256": "ab" * 32,
        },
        "mailbox": {"page": 3, "offset": 128, "configuration_limit": 512},
        "entries": [
            dict(
                code=code,
                codec=codec,
                label=codec,
                slots=slots,
                enabled_modes=4,
                next_mode=0,
                firmware_kind=1,
                handler=0,
                parameter_table=0,
            )
            for code, codec, slots in rows
        ],
    }
    service = a.Service(
        catalog.service.Definition.from_json(
            json.dumps(doc), catalog.FirmwareIdentity("extended", "ab" * 32)
        )
    )
    modes = catalog.service.ServiceModes(4)
    command = a.Command.Pipeline(action=a.PipelineAction.RunRequestAndWait())
    try:
        service.prepare(modes, command)
    except a.CommandError:
        pass
    else:
        raise AssertionError("ambiguous command was selected arbitrarily")
    for code in [165, 167, 169]:
        p = service.prepare_code(a.CommandCode(code), modes, command)
        assert p.raw[0] == code
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    try:
        service.prepare_code(a.CommandCode(166), modes, command)
    except a.CommandError:
        pass
    else:
        raise AssertionError("command codec mismatch accepted")
    for value in [1, 255]:
        p = service.prepare(
            modes,
            a.Command.Pipeline(
                action=a.PipelineAction.RequestValue(value=a.PipelineValue(value))
            ),
        )
        assert p.raw[8] == value
    try:
        a.PipelineValue(0)
    except ValueError:
        pass
    else:
        raise AssertionError("zero pipeline value accepted")
    for index, count in [(0, 1), (63, 64), (254, 255)]:
        selected = a.ScanSelection(index, count)
        p = service.prepare(modes, a.Command.ReadScan(selection=selected))
        assert len(p.raw) == 34 and p.raw[1] == index
        for length in [0, 1, 31, 32]:
            raw = bytearray(p.raw)
            raw[2 : 2 + length] = bytes(range(128, 128 + length))
            value = p.decode(raw).value
            assert isinstance(value, a.ReplyValue.ScanName)
            assert value.data == bytes(range(128, 128 + length)) + bytes(32 - length)
    for index, count in [(0, 0), (1, 1), (255, 255)]:
        try:
            a.ScanSelection(index, count)
        except ValueError:
            pass
        else:
            raise AssertionError("out of observed scan range accepted")
    for position in [-32768, -1, 0, 1, 32767]:
        p = service.prepare(modes, a.Command.SetLensPosition(position=position))
        high, low = (position & 65535).to_bytes(2, "big")
        assert (p.raw[32], p.raw[18], p.raw[5]) == (high, low, 255)
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
        p = service.prepare_code(
            a.CommandCode(98), modes, a.Command.ReadLensParameter()
        )
        raw = bytearray(p.raw)
        raw[32] = high
        raw[18] = low
        value = p.decode(raw).value
        assert (
            isinstance(value, a.ReplyValue.LensParameter)
            and value.raw_value == position
        )
    for invalid in [-32769, 32768, 1.5, float("nan"), float("inf"), "1", None]:
        try:
            a.Command.SetLensPosition(position=invalid)  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
        except TypeError, OverflowError, ValueError:
            pass
        else:
            raise AssertionError(f"accepted invalid signed field {invalid!r}")
    for count in [0, 1, 28, 29]:
        p = service.prepare(
            modes,
            a.Command.Device9(
                action=a.Device9Action.Read(
                    first=18, second=52, count=a.Device9Count(count)
                )
            ),
        )
        assert len(p.raw) == 34
        raw = bytearray(p.raw)
        for i in range(count):
            raw[1 + 32 - (4 + i)] = i
        value = p.decode(raw).value
        assert isinstance(value, a.ReplyValue.Device9Data) and value.data == bytes(
            range(count)
        )
        try:
            p.decode(raw[:33])
        except Exception:
            pass
        else:
            raise AssertionError("truncated extended result accepted")
        payload = a.Device9Payload(bytes(range(count)))
        p = service.prepare(
            modes,
            a.Command.Device9(
                action=a.Device9Action.Write(first=1, second=2, payload=payload)
            ),
        )
        assert [p.raw[1 + 32 - (4 + i)] for i in range(count)] == list(range(count))
        assert isinstance(p.decode(p.raw).value, a.ReplyValue.NoPayload)
    for make in [lambda: a.Device9Count(30), lambda: a.Device9Payload(bytes(30))]:
        try:
            make()
        except ValueError:
            pass
        else:
            raise AssertionError("30 byte device9 payload accepted")
    return True
