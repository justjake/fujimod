"""Resolved board execution through the shared native session."""

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .board import (
        Board,
        BootReport,
        ExecutionPolicy,
        ScanoutFrame,
        Session,
        make_board,
    )
    from .definition import Asset, Definition, Image, Xp4Definition

__all__ = [
    "Asset",
    "Board",
    "BootReport",
    "Definition",
    "ExecutionPolicy",
    "Image",
    "ScanoutFrame",
    "Session",
    "Xp4Definition",
    "make_board",
]


def __getattr__(name: str) -> object:
    if name in (
        "Board",
        "BootReport",
        "ExecutionPolicy",
        "ScanoutFrame",
        "Session",
        "make_board",
    ):
        from .board import (
            Board,
            BootReport,
            ExecutionPolicy,
            ScanoutFrame,
            Session,
            make_board,
        )

        return {
            "Board": Board,
            "BootReport": BootReport,
            "ExecutionPolicy": ExecutionPolicy,
            "ScanoutFrame": ScanoutFrame,
            "Session": Session,
            "make_board": make_board,
        }[name]
    if name in ("Asset", "Definition", "Image", "Xp4Definition"):
        from .definition import Asset, Definition, Image, Xp4Definition

        return {
            "Asset": Asset,
            "Definition": Definition,
            "Image": Image,
            "Xp4Definition": Xp4Definition,
        }[name]
    raise AttributeError(name)
