#!/usr/bin/env python3
"""Pack the three seeded plant shapes and their compact meshes by category."""

import argparse
from pathlib import Path

from build_plant_assets import DESTINATION, Plant
from plant_geometry import ground_level, write_glb
from plant_species import SPECIES


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=DESTINATION / "runtime")
    args = parser.parse_args()
    for category in dict.fromkeys(species.category for species in SPECIES):
        models = []
        for recipe in SPECIES:
            if recipe.category != category:
                continue
            for variant in range(3):
                mesh = Plant(recipe, seed=23, variant=variant).build()
                ground = ground_level(mesh)
                name = f"{recipe.name}_{variant}"
                models.extend(((f"{name}_detail", mesh, ground), (f"{name}_compact", mesh.compact(), ground)))
        size = write_glb(args.out / f"{category}.glb", models)
        print(f"{category}: {len(models)} meshes, {size:,} bytes")


if __name__ == "__main__":
    main()
