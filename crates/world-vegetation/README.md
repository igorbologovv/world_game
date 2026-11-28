# World vegetation

`ProceduralForestPlugin` streams deterministic tree sites around the world
camera. Terrain, climate, slope, water, clearings and construction determine
placement. Each tree uses the planet grid, a local radial up axis and its
canonical scale and yaw. The canopy material, ferns and ground cover read
the same placement data.

## Hybrid tree renderer

World-client runs the multi-species Igor_renderer hybrid forest from
`f75f11e`. Igor loads the original GLBs and textures, builds the shared middle
canopy templates and far Surface Nets cells, and selects LODs using the
original projected bounds, ranked hysteresis, and finite-time coverage
transitions. The mesh and proxy shaders share direct sunlight and canopy
shadow attenuation. They retain Igor's wind, alpha cutoffs, world-space
coverage and ordered sample masks.

World-client supplies canonical sites in local tangent frames along with the
Bevy camera projection and direction to the sun. Igor draws into the camera's
color and reverse-Z depth before Bevy's opaque pass. Bevy then applies the
same atmospheric scattering to the forest and terrain. The camera's MSAA
count drives the sample masks, including the distance fade to terrain canopy.

`VegetationSettings` controls the stream range and preload margin. Igor's
`IGOR_CANOPY_*` settings control LOD thresholds, transition durations and
coverage. F8 shows submitted triangles, selected trees by LOD and ongoing
transitions. The standalone Hero Oak GLB and textures are shipped under
`assets/models/hero_oak/igor`. Stock procedural geometry remains the fallback
for tree variants without a packaged GLB. See the [library sources and
licenses](../../assets/models/vegetation/library/SOURCES.md) and
[vendored renderer](../../vendor/igor-renderer/README.md).

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

## Procedural ground plants

Ground cover streams 40 species with three seeded shapes per species. Each
physical patch seeds several clumps, with a dominant species and companion
species drawn from the same habitat. Temperature follows latitude and
elevation. Moisture combines the regional climate with local damp pockets,
river and lake coverage, and coastal influence. The existing canopy climate and
local grove density favor ferns and shrubs in shaded woodland, while open meadows
favor grasses and flowers. Wet areas favor moisture-tolerant plants. Salt-
tolerant species stay in coastal lowlands. Individual roots are checked against
the terrain slope, sea-surface elevation, river and lake coverage, and the
species' elevation limit.

Workers build both LODs from the same placements. Each patch uses one mesh
and one shared foliage material, with a second mesh during dither transitions.
Compact plants use up to 128 triangles each. Up to eight nearby patches can
hold a detailed mesh, each capped at 9,600 triangles including background grass.
Detail enters at 17 m and leaves at 22 m from the patch center. Compact cover
fades across 44–62 m. Per-frame uploads are capped at two compact patches and
one detailed patch. Legacy grass and gravel fill gaps between clumps.

The five runtime GLBs contain the generated meshes used by the streamer.
See the [plant library rebuild command and source catalog](../../tools/PROCEDURAL_PLANTS.md).

## Orbital canopy

The terrain samples a 1024 × 1024 cubemap per face with eleven mip levels.
Each texel estimates foliage cover and color from stratified groups of the
same tree candidates used by the hybrid renderer. The bake uses the shared
terrain, hydrology, slope, climate and biome checks. The cubemap filters
across face boundaries and averages linear, premultiplied color and coverage
over the texel solid angles. Camera-centered layers resolve finer stands and
crowns near the mesh fade. F9 cycles the coverage and color diagnostics.

The default world ships its own bake at `assets/world/planet-canopy-v2.gz`.
Regenerate it after changing the terrain inputs or the canopy sampling
algorithm. When tree placement, terrain sampling, or the bake changes, bump
the bake version and regenerate the packaged file.

```sh
cargo run --locked -p world-vegetation --example bake_canopy -- assets
```

The baker also accepts an optional output file. It hashes the source height
faces, river and ocean atlases, lake masks, and inland water meshes to detect stale bakes. The runtime loads and
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
WGPU_BACKEND=vulkan cargo test --locked --release -p world-client forest_altitude_sweep -- --ignored --nocapture
```

Screenshots are written to `target/forest-sweep`. Set `FOREST_CAPTURE_DIR`
to keep different runs separate.

Run `tree_lod_sweep` for the near-to-far camera dolly and shared-depth check.
Run `broadleaf_lighting_sweep` for captures from both sides of the forest sun direction.
