"""Synthetic browser integration controller; never enumerates local USB."""

import asyncio
import json
import os
import sys

from fujimod import usb


def limits() -> usb.FirmwareLimits:
    return usb.FirmwareLimits(
        memory_read=256, memory_write=256, configuration=256, console=256
    )


async def check_discovery() -> None:
    devices = await usb.discover(usb.DeviceFilter.Diagnostic)
    assert len(devices) == 1
    choices = await usb.interfaces(devices[0])
    trace = usb.tracing.Trace(128, 8192)
    session = await usb.Session.open(devices[0], choices[0], limits(), trace=trace)
    assert session.token == 0x1234
    assert await session.device_info(usb.InfoSelector.Product) == "SYNTHETIC"
    assert await session.read_data(123, 5) == bytes([123, 124, 125, 126, 127])
    plan = usb.CommandSequence()
    plan.write(0x9000, b"old!")
    plan.read(0x9000, 4, b"old!")
    query = plan.query(77, [(0, 0), (1, 0xFFFF)])
    plan.write(0x9000, b"new!")
    result = await session.execute_sequence(plan)
    assert result[1] == b"old!" and result[query][4:12] == bytes.fromhex(
        "01000000ffff0000"
    )
    assert await session.read_memory(0x9000, 4) == b"new!"
    await session.write_memory(0x2000, b"\0\xff\x80")
    assert await session.read_memory(0x2000, 3) == b"\0\xff\x80"
    assert await session.close() == b"\xaa\xbb"
    assert any(event.endpoint == 0x82 for event in trace.drain().events)


async def check_async(url: str, token: str) -> bool:
    devices = await usb.discover_proxy(url, token, usb.DeviceFilter.Diagnostic)
    assert len(devices) == 1
    choices = await usb.interfaces(devices[0])
    session = await usb.Session.open(devices[0], choices[0], limits())
    assert await session.device_info(usb.InfoSelector.Product) == "SYNTHETIC"
    assert await session.read_data(123, 5) == bytes([123, 124, 125, 126, 127])
    plan = usb.CommandSequence()
    plan.write(0x9000, b"old!")
    plan.read(0x9000, 4, b"old!")
    plan.query(77, [(0, 0), (1, 0xFFFF)])
    plan.write(0x9000, b"new!")
    assert (await session.execute_sequence(plan))[1] == b"old!"
    assert await session.read_memory(0x9000, 4) == b"new!"
    assert await session.close() == b"\xaa\xbb"
    return True


async def check_timeout() -> None:
    device = (await usb.discover(usb.DeviceFilter.Diagnostic))[0]
    session = await usb.Session.open(
        device, (await usb.interfaces(device))[0], limits(), timeout_ms=100
    )
    try:
        await session.ping()
    except usb.TransferError:
        pass
    else:
        raise AssertionError("hung transfer succeeded")
    assert session.state == usb.SessionState.CompletionUnknown
    try:
        await session.close()
    except usb.CompletionUnknownError:
        pass
    else:
        raise AssertionError("uncertain session sent camera cleanup")
    session.abandon()


if __name__ == "__main__":
    if sys.argv[1] == "discovery":
        asyncio.run(check_discovery())
    elif sys.argv[1] == "timeout":
        asyncio.run(check_timeout())
    else:
        asyncio.run(
            check_async(
                os.environ["FUJIMOD_USB_PROXY_URL"],
                os.environ["FUJIMOD_USB_PROXY_TOKEN"],
            )
        )
    print(json.dumps({"passed": True, "controller": sys.argv[1]}))
