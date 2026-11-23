# World vegetation

Install `ProceduralForestPlugin` alongside the shared world space plugin. It
reads `WorldCamera`, the floating-origin frame, and the authoritative terrain
and river queries from `world-core`. It has no dependency on player movement,
terrain rendering, or water rendering.

## Quality controls

Press **F8** to toggle the overlay. It shows active mesh LODs and blends plus
resident impostor and forest batches, placement work, asynchronous jobs, and
the retirement queue. Counts are taken before the renderer's frustum cull.

Insert a `VegetationSettings` resource to override the defaults. The reference
view is 1080p at a 60-degree vertical FOV. The defaults are about 30 m for the
detailed mesh, 30–100 m for simplified meshes, 100–400 m for impostors, and
2 km for the outer forest fade. Mesh LOD thresholds use projected tree height
with 10% hysteresis. Tree size, viewport height, and FOV affect the mesh and
impostor transition distances.

## Ownership inside the crate

- `placement` assigns canonical tree sites with the existing spherical
  latitude patches, seeds, biome rules, and height and river queries.
- `streaming` discovers an all-direction preload ring and prioritizes nearby
  and forward chunks. It budgets height queries on the main thread. Workers
  bake meshes from immutable site lists, and the main thread limits uploads,
  detail admission, and retirement. The cache holds up to 300 individual
  detail trees. Impostor coverage stays available when a detail request is
  waiting for that budget.
- `lod` owns projected-size selection, reversible transitions, and the shared
  material palette. Shared mesh and material handles allow Bevy 0.20 to batch
  matching detailed instances on supported render backends.
- `impostor` packs the procedural and authored portraits and the exported
  Hero Oak silhouette into one mipmapped atlas. `proxy` merges one
  camera-facing cutout per tree into a chunk. The parent forest batch aggregates
  the same cutouts from multiple child chunks into one draw. It uses the same
  tree roots and silhouettes during the handoff.
- `ground_cover` preloads grass and plant patches beyond their dithered
  display range and fades them in after loading.

All tree tiers use the same screen-space dither pattern. Their coverage
intervals meet at the same thresholds and give each shared pixel to one tier. The coarse parent fades away behind
resident impostors, and a chunk's impostors cover its sites until the detail
entities are ready. Frustum culling uses conservative mesh bounds, and parent
forest batches are also culled against the spherical horizon. A teleport into
a cold region still requires streaming.

Hero Oak trunk meshes share mipmapped albedo, normal, and roughness maps from
the supplied Jolcham Oak Bark pack. Albedo is sRGB. The OpenGL normal and
metallic-roughness maps are linear, with roughness in green and zero metallic
in blue. The trunk uses four repeats across the circumference, with matched
normals across each UV seam. The loader prepares tangents once and uses the
bark embedded in the GLBs if the standalone maps are unavailable or incompatible.
See the [terrain material baker](../world-terrain/README.md) for the source
pack and rebuild command.

## Streamed tree assets

| Model | Placement | Mesh triangles LOD0 / LOD1 / LOD2 | Source textures |
| --- | --- | ---: | --- |
| Coconut Palm (*Cocos nucifera*) | Hot, humid lowlands and warm coasts | 5,092 / 2,304 / 1,010 | Five `Tree_0Mat`–`Tree_4Mat` sets with base color, normal, and metallic-roughness. `Tree_2Mat` is the cutout frond material. |
| Tree (part9), unnamed broadleaf | One warm, humid jungle variant | 5,358 / 3,129 / 2,465 | `bark8` base color and normal, plus four cutout base-color maps (`tree4_leaf_small1`, `tree4_leaf_small2`, `tree5_leaf_small3`, and `trees_bushes_0001_a_1`). |
| Woodland trees 00 and 01, stylized deciduous trees | Two temperate oak variants | 4,599 / 2,068 / 827 each | Shared `Bark` base color, normal, and metallic-roughness, and `Leaves` base color and normal. |

The source exports identify the palm. The broadleaf and woodland sources give
no botanical species. The oak and jungle assignments reflect their shapes
and gameplay biomes. Hero Oak retains its existing variant. Procedural pines
and acacias still fill cool forest and warm, drier sites respectively.

Potential coastal palms sample terrain in eight directions up to 220 m away.
A probe below the sea datum classifies a warm site within 45 m of sea level as
coastal. Tropical groves use the forest's temperature and moisture fields and
may extend up to 420 m elevation. All sites still use the shared height, river,
and slope checks and the same deterministic patch seeds.

The assets under `assets/models/vegetation/` are standalone glTF meshes with
shared external textures across the three LODs. Each tree has one bark mesh and
one foliage mesh per LOD. The loader supplies each tree's portrait to the
existing atlas, so parent and child batches use the authored silhouette with
the same grid-relative root. The procedural versions cover sites while the
authored assets load.

Export with Blender and Pillow. The first command extracts the two woodland
intermediates from the original scene. The second packs the runtime models.

```sh
blender -b --python-exit-code 1 --python tools/extract_woodland_prefabs.py -- \
  --auto-trees 2 --output "$PWD/target/woodland_raw"
blender -b --python-exit-code 1 --python tools/build_tree_prefabs.py -- \
  --woodland-input "$PWD/target/woodland_raw"
```

