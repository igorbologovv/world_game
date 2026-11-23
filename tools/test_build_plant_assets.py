"""Regression checks for the offline plant catalog and its GLB asset contract."""

import json
import math
import struct
import subprocess
import sys
import tempfile
import unittest
from collections import Counter
from pathlib import Path

from build_plant_assets import CATALOG, DESTINATION
from plant_geometry import MAX_TRIANGLES, Vertex


SCRIPT = Path(__file__).with_name("build_plant_assets.py")


class PlantExportTests(unittest.TestCase):
    def generate(self, out, *args):
        subprocess.run([sys.executable, "-B", str(SCRIPT), "--out", str(out), *args],
                       check=True, capture_output=True, text=True)

    def read_glb(self, data):
        magic, version, size, json_size, kind = struct.unpack_from("<4sIII4s", data)
        self.assertEqual((magic, version, size, kind), (b"glTF", 2, len(data), b"JSON"))
        self.assertEqual(json_size % 4, 0)
        end = 20 + json_size
        document = json.loads(data[20:end])
        bin_size, kind = struct.unpack_from("<I4s", data, end)
        payload = data[end + 8:]
        self.assertEqual((bin_size, kind), (len(payload), b"BIN\0"))
        return document, payload

    def test_shipped_catalog_matches_the_generator(self):
        self.assertEqual(Counter(recipe.category for recipe in CATALOG.values()),
                         dict.fromkeys(("grasses", "flowers", "shrubs", "ferns", "coastal"), 8))
        with tempfile.TemporaryDirectory() as folder:
            self.generate(folder)
            files = sorted(Path(folder).glob("*/*.glb"))
            self.assertEqual(len(files), len(CATALOG))
            for file in files:
                with self.subTest(species=file.stem):
                    data = file.read_bytes()
                    self.assertEqual(data, (DESTINATION / file.relative_to(folder)).read_bytes())
                    document, payload = self.read_glb(data)
                    self.assertEqual(len(document["scenes"]), 1)
                    self.assertEqual(len(document["materials"]), 1)
                    self.assertEqual(len(document["meshes"][0]["primitives"]), 1)
                    self.assertNotIn("images", document)
                    attributes, _, colors, indices = document["accessors"]
                    self.assertEqual((colors["componentType"], colors["normalized"]), (5121, True))
                    self.assertEqual(indices["componentType"], 5123)
                    vertex_end = document["bufferViews"][1]["byteOffset"]
                    vertices = list(Vertex.iter_unpack(payload[:vertex_end]))
                    self.assertEqual(attributes["count"], len(vertices))
                    self.assertEqual(min(vertex[1] for vertex in vertices), 0.0)
                    self.assertTrue(all(math.isclose(math.hypot(*v[3:6]), 1, abs_tol=1e-6) for v in vertices))
                    triangle_count, remainder = divmod(indices["count"], 3)
                    self.assertEqual(remainder, 0)
                    self.assertLessEqual(triangle_count, MAX_TRIANGLES)
                    faces = list(struct.iter_unpack("<HHH", payload[vertex_end:vertex_end + 6 * triangle_count]))
                    self.assertLess(max(max(face) for face in faces), len(vertices))

    def test_seeded_variants_and_swappable_parts(self):
        recipe = ("--species", "sea_lavender", "--variants", "12", "--seed", "57")
        with tempfile.TemporaryDirectory() as folder:
            first = Path(folder) / "first"
            second = Path(folder) / "second"
            self.generate(first, *recipe)
            self.generate(second, *recipe)
            variants = sorted(first.glob("coastal/**/variant_*.glb"))
            self.assertEqual(len(variants), 12)
            payloads = set()
            for file in variants:
                data = file.read_bytes()
                self.assertEqual(data, (second / file.relative_to(first)).read_bytes())
                payloads.add(self.read_glb(data)[1])
            self.assertEqual(len(payloads), len(variants))

            self.generate(first, "--species", "sea_lavender", "--parts-from", "stem=red_glasswort",
                          "--parts-from", "leaf=meadow_cranesbill", "--parts-from", "branch=oxeye_daisy",
                          "--parts-from", "flower=scarlet_poppy")
            hybrids = list(first.glob("hybrids/*/coastal/sea_lavender.glb"))
            self.assertEqual(len(hybrids), 1)
            original = self.read_glb((first / "coastal/sea_lavender.glb").read_bytes())[1]
            hybrid = self.read_glb(hybrids[0].read_bytes())[1]
            self.assertNotEqual(original, hybrid)
