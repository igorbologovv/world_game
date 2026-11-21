# Connected planetary hydrology (experimental)

This branch replaces the previous local steepest-neighbor-only drainage with a
planet-wide, ocean-connected watershed graph. This is an **offline** computation:
it is not a per-frame GPU or simulation feature.

## Pipeline

1. Generate the existing six cube-sphere height faces (unchanged noise seeds).
2. Identify the largest *connected* below-sea-level body as the global ocean.
3. Run a priority flood from its coast, following all six cube faces. Each land
   cell receives an acyclic outlet and the minimum elevation at which water can
   escape to the ocean (its spill elevation).
4. Accumulate upstream contributing area in downstream-first graph order.
   River tributaries are now connected even across shallow and closed basins.
5. Keep sufficiently deep, contiguous filled depressions as **inland lakes**,
   with water at each basin's spill elevation. Fill only smaller raster pits.
6. Carve riverbeds and output water ribbons, plus connections from large lake
   outlets back into the downstream network. The existing river flow shader is
   reused; lakes are rendered as separate water surfaces rather than as pieces
   of the global sea-level sphere.
7. Regenerate the river/wet-area atlas. Lake cells are also wet-area masks so
   vegetation placement excludes submerged ground.

## Generated files

The normal generator rewrites:

- `assets/terrains/spherical/source/height/face{0..5}.tif`
- `assets/world/river_atlas.png`
- `assets/world/river_surface.bin`
- `assets/world/lake_surface.bin` (**new**)
- `assets/world/ocean_atlas.png` and ocean/basin face masks

The Bevy terrain runtime currently samples the TIFF source directly, so it
should see the same carved elevation as the river generator after a fresh bake.

## Try in a separate worktree

From a checkout of `feature/connected-hydrology` with the existing sibling
`../bevy_terrain` dependency:

```sh
cargo fmt --all
cargo check --workspace --all-targets
cargo test -p world-generation
cargo test -p world-generator
cargo test -p world-water
cargo run --release -p world-generator
cargo run -p world-client
```

**Important:** the checked-in world data on the base branch still represents
the previous hydrology. A fresh `world-generator` bake is required to see the
new drainage. Back up or commit any personally edited/generated assets before
running it. Do not merge generated binaries into the main development branch
until the visual review and terrain/water alignment checks pass.

## Visual and numerical acceptance checks

- Mountain catchments merge into connected downstream rivers, not dead-end
  fragments at local minima.
- Check representative rivers from mountain headwaters through confluences
  and any lake inlets/outlets down to the coast.
- Inspect rivers at cube-face seams.
- Inspect lake shorelines at walking distance and check for intersections with
  terrain, floating water, abrupt water-height steps and overlapping water
  surfaces. The local terrain may need a higher-resolution authored patch.
- Verify that ferns/trees no longer spawn in newly generated elevated lakes.
- Reprofile FPS and GPU workload on the real graphics card after regeneration.

## Limitations and next engineering step

This is a first **connected drainage** pass, not a physically complete
hydrological simulation. The source height grid is only 1024 x 1024 per
cube face, which is too coarse to create convincingly narrow, meandering,
walkable streams and bank erosion everywhere on a 100 km radius planet.

River centerlines still follow grid-neighbor edges, the shorelines of raised
lakes are tessellated from raster cells, and the same river PBR material is
temporarily used for lake water. These can look polygonal or sharp near the
camera. The next quality pass should spline/smooth centerlines **together with**
terrain carving (never move a river visually without changing its bed), use
higher-resolution local region patches and build proper lake shore geometry.
Hydraulic erosion and water-budget/climate modeling remain future work.

The changes have not yet been validated by a native GPU run in this authoring
environment. Treat this as a reviewable implementation branch, not as a
verified-final rendering result.
