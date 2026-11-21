#!/usr/bin/env python3
"""Measure baked cube-sphere inland water footprints and centerline bed clearance."""

import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image

RADIUS = 100_000.0
CORRECTION = 0.87**2
TEXEL_OFFSET = 0.5 / (508 * 16)
HEIGHT_DIR = Path("terrains/spherical/source/height")
HEADER = b"WLWATER1"


def cube_uv(vector):
    dominant = np.abs(vector).argmax(axis=1)
    divisor = vector[np.arange(len(vector)), dominant]
    x, y, z = (vector / np.abs(divisor[:, None])).T
    face = np.choose(dominant, [np.where(divisor < 0, a, b)
                                for a, b in [(0, 3), (5, 2), (4, 1)]])
    axes = np.stack([
        np.choose(face, [z, x, x, -y, -y, z]),
        np.choose(face, [-y, -y, z, z, x, x]),
    ], axis=1)
    uv = 0.5 + axes * (0.5 * np.sqrt((1 + CORRECTION) / (1 + CORRECTION * axes**2)))
    return face, np.clip(uv, 0, 1)


def runtime_positions(vector, size):
    face, uv = cube_uv(vector)
    plane = np.clip((uv * (size - 1) + 0.5) / size + TEXEL_OFFSET, 0, 1) * 2 - 1
    u, v = (plane / np.sqrt(1 + CORRECTION - CORRECTION * plane**2)).T
    one = np.ones_like(u)
    direction = np.stack([
        np.choose(face, [-one, u, u, one, v, v]),
        np.choose(face, [-v, -v, one, -u, -u, -one]),
        np.choose(face, [u, one, v, v, -one, u]),
    ], axis=1)
    return (direction * (np.linalg.norm(vector, axis=1) / np.linalg.norm(direction, axis=1))[:, None]).astype(np.float32).astype(np.float64)


def weighted_pixels(side):
    u = np.linspace(-1, 1, side)
    v = u[:, None]
    sx, sy = 1 + CORRECTION - CORRECTION * u**2, 1 + CORRECTION - CORRECTION * v**2
    scale = (2 * RADIUS / (side - 1) / 1000)**2
    area = scale * (1 + CORRECTION)**2 / (sx * sy * (1 + u**2 / sx + v**2 / sy))**1.5
    area[[0, -1], :] *= 0.5
    area[:, [0, -1]] *= 0.5
    return area


def read_faces(path, suffix):
    return np.stack([np.asarray(Image.open(path / f"face{face}.{suffix}")) for face in range(6)])


def baked_mesh(path):
    with path.open("rb") as mesh:
        return mesh.read(len(HEADER)) == HEADER


def read_vertices(path, count):
    offset = len(HEADER) if baked_mesh(path) else 0
    if path.stat().st_size == offset:
        return np.empty((0, count, 3), dtype=np.float32)
    return np.memmap(path, mode="r", dtype="<f4", offset=offset).reshape(-1, count, 3)


def cell(vector, size, baked=False):
    face, uv = cube_uv(vector)
    grid = (uv - TEXEL_OFFSET) * size - 0.5 if baked else uv * (size - 1)
    x, y = np.clip(grid.round(), 0, size - 1).astype(int).T
    return face, y, x


def terrain_depth(vector, heights):
    size = heights.shape[-1]
    face, uv = cube_uv(vector)
    x, y = np.clip((uv - TEXEL_OFFSET) * size - 0.5, 0, size - 1).T
    x0, y0 = x.astype(int), y.astype(int)
    x1, y1 = np.minimum(x0 + 1, size - 1), np.minimum(y0 + 1, size - 1)
    dx, dy = x - x0, y - y0
    a = (1 - dx) * heights[face, y0, x0] + dx * heights[face, y0, x1]
    b = (1 - dx) * heights[face, y1, x0] + dx * heights[face, y1, x1]
    return np.linalg.norm(vector, axis=1) - RADIUS - ((1 - dy) * a + dy * b)


