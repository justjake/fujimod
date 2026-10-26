"""Guide execution, discovery and the generated clean-image/video contract."""

import json
import shutil
import subprocess
import tempfile
import unittest
from collections.abc import Callable
from io import BytesIO
from pathlib import Path

from fujimod.recording import Frame
from fujimod.walkthroughs import Guide, discover
from fujimod.walkthroughs.text import MenuEntry, MenuReference, plain


def png(color: str = "white", size: tuple[int, int] = (320, 240)) -> bytes:
    from PIL import Image

    buffer = BytesIO()
    Image.new("RGB", size, color).save(buffer, format="PNG")
    return buffer.getvalue()


class Camera:
    guide: Guide[Camera]
    value: int = 0
    menu_reference: Callable[[MenuReference], MenuEntry]
    select_menu_item: Callable[[MenuReference], None]
    open_menu_item: Callable[[MenuReference], None]

    def __init__(self) -> None:
        self.frame = Frame.from_png(png())

    def start_recording(self, guide: Guide[Camera]) -> None:
        self.guide = guide
        guide.frame(self.frame, 0)

    def stop_recording(self) -> None:
        pass

    def begin_segment(self) -> None:
        self.guide.frame(self.frame, 0)

    def capture(self) -> Frame:
        return self.frame

    def press(self) -> None:
        index = self.guide.input("selector.right", True, 0)
        self.guide.frame(self.frame, 0.2)
        self.guide.interaction("menu", "mod/item", input_index=index)
        self.guide.input("selector.right", False, 0.2)


class WalkthroughTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def guide(self, *, video: bool = False) -> tuple[Camera, Guide[Camera]]:
        camera = Camera()
        return camera, Guide(
            camera,
            self.root / "run",
            identity={"build": "test"},
            title="Test",
            video=video,
        )

    def test_badge_filter_preserves_every_input_as_coverage_evidence(self) -> None:
        from unittest.mock import Mock, call

        _, guide = self.guide()
        guide.recording = Mock(seconds=0)
        guide.input_badges = frozenset({"control_ring"})
        for name in ("menu_ok_button", "control_ring"):
            guide.input(name, True, 0)
            guide.input(name, False, 0.1)
        self.assertEqual(
            [event["name"] for event in guide.report["inputs"]],
            ["menu_ok_button", "menu_ok_button", "control_ring", "control_ring"],
        )
        self.assertEqual(
            guide.recording.input.call_args_list,
            [call("control_ring", True, 0), call("control_ring", False, 0.1)],
        )

    def test_multiple_functions_and_filename_identity(self) -> None:
        source = self.root / "example.py"
        source.write_text(
            "from fujimod.walkthroughs import walkthrough\n"
            '@walkthrough("One")\ndef first(camera, guide): pass\n'
            '@walkthrough("Two")\ndef second(camera, guide): pass\n'
        )
        self.assertEqual(
            [(w.name, w.title, w.source) for w in discover(source)],
            [("first", "One", source.resolve()), ("second", "Two", source.resolve())],
        )

    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required")
    def test_prerequisite_panels_neither_animate_nor_carry_into_the_child(self) -> None:
        camera, guide = self.guide(video=True)

        def prepare(camera: Camera, guide: Guide[Camera]) -> None:
            card = guide.show_card(dict(label="OLD", rows=[], highlight=None))
            assert card.visible
            card.format(dict(label="", rows=[], highlight=None))
            assert card.label == "(untitled)"
            guide.show_display(camera.frame, label="EVF")

        def run(camera: Camera, guide: Guide[Camera]) -> None:
            with guide.step("Actual camera", hold=0.1):
                assert guide.recording.frames == 0

        report = guide.run(run, prerequisites=[("prepare", prepare)])
        assert report["passed"], report.get("error")
        assert not report.get("overlays")
        raw = subprocess.check_output([
            "ffmpeg",
            "-v",
            "error",
            "-i",
            str(guide.output / "video-clean.mp4"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        assert raw and min(raw) > 245

    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required")
    def test_clean_screenshot_and_video_overlay(self) -> None:
        camera, guide = self.guide(video=True)

        def run(camera: Camera, guide: Guide[Camera]) -> None:
            with guide.step("Example", "Description", hold=0.2):
                camera.press()

        report = guide.run(run)
        self.assertTrue(report["passed"])
        self.assertEqual(
            (guide.output / "screenshots/001.png").read_bytes(), camera.frame.png()
        )
        self.assertEqual(len(report["interactions"]), 1)
        frame = subprocess.check_output([
            "ffmpeg",
            "-v",
            "error",
            "-i",
            str(guide.output / "video.mp4"),
            "-frames:v",
            "1",
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "-",
        ])
        from PIL import Image, ImageStat

        picture = Image.open(BytesIO(frame)).convert("RGB")
        self.assertLess(
            min(ImageStat.Stat(picture.crop((260, 140, 308, 178))).mean), 240
        )
        self.assertGreater(
            min(ImageStat.Stat(picture.crop((0, 0, 100, 100))).mean), 250
        )
        self.assertGreater(
            min(ImageStat.Stat(picture.crop((135, 140, 185, 178))).mean), 250
        )
        self.assertEqual(plain(report["steps"][0]["title"]), "Example")
        self.assertIn("video.mp4", report["artifacts"])

    @unittest.skipUnless(shutil.which("ffmpeg"), "ffmpeg required")
    def test_output_switch_keeps_native_screenshot_and_fixed_video_canvas(self) -> None:
        from PIL import Image

        camera, guide = self.guide(video=True)

        def run(camera: Camera, guide: Guide[Camera]) -> None:
            with guide.step("First output", hold=0.2):
                pass
            with guide.step("Second output", hold=0.2):
                camera.frame = Frame.from_png(png("red", (640, 360)))

        report = guide.run(run)
        screenshot = (guide.output / "screenshots/002.png").read_bytes()
        self.assertEqual(screenshot, camera.frame.png())
        with Image.open(BytesIO(screenshot)) as image:
            self.assertEqual(image.size, (640, 360))
        for name in ("video.mp4", "video-clean.mp4"):
            frame = subprocess.check_output([
                "ffmpeg",
                "-v",
                "error",
                "-i",
                str(guide.output / name),
                "-vf",
                "reverse",
                "-frames:v",
                "1",
                "-f",
                "image2pipe",
                "-vcodec",
                "png",
                "-",
            ])
            with Image.open(BytesIO(frame)) as image:
                self.assertEqual(image.size, (320, 240))
                top = image.getpixel((160, 0))
                middle = image.getpixel((160, 120))
                assert isinstance(top, tuple) and isinstance(middle, tuple)
                self.assertLess(max(top[:3]), 10)
                self.assertGreater(middle[0], 240)
        self.assertTrue(report["passed"])

    def test_failed_assertion_never_produces_passing_report_or_step(self) -> None:
        camera, guide = self.guide()

        def run(camera: Camera, guide: Guide[Camera]) -> None:
            with guide.step("Will fail"):
                raise AssertionError("wrong setting")

        with self.assertRaisesRegex(AssertionError, "wrong setting"):
            guide.run(run)
        report = json.loads((guide.output / "guide.json").read_text())
        self.assertFalse(report["passed"])
        self.assertEqual(report["steps"], [])
        self.assertFalse((guide.output / "screenshots").exists())

    def test_fixture_suppresses_coverage(self) -> None:
        camera, guide = self.guide()

        def run(camera: Camera, guide: Guide[Camera]) -> None:
            with guide.fixture("Prepare settings"):
                camera.press()
            with guide.step("Example", hold=0):
                pass

        report = guide.run(run)
        self.assertEqual(report["interactions"], [])
        self.assertTrue(report["fixtures"][0]["passed"])

    def test_nested_steps_refused(self) -> None:
        camera, guide = self.guide()
        with (
            guide.step("Outer"),
            self.assertRaisesRegex(ValueError, "nest"),
            guide.step("Inner"),
        ):
            pass
        guide.recording.finish()

    def test_invalid_hold(self) -> None:
        camera, guide = self.guide()
        for hold in (-1, float("nan"), float("inf"), True, "2"):
            with self.assertRaises(ValueError), guide.step("Invalid", hold=hold):  # ty: ignore[invalid-argument-type]  # Deliberately invalid input: verify runtime rejection.
                pass

    def test_timestamp_cadence_preserves_short_samples(self) -> None:
        camera, guide = self.guide()
        guide.input("selector.right", True, 0)
        for n in range(101):
            guide.frame(camera.frame, n / 100)
        self.assertEqual(guide.recording.frames, 15)

    def test_nonmonotonic_video_time_refused(self) -> None:
        camera, guide = self.guide()
        guide.frame(camera.frame, 1)
        with self.assertRaises(ValueError):
            guide.frame(camera.frame, 0.5)

    def test_relative_helpers_and_duplicate_function_alias(self) -> None:
        (self.root / "helpers.py").write_text('TITLE="Helper title"\n')
        source = self.root / "example.py"
        source.write_text(
            "from .helpers import TITLE\nfrom fujimod.walkthroughs import walkthrough\n"
            "@walkthrough(TITLE)\ndef first(camera,guide): pass\nalias=first\n"
        )
        found = discover(source)
        self.assertEqual(
            [(w.name, w.title) for w in found], [("first", "Helper title")]
        )

    def test_discovery_does_not_reuse_same_size_timestamp_bytecode(self) -> None:
        import os

        source = self.root / "example.py"
        source.write_text(
            'from fujimod.walkthroughs import walkthrough\n@walkthrough("One")\ndef first(c,g): pass\n'
        )
        self.assertEqual(discover(source)[0].title, "One")
        stamp = source.stat()
        source.write_text(source.read_text().replace('"One"', '"Two"'))
        os.utime(source, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        self.assertEqual(discover(source)[0].title, "Two")

    def test_zero_hold_adds_no_frame(self) -> None:
        camera, guide = self.guide()
        guide.recording.still(camera.frame, 0)
        self.assertEqual(guide.recording.frames, 0)

    def test_input_release_and_fixture_input_cannot_earn_coverage(self) -> None:
        camera, guide = self.guide()
        guide._recording = True
        release = guide.input("selector.right", False, 0)
        with self.assertRaises(ValueError):
            guide.interaction("menu", "m/item", input_index=release)
        with guide.fixture("Setup"):
            setup = guide.input("selector.right", True, 0)
        with self.assertRaises(ValueError):
            guide.interaction("menu", "m/item", input_index=setup)

    def test_touch_start_anchors_coverage_and_records_its_position(self) -> None:
        camera, guide = self.guide()
        guide._recording = True
        start = guide.touch("start", 120, 80, 0)
        moved = guide.touch("move", 130, 82, 0.1)
        lifted = guide.touch("end", 130, 82, 0.2)
        assert start is not None and moved is not None and lifted is not None
        guide.interaction("menu", "m/item", input_index=start)
        for index in (moved, lifted):
            with self.assertRaises(ValueError):
                guide.interaction("menu", "m/item", input_index=index)
        self.assertEqual(
            [
                (e["kind"], e.get("phase"), e.get("x"), e.get("y"), e["pressed"])
                for e in guide.report["inputs"]
            ],
            [
                ("touch", "start", 120, 80, True),
                ("touch", "move", 130, 82, None),
                ("touch", "end", 130, 82, False),
            ],
        )
        with self.assertRaisesRegex(ValueError, "No finger is down"):
            guide.touch("move", 1, 1, 0.3)
        with self.assertRaisesRegex(ValueError, "Unknown touch phase"):
            guide.touch("hover", 1, 1, 0.3)  # ty: ignore[invalid-argument-type]

    def test_catching_a_failed_step_does_not_make_run_pass(self) -> None:
        camera, guide = self.guide()

        def run(c: Camera, g: Guide[Camera]) -> None:
            try:
                with g.step("Broken"):
                    raise AssertionError("broken")
            except AssertionError:
                pass
            with g.step("Later", hold=0):
                pass

        with self.assertRaisesRegex(AssertionError, "broken"):
            guide.run(run)
        self.assertFalse(
            json.loads((guide.output / "guide.json").read_text())["passed"]
        )

    def test_helper_rewrite_with_unchanged_timestamp_is_not_cached(self) -> None:
        import os

        helper = self.root / "helpers.py"
        helper.write_text('TITLE="One"\n')
        source = self.root / "example.py"
        source.write_text(
            "from .helpers import TITLE\nfrom fujimod.walkthroughs import walkthrough\n"
            "@walkthrough(TITLE)\ndef first(camera,guide): pass\n"
        )
        self.assertEqual(discover(source)[0].title, "One")
        stamp = helper.stat()
        helper.write_text('TITLE="Two"\n')
        os.utime(helper, ns=(stamp.st_atime_ns, stamp.st_mtime_ns))
        self.assertEqual(discover(source)[0].title, "Two")

    def test_directory_discovery_preserves_cross_file_parent_and_factory_binding(
        self,
    ) -> None:
        (self.root / "navigation.py").write_text(
            "from fujimod.walkthroughs import open_menu\n"
            'page = open_menu("item", title="Open page")\n'
        )
        (self.root / "features.py").write_text(
            "from .navigation import page\n"
            "from fujimod.walkthroughs import walkthrough\n"
            '@walkthrough("Child", start_from=page)\ndef child(c,g): pass\n'
        )
        found = {w.name: w for w in discover(self.root)}
        self.assertEqual(set(found), {"page", "child"})
        self.assertIs(found["child"].start_from, found["page"].function)
        self.assertEqual(found["page"].source.name, "navigation.py")

    def test_menu_instruction_captures_highlight_before_activation_and_branch_starts_after(
        self,
    ) -> None:
        source = self.root / "navigation.py"
        source.write_text(
            "from fujimod.walkthroughs import open_menu\n"
            'page = open_menu("item", title="Item settings")\n'
        )
        visit = discover(source)[0].function

        def make_camera() -> Camera:
            camera = Camera()
            camera.menu_reference = lambda item: MenuEntry(
                "item", "ITEM", ancestors=(MenuEntry("root", "MENU"),)
            )
            camera.select_menu_item = lambda item: setattr(
                camera, "frame", Frame.from_png(png("blue"))
            )

            def activate(item: MenuReference) -> None:
                self.assertEqual(camera.frame.png(), png("blue"))
                camera.press()
                camera.frame = Frame.from_png(png("red"))
                camera.guide.frame(camera.frame, 0.2)

            camera.open_menu_item = activate
            return camera

        parent = Guide(make_camera(), self.root / "parent", identity={}, title="Parent")
        report = parent.run(visit)
        self.assertEqual(
            [plain(s["title"]) for s in report["steps"]], ["Choose MENU > ITEM"]
        )
        self.assertEqual(
            (parent.output / report["steps"][0]["screenshot"]).read_bytes(), png("blue")
        )
        self.assertEqual(parent.camera.frame.png(), png("red"))
        self.assertGreater(
            report["presentation"]["duration_seconds"], report["steps"][0]["video_end"]
        )
        child = Guide(make_camera(), self.root / "child", identity={}, title="Child")

        def continuation(camera: Camera, guide: Guide[Camera]) -> None:
            with guide.step("Item settings"):
                self.assertEqual(camera.frame.png(), png("red"))

        child.run(continuation, prerequisites=[("mod/navigation/page", visit)])
        self.assertEqual((child.output / "poster.png").read_bytes(), png("red"))

    def test_branches_replay_parent_without_inheriting_steps_or_coverage(self) -> None:
        for number in range(2):
            camera = Camera()
            camera.value = 0
            guide = Guide(camera, self.root / str(number), identity={}, title="Child")

            def parent(c: Camera, g: Guide[Camera]) -> None:
                self.assertEqual(c.value, 0)
                with g.step("Parent"):
                    c.press()
                    c.value = 1
                    c.frame = Frame.from_png(png("blue"))

            def child(c: Camera, g: Guide[Camera]) -> None:
                self.assertEqual(c.value, 1)
                with g.step("Child", hold=0):
                    c.press()
                    c.value = 2
                    c.frame = Frame.from_png(png("red"))

            report = guide.run(child, prerequisites=[("mod/file/parent", parent)])
            self.assertEqual([plain(s["title"]) for s in report["steps"]], ["Child"])
            self.assertEqual(len(report["interactions"]), 1)
            self.assertEqual(len(report["inputs"]), 2)
            self.assertEqual(report["inputs"][0]["seconds"], 0)
            self.assertTrue(report["prerequisites"][0]["passed"])
            self.assertEqual((guide.output / "poster.png").read_bytes(), png("blue"))
            self.assertEqual(
                (guide.output / "screenshots/001.png").read_bytes(), png("red")
            )

    def test_failed_prerequisite_does_not_enter_child(self) -> None:
        from unittest.mock import Mock

        camera, guide = self.guide()
        child = Mock()

        def parent(c: Camera, g: Guide[Camera]) -> None:
            with g.step("Parent"):
                raise AssertionError("parent failed")

        with self.assertRaisesRegex(AssertionError, "parent failed"):
            guide.run(child, prerequisites=[("parent", parent)])
        child.assert_not_called()
        self.assertFalse(guide.report["prerequisites"][0]["passed"])
        self.assertFalse(guide.report["passed"])

    def test_idle_compression_preserves_changed_frames_and_guest_timestamps(
        self,
    ) -> None:
        camera, guide = self.guide()
        recording = guide.recording
        for image, time in [
            (camera.frame.png(), 0),
            (camera.frame.png(), 3),
            (png("blue"), 3.1),
            (png("blue"), 4.1),
        ]:
            recording.frame(Frame.from_png(image), time)
        self.assertAlmostEqual(recording.skipped_seconds, 3.3)
        self.assertEqual(recording.frames, 12)
        self.assertEqual(json.loads(recording.metrics())["changed_frames"], 2)

    def test_explicit_holds_are_not_compressed(self) -> None:
        camera, guide = self.guide()
        guide.recording.still(camera.frame, 2)
        self.assertEqual(guide.recording.frames, 30)

    def test_action_holds_only_present_the_result_and_replay_has_no_hold(self) -> None:
        from unittest.mock import Mock

        camera, guide = self.guide()
        camera.capture = Mock(return_value=camera.frame)
        with guide.action(navigation=True):
            pass
        self.assertEqual(guide.recording.frames, 1)
        with guide.action():
            pass
        self.assertEqual(guide.recording.frames, 8)
        guide._preparing = True
        with guide.action():
            pass
        self.assertEqual(guide.recording.frames, 8)
        self.assertEqual(camera.capture.call_count, 2)

    def test_failed_action_never_adds_a_presentation_hold(self) -> None:
        camera, guide = self.guide()
        with self.assertRaises(AssertionError), guide.action():
            raise AssertionError("input failed")
        self.assertEqual(guide.recording.frames, 0)
