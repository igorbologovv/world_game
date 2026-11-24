# Procedural plant assets

The catalog ships 40 base GLBs in `assets/models/vegetation/procedural_plants/`,
with eight species each in `grasses`, `flowers`, `shrubs`, `ferns`, and `coastal`.
The exporter uses Python 3.10 or later and the standard library.

```sh
# List the available species and their growth habits.
python3 -B tools/build_plant_assets.py --list-species

# Rebuild the 40 base assets in their repository directory.
python3 -B tools/build_plant_assets.py

# Rebuild the five runtime packs used by the planetary ground-cover streamer.
python3 -B tools/build_plant_library.py

# Export 2,560 seeded variants plus the 40 bases to another directory.
python3 -B tools/build_plant_assets.py --variants 64 --seed 23 --out /tmp/world-plants

# Swap parts on one plant and export 12 variants of the resulting recipe.
python3 -B tools/build_plant_assets.py --out /tmp/world-plants --species sea_lavender \
  --parts-from stem=red_glasswort --parts-from leaf=meadow_cranesbill \
  --parts-from branch=oxeye_daisy --parts-from flower=scarlet_poppy --variants 12

python3 -B -m unittest discover -s tools -p test_build_plant_assets.py
```

Base files use `<category>/<species>.glb`. Numbered variants use
`<category>/<species>/seed_<seed>/variant_0001.glb` and higher numbers. A seed
reproduces the same numbered variants independently of export order and species
selection. It leaves base files byte-identical. Part swaps go under
`hybrids/<part>_<donor>[--<part>_<donor>...]/` within the export directory.

Recipes in `plant_species.py` define the growth habit and four reusable parts.
Stems set thickness, curve, and color. Leaves set the outline, proportions, fold,
and palette. Branches set the number and spread of shoots and leaves. Flowers
define the bloom or seed-head shape, color, and size. Branch donors share the
recipient's growth habit, while a flower donor without blooms removes flowering.
Seeded variants change proportions, palette, available leaf shapes, branching
counts and angles, and flower or seed-head size and density.

Each GLB has a single indexed mesh and a single double-sided material. It uses
16-bit indices, packed vertex colors, and geometry for stems, blades, and buds.
Meshes use meters, Y-up, and a ground-level pivot. The base assets span
220–1,980 triangles and total 1.04 MiB. The generator caps exports at 2,400
triangles by simplifying dense foliage and limiting repeated flower clusters
to fit. Each file exposes `Scene0` through Bevy's glTF loader.

The game loads the five category packs in `procedural_plants/runtime/`.
Each pack contains the base and two seeded variants per species, with paired
`_detail` and `_compact` meshes at the same pivot. The compact LOD keeps
representative leaves, stems, and flowering parts from the full mesh within
128 triangles. The existing ground-cover streamer merges these meshes into its
patch batches. [Vegetation streaming and placement](../crates/world-vegetation/README.md#procedural-ground-plants)
