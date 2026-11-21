# Woodland Fern — procedural 3D game asset

A single mature woodland fern (inspired by the feathered shape of *Dryopteris* ferns). This is **actual 3D geometry**, not a generated picture: individual curved stems, pinnae and finely divided pinnules on a rosette of 14 arching fronds. It is a procedural interpretation, not a botanical scan.

## Contents

| File | Description | Triangles |
| --- | --- | ---: |
| `woodland_fern_lod0.glb` | Close view, true pinnule geometry | 33,600 |
| `woodland_fern_lod1.glb` | Mid-distance simplified fronds | 5,544 |
| `woodland_fern_lod2.glb` | Far-distance simplified fronds | 1,280 |
| `woodland_fern_lod3.glb` | Two crossed textured alpha-masked billboards | 4 |
| `impostor.png` | Transparent foliage image embedded in LOD3 | — |
| `preview.png` / `showcase.png` | Software-generated previews | — |
| `build_woodland_fern.py` | Reproducible Python mesh generator | — |
| `create_blend.py` | Optional Blender import-and-save helper | — |

**Game coordinates:** right-handed, **Y-up** in GLB / Bevy; meters. Base is at the origin, with the stem crown about 1 cm above ground. LOD0 bounding box is roughly 1.70 m across X × 1.56 m across Z × 0.72 m tall. LOD0–2 retain stable outlines and origin. LOD3 uses an alpha-masked crossed billboard approximation.

**Materials:** RGB vertex colors for LOD0–2 (PBR metallic 0 / roughness 0.85, double-sided), and alpha-mask texture in LOD3 (PBR double-sided). No external texture dependencies for the GLBs. For Blender, **Material Preview** is needed to see colored foliage. The packages contain no animated wind rigs.

## Use directly in the game

The 4 `.glb` files are already exported. Copy them into your Bevy `assets/` folder, e.g. `assets/models/vegetation/woodland_fern/`, and load them using Bevy AssetServer (`...glb#Scene0`). You do **not** need to regenerate them. Bevy glTF import may require the existing texture and asset-loader configuration. Keep all 4 at the same origin and orientation. Import a single LOD at a time for an instance; do not draw all 4 simultaneously.

Suggested initial screen-space / distance tiers for this small plant:

- `LOD0`: within ~3 m of camera
- `LOD1`: ~3–9 m
- `LOD2`: ~9–25 m
- `LOD3`: ~25–60 m; after this, ground-cover patches or no individual fern

These are **starting values**, not hardcoded limits; measure on the RX 6750 XT, add hysteresis and dithered transitions, use GPU instancing/batching for identical GLB meshes. Avoid spawning separate entities for individual leaflets.

## View the plant in Blender

From your repo directory after unzipping:

```sh
blender -b --python woodland_fern/create_blend.py -- --asset-dir ./woodland_fern
blender ./woodland_fern/woodland_fern.blend
```

In Blender press **Z → M** to display Material Preview. The .blend helper is a convenience; Blender is **not required** to use the game-ready `.glb` files.

## Regenerate / vary the plant

Python dependencies: `numpy`, `Pillow`, `trimesh`.

```sh
python woodland_fern/build_woodland_fern.py --output ./woodland_fern --seed 80327
```

Change `--seed` to produce a different arrangement. Each generated variant must get its **own set of four files**, because the output filenames are reused. Pre-generate variants offline; avoid rebuilding leaves for each plant instance at runtime.

## Verification notes

Created and GLB-parsed with Python trimesh; triangle counts and bounds were checked. A preview was rendered by a lightweight software renderer, **not Blender Eevee or a live Bevy build**. Blender import, game appearance, wind shading and transitions should be visually verified by Codex/user after import.
