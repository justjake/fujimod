"""Presentation pauses belong at maneuver boundaries, outside input timing."""

from collections.abc import Generator
from contextlib import contextmanager
from pathlib import Path
from string.templatelib import Template
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from typing import cast
from unittest import TestCase
from unittest.mock import Mock

from fujimod.recording import Frame, Recorder
from fujimod.walkthroughs import Guide, MenuCamera, MenuGuide, open_menu
from fujimod.walkthroughs.text import MenuEntry, encode, plain


class ManeuverPacingTests(TestCase):
    def test_illustrated_destination_holds_for_one_and_a_half_seconds(self) -> None:
        with TemporaryDirectory() as directory:
            guide = Guide.__new__(Guide)
            guide.output = Path(directory)
            image = Frame.rgb(2, 2, bytes(12))
            guide.camera = SimpleNamespace(capture=Mock(return_value=image))
            video = Mock(spec=Recorder)
            video.seconds = 0
            guide.recording = video
            guide.report = dict(
                schema=3,
                identity={},
                title="Test",
                passed=False,
                media="screenshots",
                steps=[],
                inputs=[],
                interactions=[],
                fixtures=[],
                prerequisites=[],
            )
            guide._inside_step = guide._preparing = False
            guide._failures = []
            guide.save = Mock()
            with guide.step("Choose the tile"):
                pass
            video.still.assert_called_once_with(image, 1.5)
            self.assertEqual(guide.report["steps"][0]["hold"], 1.5)

    def test_menu_starter_pauses_after_activation_before_following_navigation(
        self,
    ) -> None:
        events = []
        entry = MenuEntry("item", "ITEM", ancestors=(MenuEntry("root", "MENU"),))

        @contextmanager
        def step(title: str | Template) -> Generator[None]:
            tokens = encode(title, resolve=lambda item: entry, output=Path("."))
            events.append(("instruction", plain(tokens)))
            yield
            events.append(("screenshot", "highlighted item"))

        camera = SimpleNamespace(
            select_menu_item=lambda item: events.append(("select", item.menu)),
            open_menu_item=lambda item: events.append(("open", item.menu)),
        )
        guide = SimpleNamespace(
            step=step, pause=lambda seconds: events.append(("hold", seconds))
        )
        open_menu("item", title="Item settings")(
            cast(MenuCamera, camera), cast(MenuGuide, guide)
        )
        self.assertEqual(
            events,
            [
                ("instruction", "Choose MENU > ITEM"),
                ("select", "item"),
                ("screenshot", "highlighted item"),
                ("open", "item"),
                ("hold", 1.5),
            ],
        )
