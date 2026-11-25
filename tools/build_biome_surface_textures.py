"""Bake the CC0 biome surface library. See crates/world-terrain/README.md."""

import argparse
import io
import time
import urllib.request
import zipfile
from concurrent.futures import ThreadPoolExecutor
from functools import lru_cache
from pathlib import Path

import numpy as np

from build_surface_textures import read_rgb, rgba, write_ktx2


# Two consecutive layers per family. Match the order in terrain_materials.wesl.
# A tuple specifies a second source to composite, followed by the surface treatment.
FAMILIES = (
    ("forest soil", "forest_ground_06", ("dirt_floor", None, "humus")),
    ("dry leaves", "forest_leaves_04", "dry_decay_leaves"),
    ("wet leaves", ("brown_mud_leaves_01", None, "wet"), ("mud_forest", None, "wet")),
    ("needles", "forrest_ground_03", "forrest_sand_01"),
    ("green moss", "Moss001", "Moss002"),
    ("dry moss", ("Moss001", "forrest_ground_01", "dry_moss"), ("Moss002", "withered_grass", "dry_moss")),
    ("grass-covered soil", "sparse_grass", "leafy_grass"),
    ("roots and debris", "roots", "wood_chip_path"),
    ("peat", ("forest_ground_05", "mud_forest", "peat"), ("brown_mud", "brown_mud_02", "peat")),
    ("wet mud", ("brown_mud_03", None, "wet"), ("Ground094B", None, "wet")),
    ("cracked mud", "mud_cracked_dry_03", "mud_cracked_dry_riverbed_002"),
    ("gravel", "rocky_gravel", "gravel_stones"),
    ("sand", "sand_01", "dense_sand"),
    ("clay", "cracked_red_ground", "red_mud_stones"),
    ("rock", "dark_rock", "dark_rock_02"),
    ("alpine ground", "rocky_trail_02", ("rocky_trail", "withered_grass", "alpine")),
    ("coastal sediment", "damp_beach_sand_02", "coral_mud_01"),
)

MAPS = (("diff", "Color"), ("nor_gl", "NormalGL"), ("rough", "Roughness"), ("disp", "Displacement"))


def variant(spec):
    return (spec, None, None) if isinstance(spec, str) else spec


def request(url):
    headers = {"User-Agent": "WorldClient-TerrainTextureBaker/1.0"}
    for attempt in range(3):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as response:
                return response.read()
        except OSError:
            if attempt == 2:
                raise
            time.sleep(attempt + 1)


def fetch(source, cache):
    directory = cache / source
    paths = [directory / f"{channel}.jpg" for channel, _ in MAPS]
    if all(path.is_file() for path in paths):
        return
    directory.mkdir(parents=True, exist_ok=True)
    if source[0].isupper():
        url = f"https://ambientcg.com/get?file={source}_1K-JPG.zip"
        with zipfile.ZipFile(io.BytesIO(request(url))) as archive:
            for path, (_, channel) in zip(paths, MAPS):
                member = next(name for name in archive.namelist() if name.endswith(f"_{channel}.jpg"))
                path.write_bytes(archive.read(member))
    else:
        for path, (channel, _) in zip(paths, MAPS):
            url = f"https://dl.polyhaven.org/file/ph-assets/Textures/jpg/1k/{source}/{source}_{channel}_1k.jpg"
            try:
                path.write_bytes(request(url))
            except OSError as error:
                raise RuntimeError(f"Could not download {source} {channel}") from error
    print(f"Downloaded {source}", flush=True)


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def build(cache, terrain, side):
    @lru_cache(maxsize=None)
    def read(source):
        rgb, normal, roughness, height = (
            read_rgb(cache / source / f"{channel}.jpg", side, side, color=index == 0)
            for index, (channel, _) in enumerate(MAPS)
        )
        # Put source heights on the same blend range, preserving local relief.
        low, high = np.percentile(height[:, :, 0], (4, 96))
        height = np.clip((height[:, :, :1] - low) / max(high - low, 0.02), 0, 1)
        return rgb, normal, roughness[:, :, :1], height

    theta = (np.arange(side, dtype=np.float32) + 0.5) * (2 * np.pi / side)
    x, y = theta[None, :, None], theta[:, None, None]
    colors, finishes = [], []
    for _, *specs in FAMILIES:
        for spec in specs:
            primary, secondary, treatment = variant(spec)
            rgb, normal, roughness, height = read(primary)
            if secondary:
                phase = 0.4 if treatment == "peat" else 2.1
                pool = 0.23 * np.sin(3*x + y + phase) + 0.17 * np.cos(x - 4*y)
                pool += 0.11 * np.sin(7*x + 5*y + phase)
                weight = smoothstep(0.23, 0.73, 0.43 + pool - 0.35 * (height - 0.5))
                rgb, normal, roughness, height = (
                    a * (1 - weight) + b * weight
                    for a, b in zip((rgb, normal, roughness, height), read(secondary))
                )
            if treatment == "humus":
                rgb = rgb * np.array([0.51, 0.47, 0.40], dtype=np.float32)
            elif treatment == "wet":
                rgb = rgb * np.array([0.80, 0.81, 0.82], dtype=np.float32)
                roughness = np.clip(roughness * 0.78 - 0.11 * (1 - height), 0.32, 0.86)
            elif treatment == "peat":
                rgb = rgb * np.array([0.43, 0.40, 0.36], dtype=np.float32)
                roughness = np.clip(roughness * 0.92, 0.56, 0.91)
            elif treatment == "dry_moss":
                lightness = rgb @ np.array([0.2126, 0.7152, 0.0722], dtype=np.float32)
                rgb = rgb * 0.35 + lightness[:, :, None] * np.array([0.78, 0.62, 0.34], dtype=np.float32)
                roughness = np.maximum(roughness, 0.78)
            elif treatment == "alpine" or primary == "rocky_trail_02":
                rgb = rgb * np.array([0.78, 0.89, 1.04], dtype=np.float32)
            colors.append(rgba(rgb, height))
            finishes.append(rgba(normal, roughness))

    write_ktx2(terrain / "biome-albedo-height.ktx2", colors, terrain / "albedo.ktx2", srgb=True)
    write_ktx2(terrain / "biome-normal-roughness.ktx2", finishes, terrain / "normal-roughness.ktx2", normal=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cache", type=Path, help="directory for the original JPEG maps")
    parser.add_argument("--fetch", action="store_true", help="download missing sources before baking")
    parser.add_argument("--asset-root", type=Path, default=Path(__file__).resolve().parents[1] / "assets")
    args = parser.parse_args()
    if args.fetch:
        sources = sorted({
            name for _, *specs in FAMILIES for spec in specs for name in variant(spec)[:2] if name
        })
        with ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(lambda source: fetch(source, args.cache), sources))
    build(args.cache, args.asset_root / "textures/terrain", 512)