The runtime assets are adaptations of **Coconut Palm** by evolveduk,
**Tree (part9)** by YN14R, and **Low Poly Tree Scene Free** by Nicholas-3D.
The adaptations separate trees from source scenes, normalize their geometry,
simplify LODs, and repack textures. [Coconut Palm](https://sketchfab.com/3d-models/coconut-palm-26e787f2ff2e4c0fb004c3b0210805a3), [Tree (part9)](https://sketchfab.com/3d-models/tree-part9-6fbd1594b62b4f94951dcca9e74a7347), [Low Poly Tree Scene Free](https://sketchfab.com/3d-models/low-poly-tree-scene-free-89daa5e21f0d4f08a59dba0d566e88bd), [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

## Woodland Fern

The four supplied GLBs are packaged in
`assets/models/vegetation/woodland_fern/`. Blender is only needed to edit the
source asset. The asset loader validates the entire set at startup. It reports
load or layout errors in the log and on the F8 overlay.

The public `FernSettings` resource controls distances, hysteresis, the preload
margin, blend duration, and the LOD0 budget. The default bands are 0–15 m,
15–35 m, 35–70 m, and 70–120 m. The default hysteresis is 10%, the preload margin
is 30 m, and up to 12 ferns can use the 33,600-triangle LOD0 mesh. The far range
is capped at 195 m to bound the streamed region. The last band fades out
gradually near its outer edge.

Fern clumps use the canonical tree sites to estimate canopy shade. Placement
also samples humidity, height, slope, and river coverage. Each clump has
seeded spacing, rotation, and scale. All four LODs use the same terrain-aligned
root.

| Export | Triangles per fern | Rendering |
| --- | ---: | --- |
| LOD0 | 33,600 | Shared indexed mesh and PBR material |
| LOD1 | 5,544 | Shared indexed mesh and PBR material |
| LOD2 | 1,280 | Shared indexed mesh and PBR material |
| LOD3 | 4 | Exported crossed alpha-mask cards merged by chunk |

The three detailed LODs merge each export's rachis and fronds and share
dither material variants through Bevy's normal batching. A detailed fern
needs one render entity, with a second only during mesh-to-mesh blending.
The distant layer uses one small mesh per occupied chunk with no fern
entities in that layer. Both layers use Bevy's frustum culling. F8 shows
fern asset status, per-LOD counts, blends, card batches, and queued chunks.

## Validation

```sh
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
```

The graphics comparison uses the complete client rendered offscreen at
832 × 468 with Mesa llvmpipe and Bevy's OpenGL backend. The deterministic
135-frame camera path starts at the starter region, travels about 930 m, and
rises from ground level to 166 m. It uses a fixed 100 ms simulation step.
Frame timings cover the client update and a render-device wait. Measurements
exclude the first 25 frames and do not include image readback. These results
measure the software renderer on this route.

| Build | FPS | Mean frame | Median frame | P95 frame | Worst frame |
| --- | ---: | ---: | ---: | ---: | ---: |
| Before | 6.98 | 143.31 ms | 134.74 ms | 208.65 ms | 671.29 ms |
| Continuous vegetation | 6.40 | 156.28 ms | 149.87 ms | 220.97 ms | 235.93 ms |

Wider forest coverage came with higher mean and P95 frame times on this run.
The worst measured frame fell by 65%. Native GPU performance requires a
separate measurement.

## Orbital canopy

The terrain samples a 1024 × 1024 cubemap per face with eleven mip levels.
Each texel estimates foliage cover and color from stratified groups of the
same tree candidates used for impostors and meshes. The bake uses the shared
terrain, hydrology, slope, climate and biome checks. The cubemap filters
across face boundaries and averages linear, premultiplied color and coverage
over the texel solid angles. Camera-centered layers resolve finer stands and
crowns for the impostor handoff. F9 cycles the coverage and color diagnostics.

The default world ships its own bake at `assets/world/planet-canopy-v1.gz`.
Regenerate it after changing the terrain inputs or the canopy sampling
algorithm. When tree placement, terrain sampling, or the bake changes, bump
the bake version and regenerate the packaged file.

```sh
cargo run --locked -p world-vegetation --example bake_canopy -- assets
```

The baker also accepts an optional output file. It hashes the source height
faces, river atlas, and lake masks to detect stale bakes. The runtime loads and
filters a matching packaged bake on a background thread. Custom worlds use
the per-user cache under `$XDG_CACHE_HOME/world-client/canopy` or
`~/.cache/world-client/canopy` on Linux. On a cache miss, a 64 × 64 preview
arrives first and the background worker builds and caches the full atlas.
The client uploads at most 1 MiB of orbital canopy per frame and only exposes
complete levels. The full GPU cubemap occupies 32 MiB. Crown relief is
generated from tree dimensions and species, so the bake requires no external
textures.

Run the camera sweep with a Vulkan-capable adapter. It captures the same
coastal ground, inland handoff, 24 km aerial, and 1,450 km orbital views and
prints median and 95th-percentile synchronized frame times.

```sh
WGPU_BACKEND=vulkan cargo test --locked -p world-client forest_altitude_sweep -- --ignored --nocapture
```

Screenshots are written to `target/forest-sweep`. Set `FOREST_CAPTURE_DIR`
to keep different runs separate.