def measure(assets, lake_vertices):
    world = assets / "world"
    heights = read_faces(assets / HEIGHT_DIR, "tif").astype(float) * (21_000 / 65_535) - 12_000
    size = heights.shape[-1]
    ocean = read_faces(world / "ocean", "png") > 0
    land_area = float(np.sum((~ocean) * weighted_pixels(size)))
    alpha = np.asarray(Image.open(world / "river_atlas.png"))[:, :, 3]
    side = alpha.shape[0] // 2
    weights = weighted_pixels(side)
    source = (np.arange(side) * (size - 1) / (side - 1)).round().astype(int)
    area = 0.0
    for face in range(6):
        uy, ux = divmod(face, 3)
        cover = alpha[uy * side:(uy + 1) * side, ux * side:(ux + 1) * side] >= 156
        area += float(weights[cover & ~ocean[face][source[:, None], source]].sum())

    tiles = read_vertices(world / "lake_surface.bin", lake_vertices)
    if lake_vertices == 30:
        # Legacy lake records are ribbons with their source center stored in the first fan triangle.
        center = (tiles[:, 6].astype(float) + (tiles[:, 1].astype(float) + tiles[:, 5]) / 2) / 2
        lake = np.zeros_like(ocean)
        lake[cell(center, size)] = True
    else:
        lake = read_faces(world / "basins", "png") > 0
    lake &= ~ocean
    lake_area = float(np.sum(lake * weighted_pixels(size)))

    river_path = world / "river_surface.bin"
    rivers = read_vertices(river_path, 30)
    baked = baked_mesh(river_path)
    start = rivers[:, 6].astype(float)
    end = (rivers[:, 1].astype(float) + rivers[:, 5]) / 2
    start_norm = np.linalg.norm(start, axis=1)
    end_norm = np.linalg.norm(end, axis=1)
    nonzero = end_norm >= 0.5 * RADIUS
    index = np.flatnonzero(nonzero)
    f = start[index] / start_norm[index, None]
    t = end[index] / end_norm[index, None]
    arc = RADIUS * np.arctan2(np.linalg.norm(np.cross(f, t), axis=1), np.sum(f * t, axis=1))
    # A valid source-grid edge is far shorter than 1 km on this 1024² grid.
    valid = arc < 1000
    index, arc = index[valid], arc[valid]
    a, b = cell(start[index], size, baked), cell(end[index], size, baked)
    heads = ~np.isin(np.ravel_multi_index(a, ocean.shape), np.ravel_multi_index(b, ocean.shape))
    dry = ~lake[a] & ~lake[b] & ~ocean[a] & ~ocean[b]
    if baked:
        p, q = start[index][dry], end[index][dry]
    else:
        p = runtime_positions(start[index][dry], size)
        q = (runtime_positions(rivers[index, 1][dry], size) + runtime_positions(rivers[index, 5][dry], size)) / 2
    gaps = np.concatenate([terrain_depth(p + (q - p) * fraction, heights)
                           for fraction in np.linspace(0.05, 0.95, 19)])
    widths = np.linalg.norm(rivers[index, 0] - rivers[index, 2], axis=1)
    return {
        "inland_area_km2": area,
        "inland_water_percent_of_non_ocean": area / land_area * 100,
        "lake_area_km2": lake_area,
        "river_reaches": len(rivers),
        "invalid_grid_links": len(rivers) - len(index),
        "river_length_km": float(arc.sum() / 1000),
        "river_heads_excluding_lake_outlets": int(np.count_nonzero(heads & ~lake[a])),
        "ocean_mouths": int(np.count_nonzero(ocean[b])),
        "river_width_m_p50_p90": (np.quantile(widths, [0.5, 0.9]).tolist() if len(widths) else []),
        "highest_river_source_m": (float(start_norm[index].max() - RADIUS) if len(index) else None),
        "dry_centerline_samples": len(gaps),
        "dry_centerline_gaps_m_min_p50_p99_max": (np.quantile(gaps, [0, 0.5, 0.99, 1]).tolist() if len(gaps) else []),
        "dry_centerline_samples_below_bed_by_0_5m": int(np.count_nonzero(gaps < -0.5)),
        "dry_centerline_samples_above_bed_by_8m": int(np.count_nonzero(gaps > 8)),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("assets", type=Path)
    parser.add_argument("--lake-record-vertices", type=int, choices=(6, 30), default=6)
    args = parser.parse_args()
    print(json.dumps(measure(args.assets, args.lake_record_vertices), indent=2))


if __name__ == "__main__":
    main()
