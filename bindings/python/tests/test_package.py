"""Python conversion boundaries; no USB discovery or device I/O."""

import importlib

import pytest
from fujimod import catalog, emulator, usb
from fujimod._native import emulator as native_emulator


def test_one_package_supports_direct_submodule_imports() -> None:
    assert importlib.import_module("fujimod.catalog") is catalog
    assert importlib.import_module("fujimod.catalog.service") is catalog.service
    assert importlib.import_module("fujimod.emulator") is emulator
    assert importlib.import_module("fujimod._native.emulator") is native_emulator
    assert catalog.FirmwareIdentity("synthetic-v1", "ab" * 32).id == "synthetic-v1"
    assert catalog.service.ServiceModes(4).mask == 4
    assert isinstance(usb.app80.Command.FlushConfiguration(), usb.app80.Command)
    assert importlib.import_module("fujimod.usb") is usb
    assert importlib.import_module("fujimod.usb") is usb
    assert usb.Session.__module__ == "fujimod.usb"
    assert importlib.import_module("fujimod.usb.memory") is usb.memory
    assert importlib.import_module("fujimod.usb.memory") is usb.memory
    assert importlib.import_module("fujimod.usb.data") is usb.data
    assert usb.Session.__module__ == "fujimod.usb"
    assert not hasattr(usb.Device, "interfaces")
    assert callable(usb.interfaces)
    assert not hasattr(usb, "blocking")
    with pytest.raises(ModuleNotFoundError):
        importlib.import_module("fujimod.usb.blocking")
    assert not hasattr(usb.Session, "read_range")
    assert callable(usb.memory.read_range)


def test_selectors_are_typed_enums() -> None:
    assert all(usb.InfoSelector.Product != value for value in range(8))
    assert usb.ConfigurationView.Ram != 1
    assert usb.KeyState.On != 0
    assert type(usb.DataReadMode.Echo) is not type(usb.DataWriteMode.Echo)
    assert usb.ConfigurationView.Ram != "ram"
    assert usb.KeyState.On != "ON"
    with pytest.raises(TypeError):
        usb.InfoSelector("Product")


def test_firmware_limits_are_explicit_and_immutable() -> None:
    limits = usb.FirmwareLimits(
        memory_read=256, memory_write=128, configuration=64, console=80
    )
    assert (
        limits.memory_read,
        limits.memory_write,
        limits.configuration,
        limits.console,
    ) == (256, 128, 64, 80)
    with pytest.raises(AttributeError):
        limits.memory_read = 512  # ty: ignore[invalid-assignment]  # Deliberately invalid input: verify runtime rejection.
    with pytest.raises(TypeError):
        usb.FirmwareLimits()  # ty: ignore[missing-argument]  # Deliberately invalid input: verify runtime rejection.


@pytest.mark.parametrize("length", [0, -1, 2**32])
def test_invalid_limits_rejected_before_device_io(length: int) -> None:
    with pytest.raises((ValueError, OverflowError)):
        usb.FirmwareLimits(
            memory_read=length, memory_write=256, configuration=256, console=256
        )


def test_usb_descriptor_quantities_and_research_tags_remain_exact() -> None:
    selected = usb.InterfaceSelection(configuration=2, interface=3, alternate=4)
    assert (selected.configuration, selected.interface, selected.alternate) == (2, 3, 4)
    with pytest.raises(OverflowError):
        usb.InterfaceSelection(configuration=256, interface=0, alternate=0)
    assert usb.DiagnosticTag.bulk_source().value == 0x80
    assert usb.DiagnosticTag.research(0xDEADBEEF).value == 0xDEADBEEF


def test_error_categories_support_structured_handling() -> None:
    assert usb.UsbError.__module__ == "fujimod.usb"
    assert issubclass(usb.TransferError, usb.UsbError)
    assert issubclass(usb.ProtocolError, usb.UsbError)
    assert issubclass(usb.DeviceCommandError, usb.UsbError)
    assert issubclass(usb.CompletionUnknownError, usb.UsbError)
