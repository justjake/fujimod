"""The shared native source protocol and screenshot-default recording API."""

import hashlib
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

from fujimod.recording import Frame, Recorder, capture_scanout
from PIL import Image, ImageChops


class Source:
    def __init__(self) -> None:
        self.calls = 0
        self.instructions, self.ticks = 123, 456

    def recording_frame(
        self, node: str, instance: int, diagnostics: bool
    ) -> tuple[bytes, str, int, int]:
        assert node == "scanout" and instance == 0
        self.calls += 1
        return (
            bytes([40]) * 320 * 240 * 3,
            json.dumps(dict(instance=instance)),
            320,
            240,
        )


class RecordingTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required")
    def test_sd_overlay_motion_formatting_and_original_resolution(self) -> None:
        from fujimod.walkthroughs.overlays import CardOverlay, CardSnapshot

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            recorder = Recorder(root, fps=10, video=True)
            frame = Frame.rgb(720, 480, bytes((40, 80, 120)) * 720 * 480)
            recorder.frame(frame, 0)
            recorder.still(frame, 0.2)
            snapshot: CardSnapshot = dict(
                label="OLD CARD",
                rows=[
                    dict(name="DCIM", path="/DCIM", depth=0, directory=True, bytes=0),
                    dict(
                        name="DSCF0001.JPG",
                        path="/DCIM/DSCF0001.JPG",
                        depth=1,
                        directory=False,
                        bytes=123456,
                    ),
                ],
                highlight="/DCIM/DSCF0001.JPG",
            )
            events: list[dict[str, object]] = []
            card = CardOverlay(recorder, snapshot, events.append)
            assert card.visible and card.paths == ("/DCIM", "/DCIM/DSCF0001.JPG")
            snapshot["label"] = "CALLER MUTATION"
            assert card.label == "OLD CARD"
            recorder.still(frame, 0.2)
            card.format(dict(label="", rows=[], highlight=None))
            assert card.label == "(untitled)" and not card.paths
            recorder.still(frame, 0.2)
            card.hide()
            assert not card.visible
            with self.assertRaisesRegex(ValueError, "Show the SD card"):
                card.format(dict(label="NEW", rows=[], highlight=None))
            recorder.still(frame, 0.2)
            recorder.screenshot(frame, "native.png")
            recorder.finish()
            self.assertEqual(
                Frame.from_png((root / "native.png").read_bytes()).pixels(),
                frame.pixels(),
            )
            for movie in ("video.mp4", "video-clean.mp4"):
                raw = subprocess.check_output([
                    "ffmpeg",
                    "-v",
                    "error",
                    "-i",
                    str(root / movie),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "rgb24",
                    "pipe:1",
                ])
                images = [
                    Image.frombytes("RGB", (720, 480), raw[i : i + 720 * 480 * 3])
                    for i in range(0, len(raw), 720 * 480 * 3)
                ]
                assert len(images) == 23
                # The camera stays full-size; only its lower-right corner is covered.
                for image in images:
                    for box in ((0, 0, 720, 180), (0, 0, 390, 480)):
                        assert (
                            max(
                                ImageChops.difference(
                                    image.crop(box), images[0].crop(box)
                                ).tobytes()
                            )
                            <= 3
                        )
                tops = []
                for image in images[2:7]:
                    changed = (
                        ImageChops
                        .difference(image, images[0])
                        .convert("L")
                        .point(lambda v: 255 if v > 12 else 0)
                    )
                    bounds = changed.getbbox()
                    tops.append(bounds[1] if bounds else 480)
                assert tops[0] >= tops[1] > tops[-1]
                # Native entry, rapid shake, new empty card, then a clean exit.
                assert images[9].tobytes() != images[10].tobytes()
                assert (
                    images[8].crop((420, 320, 700, 460)).tobytes()
                    != images[15].crop((420, 320, 700, 460)).tobytes()
                )
                assert max(ImageChops.difference(images[-1], images[0]).tobytes()) <= 3

    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required")
    def test_display_overlay_contains_its_source_and_rounds_its_corners(self) -> None:
        from fujimod.walkthroughs.overlays import DisplayOverlay

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            recorder = Recorder(root, fps=10, video=True)
            primary = Frame.rgb(720, 480, bytes((20, 40, 120)) * 720 * 480)
            secondary = Frame.rgb(720, 480, bytes((210, 30, 20)) * 720 * 480)
            recorder.frame(primary, 0)
            events: list[dict[str, object]] = []
            display = DisplayOverlay(
                recorder, secondary, label="HDMI", width=0.6, event=events.append
            )
            assert display.visible and display.label == "HDMI" and display.width == 0.6
            recorder.still(primary, 0.2)
            display.hide()
            recorder.finish()
            raw = subprocess.check_output([
                "ffmpeg",
                "-v",
                "error",
                "-i",
                str(root / "video-clean.mp4"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ])
            images = [
                Image.frombytes("RGB", (720, 480), raw[i : i + 720 * 480 * 3])
                for i in range(0, len(raw), 720 * 480 * 3)
            ]
            shown = images[6]
            assert shown.tobytes()[(150 * 720 + 500) * 3] > 180
            assert (
                shown.tobytes()[(14 * 720 + 274) * 3 + 2] > 90
            )  # Rounded outer corner reveals the primary.
            box = (0, 0, 250, 480)
            assert (
                max(
                    ImageChops.difference(
                        shown.crop(box), images[0].crop(box)
                    ).tobytes()
                )
                <= 3
            )

            assert primary.pixels() == bytes((20, 40, 120)) * 720 * 480

    def test_pixels_preserve_the_unencoded_rgb_raster(self) -> None:
        pixels = bytes((10, 20, 30, 40, 50, 60))
        frame = Frame.rgb(2, 1, pixels)
        self.assertEqual(frame.pixels(), pixels)
        self.assertEqual(Frame.from_png(frame.png()).pixels(), pixels)

    def test_idle_limit_can_preserve_a_required_observation_interval(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            recorder = Recorder(Path(directory), fps=10)
            frame = Frame.rgb(2, 1, bytes((10, 20, 30)) * 2)
            recorder.frame(frame, 0)
            recorder.idle_limit = 2.9
            recorder.frame(frame, 2.5)
            self.assertEqual(recorder.frames, 25)
            self.assertEqual(recorder.skipped_seconds, 0)
            for value in (-1, float("nan"), float("inf")):
                with self.assertRaises(ValueError):
                    recorder.idle_limit = value
            self.assertEqual(recorder.idle_limit, 2.9)
            recorder.finish()

    def test_screenshot_default_samples_just_the_start_and_never_requires_encoders(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            recorder, source = Recorder(root), Source()
            recorder.sample_scanout(source, 0)
            for index in range(100):
                recorder.sample_scanout(source, index / 15, force=True)
            frame = capture_scanout(source)
            assert frame is not None
            digest = recorder.screenshot(frame, "screenshots/001.png")
            recorder.still(frame, 1.5)
            recorder.finish()
            self.assertEqual(source.calls, 2)
            self.assertEqual((source.instructions, source.ticks), (123, 456))
            self.assertEqual(
                digest,
                hashlib.sha256((root / "screenshots/001.png").read_bytes()).hexdigest(),
            )
            self.assertEqual(frame.sha256, digest)
            self.assertEqual(json.loads(recorder.metrics())["written_frames"], 0)
            self.assertFalse((root / "video.mp4").exists())
            self.assertFalse((root / "ffmpeg.log").exists())

    def test_badges_do_not_change_clean_frames_or_native_resolution(self) -> None:
        frame = Frame.rgb(320, 240, bytes([40]) * 320 * 240 * 3)
        before = frame.png()
        annotated = frame.annotated([("selector.right", 255)])
        self.assertEqual(frame.png(), before)
        self.assertNotEqual(annotated.png(), before)
        self.assertEqual((annotated.width, annotated.height), (320, 240))
        with self.assertRaisesRegex(ValueError, "Missing input presentation"):
            frame.annotated([("unconfigured control", 255)])
