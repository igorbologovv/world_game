# Woodland source-scene integration

This branch treats the imported scene as **source assets**, not a second
planet surface. The client no longer spawns the 47 MB starter diorama: no
cutout island, black underside, pond geometry or rectangular ground tile.

## Extract reusable trees

The **original** `low_poly_tree_scene_free/scene.gltf` has separate Bark,
Leaves, Grass, Ground and Water objects. The flattened
`assets/models/starter/low_poly_tree_scene/scene.glb` does *not* preserve
individual tree positions as usable standalone prefabs.

The export script uses Blender's existing glTF importer and material handling.
It finds distinct Bark and Leaves object pairs, uses their common terrain
footprint, moves them to a local root and exports three reduced mesh levels.
It deliberately rejects ground, water and soil materials.

```sh
cd ~/world-client
git fetch origin
git switch feature/woodland-world-integration

# Inspect the source structure and candidate mesh names first:
blender -b --python tools/extract_woodland_prefabs.py -- --list > /tmp/woodland-objects.json

# Export two reusable tree prefabs, with LOD0/1/2:
blender -b --python tools/extract_woodland_prefabs.py -- --auto-trees 2

# Inspect the actual pond material separately; it is NOT a river shader:
blender -b --python tools/extract_woodland_prefabs.py -- --water-report

cargo fmt --all
cargo check -p world-vegetation --all-targets
cargo test -p world-client
cargo run -p world-client
```

The exporter writes:

```text
assets/models/vegetation/woodland/
  woodland_tree_00/tree_lod0.glb
  woodland_tree_00/tree_lod1.glb
  woodland_tree_00/tree_lod2.glb
  woodland_tree_01/tree_lod0.glb
  woodland_tree_01/tree_lod1.glb
  woodland_tree_01/tree_lod2.glb
```

The forest uses the LOD0 exports for oak variants 1 and 2 at canonical
terrain-aligned sites. Igor's hybrid renderer builds the medium and far mesh
proxies from those same textured assets. Hero Oak remains variant zero.
Missing or invalid prefabs use the procedural fallback.

## About the imported pond

The imported source has a mesh named `Water` using material `material_3`:
a tinted semitransparent PBR surface (base alpha 0.6, roughness 0), **not**
an animated water shader. It has no useful flow map or animated normals.
Copying it wholesale to the planetary ocean and river meshes would not
produce physically realistic water and could break shorelines.

Keep existing `world-water` intact until a separate shader implementation
adds texture-driven normals, flow, depth absorption and scene reflections.
The `--water-report` command prints the actual imported shader resources.

## Validation boundaries

Changes to the water/terrain hydrology branch are intentionally not merged
here. The original imported scene is still checked in as a source asset,
but no longer spawned by the client. These changes have not been tested with
a native GPU in the authoring environment; validate the exported meshes in
Blender and the game before merging.

Source asset attribution: Nicholas-3D, CC BY 4.0 (see
`assets/models/starter/low_poly_tree_scene/LICENSE.txt`).
