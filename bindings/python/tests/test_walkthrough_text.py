"""T-strings retain semantic identity through JSON and HTML rendering."""

import hashlib
from pathlib import Path
from string.templatelib import Template
from tempfile import TemporaryDirectory
from unittest import TestCase

from fujimod.walkthroughs import ref
from fujimod.walkthroughs.text import MenuEntry, Token, encode, icons, plain, render

PNG = b"\x89PNG\r\n\x1a\nfixture"


class SemanticTextTests(TestCase):
    def setUp(self) -> None:
        directory = TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        root = MenuEntry("stock/setup", "SET-UP")
        parent = MenuEntry("stock/button", "BUTTON/DIAL SETTING", PNG, (root,))
        self.entries: dict[str | int, MenuEntry] = dict(
            root=root,
            parent=parent,
            child=MenuEntry("stock/quick", "EDIT/SAVE QUICK MENU", PNG, (root, parent)),
            mod=MenuEntry("mod/aspect", "ASPECT RATIO", PNG, (root,)),
        )

    def encode(self, text: str | Template) -> list[Token]:
        return encode(
            text, resolve=lambda item: self.entries[item.menu], output=self.output
        )

    def test_standard_template_retains_reference_and_exports_resolved_identity(
        self,
    ) -> None:
        item = ref(menu="child")
        template = t"Choose {item}"
        self.assertIs(template.values[0], item)
        tokens = self.encode(template)
        self.assertEqual(plain(tokens), "Choose EDIT/SAVE QUICK MENU")
        assert tokens[1]["type"] == "menu"
        self.assertEqual(tokens[1]["identity"], "stock/quick")
        icon = tokens[1]["icon"]
        assert icon is not None
        self.assertEqual((self.output / icon["asset"]).read_bytes(), PNG)
        self.assertEqual(icon["sha256"], hashlib.sha256(PNG).hexdigest())

    def test_name_and_icon_views_preserve_identity(self) -> None:
        item = ref(menu="mod")
        name = self.encode(t"{item.name}")[0]
        icon = self.encode(t"{item.icon}")[0]
        assert name["type"] == "menu" and icon["type"] == "menu"
        self.assertEqual(name["identity"], icon["identity"])
        self.assertIsNone(name["icon"])
        self.assertEqual(name["display"], "name")
        self.assertEqual(icon["display"], "icon")
        self.assertNotIn(">ASPECT RATIO<", render([icon]))
        self.assertIn('alt="ASPECT RATIO"', render([icon]))

    def test_absolute_and_relative_paths_retain_ordered_menu_entries(self) -> None:
        child, parent = ref(menu="child"), ref(menu="parent")
        absolute = self.encode(t"{child.path()}")
        relative = self.encode(t"{child.path(relative_to=parent)}")
        self.assertEqual(
            plain(absolute), "SET-UP > BUTTON/DIAL SETTING > EDIT/SAVE QUICK MENU"
        )
        self.assertEqual(plain(relative), "EDIT/SAVE QUICK MENU")
        assert absolute[0]["type"] == "menu_path"
        self.assertEqual(
            [part["identity"] for part in absolute[0]["items"]],
            ["stock/setup", "stock/button", "stock/quick"],
        )

    def test_relative_paths_refuse_non_ancestors_and_the_item_itself(self) -> None:
        child = ref(menu="child")
        for ancestor in (child, ref(menu="mod")):
            with self.assertRaisesRegex(ValueError, "ancestor"):
                self.encode(t"{child.path(relative_to=ancestor)}")

    def test_unknown_reference_is_an_error(self) -> None:
        unknown = ref(menu="missing")
        with self.assertRaises(KeyError):
            self.encode(t"Choose {unknown}")

    def test_f_strings_cannot_silently_discard_menu_identity(self) -> None:
        item = ref(menu="child")
        with self.assertRaisesRegex(TypeError, "t-string"):
            f"Choose {item}"
        with self.assertRaisesRegex(ValueError, "properties"):
            self.encode(t"Choose {item!s}")

    def test_normal_python_formatting_remains_available_for_other_values(self) -> None:
        value = 1.234
        self.assertEqual(plain(self.encode(t"Value: {value:.2f}")), "Value: 1.23")

    def test_composing_templates_keeps_nested_references_semantic(self) -> None:
        item = ref(menu="child")
        instruction = t"Choose {item}"
        tokens = self.encode(t"Next: {instruction}.")
        self.assertEqual(plain(tokens), "Next: Choose EDIT/SAVE QUICK MENU.")
        assert tokens[2]["type"] == "menu"
        self.assertEqual(tokens[2]["identity"], "stock/quick")

    def test_render_escapes_text_and_names_and_uses_bundled_icon_paths(self) -> None:
        self.entries["unsafe"] = MenuEntry('stock/"<entry>', "<setting & name>", PNG)
        item = ref(menu="unsafe")
        tokens = self.encode(t"<script>Choose {item}")
        markup = render(tokens, prefix="guide/")
        self.assertNotIn("<script>", markup)
        self.assertIn("&lt;setting &amp; name&gt;", markup)
        self.assertIn('src="guide/icons/', markup)
        self.assertEqual(len(list(icons(tokens))), 1)

    def test_iconless_entries_render_their_names(self) -> None:
        item = ref(menu="root")
        self.assertEqual(plain(self.encode(t"{item}")), "SET-UP")
        self.assertNotIn("<img", render(self.encode(t"{item.icon}")))

    def test_icon_paths_cannot_escape_the_report(self) -> None:
        item = ref(menu="child")
        tokens = self.encode(t"{item}")
        assert tokens[0]["type"] == "menu"
        assert tokens[0]["icon"] is not None
        tokens[0]["icon"]["asset"] = "../external.png"
        with self.assertRaisesRegex(ValueError, "icon asset"):
            render(tokens)
