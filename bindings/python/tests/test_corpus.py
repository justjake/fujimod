"""Corpus binding checks: indexing, names, annotations and cache round trips."""

import struct
import unittest
from pathlib import Path

from fujimod.corpus import Index


class CorpusBinding(unittest.TestCase):
    def test_queries_roundtrip_merge_and_errors(self) -> None:
        indexes = []
        body = struct.pack(
            "<5I", 0xE3A00001, 0xE2800002, 0xE2800003, 0xE2800004, 0xE12FFF1E
        )
        for fw in ("a", "b"):
            i = Index()
            i.add_unit(
                dict(
                    firmware=fw,
                    unit="bank",
                    architecture="arm32",
                    address_ranges=[],
                    functions=[
                        dict(
                            address=0x1000,
                            ranges=[[0x1000, 0x1014]],
                            names=["synthetic"] if fw == "a" else [],
                        )
                    ],
                ),
                [(0x1000, body)],
            )
            i.build()
            indexes.append(i)
        indexes[0].merge(indexes[1])
        i = indexes[0]
        f = i.find("synthetic", firmware="b")[0]
        self.assertEqual(i.names(f["id"], ["a"])["synthetic"][0]["firmware"], "a")
        donor = i.find("synthetic", firmware="a")[0]
        candidates = i.candidates(donor["id"], "b")
        self.assertEqual(len(candidates), 1)
        self.assertEqual(candidates[0]["function"]["key"], f["key"])
        self.assertEqual(candidates[0]["grade"], "identical")
        self.assertIn("family", candidates[0]["origins"])
        i.add_name(donor["id"], "synthetic", "invented-header")
        self.assertEqual(
            i.names(f["id"], ["a"])["synthetic"][0]["sources"], ["invented-header"]
        )
        self.assertEqual(i.pairs("a", "b")[0]["grade"], "identical")
        self.assertEqual(len(i.members(f["family"])), 2)
        self.assertEqual(i.calls(f["id"]), [])
        self.assertEqual(i.data_references(f["id"]), [])
        self.assertEqual(i.references_to("b", 0x1000), [])
        self.assertEqual(i.search("b", body[:4], unit="bank")[0]["address"], 0x1000)
        with self.assertRaises(ValueError):
            i.search("b", b"")
        with self.assertRaises(ValueError):
            i.search("b", body, mask=b"x")
        i.add_symbol(
            dict(
                firmware="b",
                unit="bank",
                address=0x1000,
                length=4,
                name="synthetic_data",
                kind="fixture",
                source="test",
            )
        )
        self.assertEqual(i.symbols("synthetic_data")[0]["address"], 0x1000)
        self.assertEqual(i.read("b", "bank", 0x1000, 4), body[:4])
        self.assertIsNone(i.read("b", "bank", 0x9999, 4))
        self.assertTrue(
            i.name_support(f["id"], i.find("synthetic", firmware="a")[0]["id"])[
                "corroborated"
            ]
        )
        i.annotate(f["family"], "note", "synthetic", "test")
        output = (
            Path(__file__).resolve().parents[3] / "target/corpus-tests/python.sqlite"
        )
        output.parent.mkdir(parents=True, exist_ok=True)
        try:
            i.save(output)
            restored = Index.open(output)
            family = restored.family(f["family"])
            assert family is not None
            self.assertEqual(family["annotations"][0]["text"], "synthetic")
            self.assertEqual(restored.symbols("synthetic_data")[0]["firmware"], "b")
            self.assertEqual(
                restored.names(f["id"], ["a"])["synthetic"][0]["sources"],
                ["invented-header"],
            )
            self.assertEqual(restored.find(firmware="b"), i.find(firmware="b"))
            self.assertEqual(restored.read("b", "bank", 0x1000, 4), body[:4])
            self.assertEqual(restored.subset(["a"]).stats()["firmwares"], {"a": 1})
            self.assertEqual(restored.pairs("a", "b"), i.pairs("a", "b"))
            restored.add_name(f["id"], "restored", "test")
            self.assertEqual(
                restored.find("restored", firmware="b")[0]["names"], ["restored"]
            )
        finally:
            output.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
