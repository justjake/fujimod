"""Video panels over immutable camera frames; snapshots support ordinary assertions."""

import json
from collections.abc import Callable
from copy import deepcopy
from typing import TypedDict

from ..recording import Frame, Recorder


class CardRow(TypedDict):
    name: str
    path: str
    depth: int
    directory: bool
    bytes: int


class CardSnapshot(TypedDict):
    label: str
    rows: list[CardRow]
    highlight: str | None


class CardOverlay:
    """A short SD tree. Its label and paths are the data actually sent to the renderer."""

    def __init__(
        self,
        recorder: Recorder,
        snapshot: CardSnapshot,
        event: Callable[[dict[str, object]], None],
    ) -> None:
        self._recorder, self._event = recorder, event
        self._visible = False
        self._snapshot: CardSnapshot = dict(label="", rows=[], highlight=None)
        self.update(snapshot)

    @property
    def visible(self) -> bool:
        return self._visible

    @property
    def label(self) -> str:
        return self._snapshot["label"] or "(untitled)"

    @property
    def paths(self) -> tuple[str, ...]:
        return tuple(row["path"] for row in self._snapshot["rows"])

    @property
    def snapshot(self) -> CardSnapshot:
        return deepcopy(self._snapshot)

    def update(self, snapshot: CardSnapshot) -> None:
        self._recorder.card_panel(json.dumps(snapshot))
        entering = not self._visible
        self._snapshot, self._visible = deepcopy(snapshot), True
        self._event(
            dict(
                kind="sd-card",
                action="show" if entering else "update",
                snapshot=self.snapshot,
            )
        )
        if entering and self._recorder.video and self._recorder.has_poster:
            self._recorder.animate(0.5)

    def format(self, snapshot: CardSnapshot) -> None:
        """Shake the visible old card, then reveal the observed post-format snapshot."""
        if not self.visible:
            raise ValueError("Show the SD card before formatting it")
        self._recorder.card_formatting()
        self._event(dict(kind="sd-card", action="format", snapshot=self.snapshot))
        if self._recorder.video and self._recorder.has_poster:
            self._recorder.animate(0.5)
        self.update(snapshot)

    def hide(self) -> None:
        if self.visible:
            self._recorder.card_panel(None)
            self._visible = False
            self._event(dict(kind="sd-card", action="hide"))
            if self._recorder.video and self._recorder.has_poster:
                self._recorder.animate(0.5)


class DisplayOverlay:
    """The same labeled picture-in-picture style for HDMI, EVF or a reference frame."""

    def __init__(
        self,
        recorder: Recorder,
        frame: Frame,
        *,
        label: str,
        width: float,
        event: Callable[[dict[str, object]], None],
    ) -> None:
        self._recorder, self._event = recorder, event
        self._label, self._width, self._visible = label, width, False
        self.update(frame)

    @property
    def visible(self) -> bool:
        return self._visible

    @property
    def label(self) -> str:
        return self._label

    @property
    def width(self) -> float:
        return self._width

    def update(self, frame: Frame) -> None:
        self._recorder.display_panel(frame, label=self.label, width=self.width)
        entering = not self.visible
        self._visible = True
        if entering:
            self._event(
                dict(
                    kind="display",
                    action="show",
                    label=self.label,
                    width=self.width,
                    source_sha256=frame.sha256,
                )
            )
            if self._recorder.video and self._recorder.has_poster:
                self._recorder.animate(0.5)

    def hide(self) -> None:
        if self.visible:
            self._recorder.display_panel(None)
            self._visible = False
            self._event(dict(kind="display", action="hide", label=self.label))
            if self._recorder.video and self._recorder.has_poster:
                self._recorder.animate(0.5)
