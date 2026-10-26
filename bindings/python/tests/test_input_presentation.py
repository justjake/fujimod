"""Native badge pixels use shared control identities and vector directions."""

import json
import unittest
from io import BytesIO
from pathlib import Path

from fujimod.presentation import SPEC, control
from fujimod.recording import Frame
from PIL import Image, ImageChops


def picture(
    labels: list[tuple[str, int]], width: int = 320, height: int = 240
) -> Image.Image:
    frame = Frame.rgb(width, height, bytes([255]) * width * height * 3)
    return Image.open(BytesIO(frame.annotated(labels).png())).convert("RGB")


class InputPresentationTests(unittest.TestCase):
    def test_native_recorder_embeds_the_shared_catalog_presentation(self) -> None:
        source = Path(__file__).resolve().parents[3] / "catalog/input-presentation.json"
        self.assertEqual(SPEC, json.loads(source.read_bytes()))

    def test_separate_exposure_and_focus_lock_buttons_have_distinct_badges(
        self,
    ) -> None:
        self.assertEqual(control("ae_lock_button")["label"], "AE-L")
        self.assertEqual(control("af_lock_button")["label"], "AF-L")
        self.assertIsNotNone(
            ImageChops.difference(
                picture([("ae_lock_button", 255)]), picture([("af_lock_button", 255)])
            ).getbbox()
        )

    def test_direction_icons_and_accessible_names(self) -> None:
        for direction in ("up", "down", "left", "right"):
            item = control("selector." + direction)
            self.assertEqual(item["label"], direction.title())
            self.assertEqual(item["icon"], direction)
            self.assertNotIn("text", item)
        with self.assertRaisesRegex(ValueError, "Missing input presentation"):
            control("right / flash")
        self.assertIsNotNone(
            ImageChops.difference(
                picture([("selector.right", 255)]), picture([("selector.left", 255)])
            ).getbbox()
        )

    def test_simultaneous_keys_remain_inside_frame(self) -> None:
        output = picture([
            ("menu_ok_button", 255),
            ("command_control.increase", 255),
            ("selector.right", 255),
        ])
        bounds = ImageChops.difference(
            Image.new("RGB", output.size, "white"), output
        ).getbbox()
        assert bounds is not None
        self.assertGreater(bounds[0], 0)
        self.assertLess(bounds[2], output.width)
        self.assertLessEqual(bounds[3], output.height - SPEC["badge"]["bottom"])

    def test_stack_is_vertical_at_bottom_right_with_newest_at_bottom(self) -> None:
        output = picture([("selector.down", 64), ("menu_ok_button", 255)], 720, 480)
        bounds = ImageChops.difference(
            Image.new("RGB", output.size, "white"), output
        ).getbbox()
        height = (
            SPEC["badge"]["icon_size"]
            + 2 * SPEC["badge"]["padding_y"]
            + 2 * SPEC["badge"]["border_width"]
        )
        assert bounds is not None
        self.assertEqual(bounds[3], 480 - SPEC["badge"]["bottom"])
        self.assertEqual(bounds[3] - bounds[1], 2 * height + SPEC["badge"]["gap"])
        self.assertEqual(bounds[2], 708)
        newest = output.crop(
            (bounds[0], 480 - SPEC["badge"]["bottom"] - height, bounds[2], bounds[3])
        ).getextrema()[1][0]
        oldest = output.crop(
            (bounds[0], bounds[1], bounds[2], bounds[1] + height)
        ).getextrema()[1][0]
        self.assertLess(newest, 100)
        self.assertGreater(oldest, newest + 100)
