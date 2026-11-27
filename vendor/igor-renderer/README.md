## Planetary host API

`planetary::Pack::build` accepts decoded model arrays from an external
engine, invokes the shared original template/atlas builders, and provides
packed upload data. `planetary::compute_shader()` assembles the existing
Bevy variant from the original shader with explicit host-specific splices.
The standalone shader is not changed. The host still owns GPU allocation,
spherical placement/selection and depth composition; wgpu 0.19 is not
enabled when used with Bevy. The planar shadow map and optional compute
passes are not yet adapted to the planet. This extraction preserves the
previous game renderer's behavior rather than claiming visual parity.

# Embedding Igor Renderer

The renderer is a Rust library; the original windowed executable is now a
consumer of its GPU API. This refactor starts at stable wind/shadow commit
9d50c07f471b5ddaf9ee6c50b4e3657df0d387f1. All original WGSL files are unchanged.

- `default-features = false`: CPU geometry, exact LOD builder, flat scene
  selection, original canopy shadow builder, public packed ABI, atlas upload
  data and original shader source strings. No wgpu or window dependency.
- `features = ["gpu"]`: original wgpu 0.19 compute renderer, persistent
  SceneBuffers uploader, original job generation and compute passes. The host
  supplies its existing device, queue and command encoder. No window required.
- Default `demo`: same executable, controls, rendering and settings as before.

```toml
[dependencies]
igor_renderer = { git = "https://github.com/igorbologovv/Igor_renderer.git", branch = "feature/renderer-library", default-features = false, features = ["gpu"] }
```

CPU entry points: TreeGeometry::load_splats, PointScene::build_templates,
PointScene::build, PointScene::select_stable, PointScene::shadow_field.
GPU entry points: SceneBuffers::upload, SceneBuffers::resources,
ComputeSplats::new, prepare, prepare_history, encode_passes, resolved_view,
screen_words, encode. FrameUniform and shader/job layouts live in render_api.
`encode_passes` records work without presenting or submitting. The host must
supply matching buffer capacities, uniforms and synchronization. The original
resolve shader still includes the demo ground and sky; its view-space depth
is not Bevy's hardware reversed-Z depth.

**Integration boundary:** wgpu 0.19 handles cannot be passed into a newer
wgpu device. A Bevy backend should use the version-neutral CPU/ABI API
with its own compatible resource adapters. The existing original planar
scene uses two sites per cell, quarter-turn rotation and 60-degree FOV;
arbitrary spherical placement, camera projection and Bevy compositing
are not implemented by this extraction. Do not copy/rewrite the CPU baker
or shaders in the game to bridge those interfaces. No second renderer
implementation has been added.

Validation:
```sh
cargo check --lib --no-default-features
cargo check --lib --no-default-features --features gpu
cargo test --lib --no-default-features
cargo test --test shaders
cargo run --release -- --side 128 --aerial
```

# Wind and directional canopy shadow experiment

Based on stable main (3.5M points, 22 LOD levels). B toggles wind and V
toggles shadows; both start on in compute mode. IGOR_WIND=0 and
IGOR_SHADOWS=0 disable each effect. IGOR_WIND_STRENGTH=1 and
IGOR_SHADOW_STRENGTH=0.85 control intensity. The wind uses coherent
height-dependent crown sway and alpha-tested foliage flutter, fading
from 80 to 140 m. Ground, world aggregates and source UVs remain fixed.
There is no audio, skeletal animation or per-leaf mesh.

Shadows use a camera-independent static light-space opacity field
built once from shared 1 m LOD points of all trees. RGBA stores four
height bands; one bilinear sample per shaded pixel attenuates direct
sunlight above the receiver while keeping ambient light. The field is
limited to 2048x2048 (16 MiB); IGOR_SHADOW_METERS=0.75 requests spacing,
automatically increased for large scenes. Includes approximate canopy
self-shadow and off-screen tree shadows, not exact dynamic leaf shadows.
Sun direction is fixed. No ray tracing, extra per-frame shadow pass or
tree triangles. Legacy mode ignores both effects. Other experimental
modes are still off by default.

# Stable main configuration

The default compute renderer uses a 3,500,000 point budget, 22 local
LOD levels, stable LOD hysteresis, early Z, fast subpixel projection
and projection caching. Hi-Z, brick LOD, GPU-assisted LOD and TAA
remain opt-in experiments. This configuration was selected after
interactive testing on an RX 6750 XT.

    cargo run --release -- --side 128 --aerial

Override IGOR_POINT_BUDGET to test other budgets. Historical settings
and commands in the sections below are retained for comparison.

# Stable defaults after the combined experiment

The brick, GPU-assisted LOD and temporal passes are opt-in after a
reported combined FPS regression and visible shaking. The 22-level
whole-tree LOD, early Z, fast subpixel projection and projection cache
remain the default. Start with:

    IGOR_POINT_BUDGET=3500000 cargo run --release -- --side 128 --aerial

Then compare each experimental mode separately using IGOR_BRICK_LOD=1,
IGOR_GPU_LOD=1 or IGOR_TAA=1. G and T toggle their modes at runtime.
Temporal accumulation now defaults to a fixed projection even when
enabled; IGOR_TAA_JITTER=1 explicitly enables the subpixel jitter for
testing. The title reports the active experimental flags. This isolates
the new passes, rather than claiming the underlying regression is fixed.

# Experimental temporal accumulation

T toggles temporal antialiasing (IGOR_TAA=1 enables it at startup).
Eight subpixel Halton offsets, previous-camera reprojection,
log-depth history rejection, 3x3 neighborhood color clipping and
motion-dependent blending. History is reset on resize, camera reset,
compute/legacy switch, GPU LOD switch and TAA toggle. HUD is drawn
after accumulation. Two RGBA16Float history textures cost 16 bytes
per screen pixel; budget checks include them. This is a first
implementation for static geometry and camera motion, not animated
vegetation motion vectors. History can soften fine details or ghost.
Use T to compare. No geometry rasterization is introduced.

# Independent brick and GPU-assisted LOD

In opt-in brick mode, near/middle templates are partitioned in place into 4 m bricks;
whole-tree ranges still work for far trees. Brick visibility and LOD
history are independent. IGOR_BRICK_LOD=1 enables brick selection.

G toggles GPU-assisted LOD (IGOR_GPU_LOD=1 enables it at startup). The CPU still
chooses candidates and reserves a strict maximum point budget and
prefix offsets. A GPU pass selects from up to two cheaper ranges per
candidate, updates instance offsets/counts, and generates point jobs.
Unused reserved jobs are empty. This is not yet a fully GPU-driven
hierarchy traversal or compact indirect dispatch. The title reports
the CPU upper-bound point count, not the actual reduced GPU count.
GPU mode never increases the number of submitted points beyond that bound.
Legacy quads ignore GPU LOD.

# Extra mid-distance LOD experiment

The default hierarchy now has 22 local template levels instead of 15.
New intermediate grid widths: 0.625, 0.875, 1.25, 1.75, 2.5, 3 and
3.5 meters. The existing near-field levels and 4/8/16 m levels remain.
The additional grids are sparse, deterministic, shared across instances,
and built from the existing alpha-tested samples. World hierarchy,
compute shaders, early-Z, cache, hysteresis and hard point cap are unchanged.
For a controlled comparison at a 3.5M point budget:

    IGOR_POINT_BUDGET=3500000 cargo run --release -- --side 128 --aerial
    IGOR_MID_LOD=0 IGOR_POINT_BUDGET=3500000 cargo run --release -- --side 128 --aerial

IGOR_MID_LOD=0 restores the 15-level hierarchy; IGOR_DENSE_LOD=0
still restores the original eight. These are startup settings.
Extra shared template records cost memory and startup time, but the
per-frame selected point cap is unchanged. This adds smaller discrete
steps, not a continuous blend or TAA. Measure visual transitions and FPS
with equal point budgets, resolution and camera path.

# Denser near-to-mid LOD

Default local template levels (meters): raw, 0.0625, 0.09375, 0.125,
0.1875, 0.25, 0.375, 0.5, 0.75, 1, 1.5, 2, 4, 8, 16.
Seven extra levels are built from the same alpha-tested samples in
sparse occupied-cell grids; template records remain shared by every
instance. Existing far/world aggregation is unchanged. The global
point budget remains hard. Local hysteresis is +/-10% in the denser
mode, with a fixed +/-6% per-tree threshold offset to stagger changes.
The stable budget threshold adjusts in smaller increments.
IGOR_DENSE_LOD=0 restores the original eight local levels and original
local hysteresis/threshold steps without building extra records.
Startup logs counts by level and total allocation. More levels cost
additional startup work and template memory; actual benefit depends
on the point budget. Raw-to-baked shading still changes discretely,
so this is not cross-fading, temporal anti-aliasing or per-chunk LOD.

# Fast projection and compact reuse

F toggles the conservative single-pixel fast path (default on,
IGOR_FAST_TINY=0 disables it). It tests a sphere-based projected diameter
before decoding the normal, building tangents or projecting disk corners.
P toggles projection caching (default on, IGOR_PROJECTION_CACHE=0).
Eight bytes per padded point slot hold packed screen bounds, or the
pixel and already alpha-tested depth for tiny points. Tiny owners need
no second projection/alpha lookup. Larger points reuse their bounds,
but still reconstruct their disk axes and local coordinates.
The cache automatically falls back to uncached mode if the GPU memory
budget, adapter limit or supported screen coordinate range is exceeded.
The title reports actual cache use, not merely the requested setting.
Compare F and P independently. No triangles are introduced.

# Stable LOD and early depth rejection

Hi-Z is now OFF by default (O toggles it, IGOR_OCCLUSION=1 enables on
startup). The observed case rejected 83,239 of 5,778,974 points, so
the extra Hi-Z passes are not justified for that scene.

L toggles stable LOD (default on, IGOR_STABLE_LOD=0 for baseline).
World cells and individual trees keep their previous selected level by
stable spatial index, not draw order. A +/-15% hysteresis band prevents
oscillation near each LOD boundary. The last accepted global threshold
is retained, coarsened immediately if the point cap is exceeded,
and refined by a small step only after 12 spare-capacity frames
(candidate must fit within 85% of the budget). Failed budget probes
do not modify history. R resets camera and history; changing budget
or target also resets history. This does not cross-fade LODs, add TAA,
or remove subpixel sampling shimmer.

E toggles early-Z (default on, IGOR_EARLY_Z=0 for baseline). After
computing the candidate pixel depth, the compute shader tests existing
depth before barycentric coordinates, material loads and alpha texture
sampling. Depth passes can discard candidates at or behind a known
accepted depth; the separate owner pass accepts only matching final
depth and still alpha-tests candidates. Resolve is unchanged.
Both modes retain the deterministic minimum owner ID on equal depth.
No new depth pass or GPU buffer is required for early-Z.
Title labels both modes. Compare E on/off with identical camera and LOD;
compare L separately while moving. No measured speedup yet.

# Current-frame Hi-Z occlusion experiment

O toggles occlusion culling (off by default,
IGOR_OCCLUSION=1 enables at startup). Use C only to compare against the
old quad renderer. Hold the same camera, resolution and point budget
while comparing O on/off.

The CPU still selects the same LOD instances. Each instance has a
conservative world-space sphere enclosing its selected point disks.
The GPU first renders the depth of nearby instances (nearest bound
depth + IGOR_OCCLUDER_RANGE, default 96 m), builds a maximum-depth
pyramid at 4/16/64/256-pixel tiles, then tests remaining instance bounds.
Only a strictly closer complete cover can reject an instance. Empty
pixels propagate through the max pyramid; near-plane or uncertain
bounds are never rejected. Rejected instances skip point projection
and alpha/coverage tests in both main passes. The prepass depth is
reused, not cleared. This uses the current camera and current LODs,
not stale previous-frame depth, and retains deterministic owner IDs.
It is an instance-level test (tree or selected aggregate block),
not yet recursive world-block culling before CPU selection.

The title and terminal report rejected instances and points and
near-occluder counts (GPU counters sampled after completion about
once a second). Total selected splats and LOD are unchanged by O;
a culled point saves both depth and owner traversal, not necessarily
every GPU scheduling cost. Job dispatch still includes rejected jobs.
The extra depth pass selection and hierarchy can cost more than they
save in sparse foliage or aerial views. No performance guarantee.
Compare visual correctness, FPS and culled points at the same camera.
Culling is designed to be conservative for the renderer's existing
approximate disk footprints; it does not fix existing LOD flicker.

# Compute-based oriented point rendering

The default path now projects fixed 3D oriented disks directly in compute.
No tree or ground triangles, no billboards, and no ray/scene traversal.
All levels use their stored normals; camera changes projection only.
A 64-lane job maps to a contiguous slice of a selected shared point template.
No per-point expanded instance array is uploaded.

Four dispatches clear depth and owner, find minimum positive view-depth bits
with atomicMin, select the smallest matching point ID (separate pass prevents
depth/owner races), and shade visible winners into a linear RGBA8 texture.
Detailed points preserve source triangle boundary clipping, alpha masks,
texture color and normal mapping. Coarse points use baked color/coverage.
A final fullscreen pass displays the resolved texture and HUD; its triangles
do not represent scene geometry. The flat ground background is evaluated
by closed-form planar projection.

C switches to the previous quad renderer for comparison; only that explicit
legacy mode uses camera-facing footprints. IGOR_SPLAT_LEGACY=1 starts there.
The default budget is reduced to 1.5 million points while testing compute
safety. Use the same IGOR_POINT_BUDGET (e.g. 1500000 or 6000000) for both
modes. H still selects the stress budget; avoid H until normal mode is stable.
All other controls remain. The title labels the active renderer.

Known limitations: subpixel primitives are deposited into one pixel, near-plane
crossing disks are clipped for bounds, and exceptional large footprints are
restricted to a centered ~96x96 pixel window to bound per-point work. This can
clip nearby geometry. Fixed coarse normal disks may cause edge-on holes.
Depth/owner coverage is evaluated twice; atomics and divergent loops may
make this slower than hardware rasterization. No measured speedup yet.
The memory estimate includes initial screen buffers but not job buffer growth,
driver allocations or resize; job buffers are checked against adapter limits.
Run cargo test --test shaders and cargo test --bin igor_renderer.

# Textured oriented splats (quality update)

The direct projection renderer retains every original raw surfel and its source
surface ID (still 16 bytes per raw record). Raw discs now lie in the original
surface plane, with perspective-correct local coordinates, hardware depth,
source triangle boundary clipping, per-fragment base color/alpha and normal maps.
There is no ray tracing. Aggregated local points are oriented with a slight
view-facing bias; very small points and large world representatives retain
screen-facing coverage. LOD colors/coverage are estimated from five samples.
Near trees get a tighter LOD threshold than distant ones when coarsening.

Default target is 0.75 pixels and budget is 6 million submitted splats.
H toggles the explicit stress budget (30 million, 20x the original 1.5 million
budget). This is a maximum, not a guarantee of 20x actual points or image detail.
Start with the default before trying H: 30 million splats can be slow.
IGOR_POINT_BUDGET and IGOR_STRESS_POINT_BUDGET override the caps.
IGOR_TEXTURE_MAX=1024 or 2048 restores higher texture resolution at extra memory
cost. GPU allocation estimate includes atlas and surface buffers. Title reports
the actual count of full-detail textured splats as well as total splats.
Coarse LOD remains approximate; no mipmaps or antialiasing yet.

# Igor Renderer: direct hierarchical point splatting experiment

This branch defaults to direct projection, not ray tracing. The old renderer
is preserved as a baseline (`--ray` or the original feature branch).

    cargo run --release -- --side 128 --width 640 --height 360 --aerial
    cargo run --release -- --side 1024 --density 100 --aerial
    cargo run --release -- --side 128 --width 640 --height 360 --aerial --ray

Same forest seeds, models, two trees per occupied site and instance transforms.
The existing GLB assets must be available at assets/trees. In direct mode
the importer generates surfels but skips local ray binning, distance-field
construction and ray-baked images. Coarse surfels sample their source material before upload; full-detail surfels retain material references.

* Each model has 16-byte colored/normal points and flat aggregate grids at
  0.25, 0.5, 1, 2, 4, 8 and 16 metres. Empty cells emit nothing. Parent
  records reduce weighted positions, colors, normals, and coverage.
* The world has flat directly indexed 16, 64, 256, 1024 metre etc levels
  until a single top cell. Each cell contains a world representative and
  coverage. Selection starts at the top level, culls conservative bounds
  against the view frustum and either emits one aggregate or visits children.
* Per visible tree, projected cell size selects a whole model LOD. GPU
  instancing reuses templates; only selected tree/world instance records
  are uploaded per frame. The GPU projects camera-facing circular splats
  using generated surface-oriented quads or coarse screen footprints and hardware depth testing. No original mesh
  triangles are submitted and there are no ray intersection calculations.
* Target 0.75 pixels by default (IGOR_LOD_PIXELS); +/- changes it. A
  selection target of 6,000,000 splats (IGOR_POINT_BUDGET) raises the LOD
  threshold when needed; the applied threshold is printed, and if it still
  exceeds the budget the frame is not submitted.
* The HUD shows completed FPS and frame interval. Window title and stdout
  report actual selected splats (not logical world points), selected tree
  instances and world representatives, visited occupied blocks, draw calls,
  applied LOD threshold, CPU selection time and submission-to-completion
  wall time. One frame in flight and 4 ms cooldown by default
  (IGOR_GPU_COOLDOWN_MS). Not hardware timestamp timing.
* WASD, arrows, space/shift, left Ctrl, R, Esc as before.

Limitations: coarse sampling and aggregate coverage are approximate; coarse disks do not exactly preserve silhouettes or visibility. Detailed disks use projected surface depth. Tree LOD is selected per instance,
not per local cell; no GPU-driven compaction, occlusion hierarchy or dynamic
world streaming yet. The CPU still traverses visible world blocks and submits
batched draws. GPU memory usage is bounded by uploaded point data and the
instance buffer, not the expanded count of logical surfels. Benchmark image
quality as well as frame time. Validate WGSL with cargo test --test shaders.

## Previous ray renderer documentation

# Igor Renderer

A fullscreen GPU ray caster for textured surfels. The forest uses the LOD0 tree
assets from `world-client`, including both variants of all 30 library trees,
Hero Oak, the two woodland trees, Coconut Palm, and Tree (part9).

## Run

Use Rust stable and a native wgpu adapter with Vulkan, Metal, or DX12. Run the
viewer in release mode.

```sh
cargo run --release
cargo run --release -- --side 1024 --density 100 --aerial
cargo run --release -- --headless --compare --side 24 --density 45 --width 160 --height 90 --aerial
cargo test
```

The default forest covers 8.192 km per side, with 512 × 512 potential sites and
95% deterministic occupancy. Each site places a mature and an understory
tree with varied positions and quarter-turns. The maximum setting places
2,097,152 trees over 16.384 km per side. Instances share about 7.76 million
surfel records across 65 tree batches. Both the ground and aerial views use the
same full-density source geometry; the GPU defaults to a separate approximate far LOD.

Use WASD to move, the arrow keys to turn, Space and Left Shift to change
altitude, and Left Ctrl to fly faster. Press R to switch between ground and
aerial views, F to toggle empty-space skipping, L to toggle the approximate far LOD, and Esc to exit.

The top-left counter displays FPS over a short rolling interval and the most
average completed-frame interval in milliseconds (includes deliberate pacing and CPU work; not a GPU timestamp).

## Surfel conversion and traversal

- At startup, each original triangle is subdivided into patches covered by
  oriented disks. The loader uniformly scales only the models that need to fit
  inside a 16 m cell. Base colors from material textures, interpolated vertex
  normals, normal maps, and leaf alpha cuts are sampled at the ray hit.
- The world uses the existing 16 m, 64 m, and 256 m flat grids. Each world cell
  stores an 8-byte batch reference and a Chebyshev distance to the nearest
  occupied cell. DDA traversal can jump to the boundary of a guaranteed empty
  region using that distance.
- Each template has a shared flat grid with 0.5 m bins. A bin indexes every
  surfel that can overlap it. Rays traverse the local bins and test disk planes
  and radii analytically. Source triangle bounds clip disk overlap at tree
  edges. The ground is an analytic plane.
- The GPU draws a fullscreen triangle. Its fragment shader ray-casts the
  forest and fills the FPS panel. Only the view and counter uniforms change
  every frame.
- Headless CPU mode follows the same traversal and writes a PPM. It reports
  world block lookups, jumps, surfel tests, and wall time. Use `--compare` to
  check empty-space skipping against fine-cell traversal.

The scene is finite and resident. Independent fragment rays are still used.
The new exact local Chebyshev distance grid skips empty 0.5 m bins. Dense
world traversal reads the fine cell first and consults coarse grids only on
empty cells. CPU --compare checks exact skips on/off (never uses proxy LOD).

The GPU additionally uses six 32x32 orthographic depth/color/normal maps baked
from alpha-tested surfel hits at startup. At >=0.30 template metres per pixel
it directly samples a selected view. This is an APPROXIMATION: off-axis
silhouettes, depth and holes can differ from full detail. Press L or set
IGOR_FULL_DETAIL=1 to disable. No mesh rasterization or octree is introduced.

GPU safety defaults: one submitted frame in flight, FIFO presentation,
16 ms idle cooldown after GPU completion (IGOR_GPU_COOLDOWN_MS=0 disables
cooldown), texture dimensions capped at 512 (IGOR_TEXTURE_MAX=2048 restores
original resolution up to 2048), and an estimated 1536 MiB scene allocation
budget (IGOR_GPU_BUDGET_MIB). The estimate includes buffers and the full texture
atlas, not driver allocations, swapchain or other applications. The cap is not
a hardware VRAM query. FPS is completion-based; use GPU timestamps separately
for kernel performance. The local assets and Cargo.lock must be present to run.
See [tree asset attribution](assets/trees/SOURCES.md) for the imported models.

## Packet and temporal experiment

Default viewer uses 8x8 compute workgroups. Up to 16 coherent rounds elect
an active lane's next cell, count matching rays, and share the cell's metadata,
empty region and two prepared tree transforms/bounds. Per-lane AABB tests
produce 64-bit canopy and understory masks (two u32 words per mask). A lane
only traverses a batch if its mask bit is set. When fewer than four rays
share the cell, or the round budget ends, unfinished rays continue from their
current cell in the independent scalar tracer. The detailed local tree bins
and surfel tests remain per-ray; this is a world-level packet experiment.

Two RGBA16F textures store linear color and hit distance. In an unchanged
static view the prior refreshed frame is copied without ray tracing. While
moving slightly, alternate pixels may reproject independently traced hits
from the previous frame if their projected position agrees within 0.4 pixel;
failed samples are freshly traced. Reused hits have negative stored depth so
cannot be reused again during motion. First stationary frame after movement
is fully refreshed. History invalidates on mode changes, resize and camera
reset. This is approximate: occlusion/disocclusion errors and flicker remain
possible. Disable temporal mode for correctness/performance comparisons.

- P: toggle packet traversal (same compute/present pipeline).
- T: toggle temporal reuse (off for fresh-frame measurements).
- B: toggle original-style independent fragment renderer.
- F: exact empty-space skipping; L: distant approximate geometry LOD.
- IGOR_PACKET=0 / IGOR_TEMPORAL=0 / IGOR_LEGACY=1 select initial modes.

Compare with the same scene, camera, size and L/F settings. For example:

    IGOR_TEMPORAL=0 IGOR_PACKET=0 cargo run --release -- --side 128 --width 640 --height 360 --aerial
    IGOR_TEMPORAL=0 IGOR_PACKET=1 cargo run --release -- --side 128 --width 640 --height 360 --aerial
    IGOR_TEMPORAL=1 IGOR_PACKET=1 cargo run --release -- --side 128 --width 640 --height 360 --aerial

HUD reports completed FPS including pacing. Title shows submit-to-completion
wall time (NOT hardware GPU timestamps) and the current path: cached,
reproject, packet, scalar compute or legacy. Cached FPS is not fresh rendering
FPS. Measure temporal reuse while moving, as well as stationary caching.
The extra textures use 16 bytes per pixel total; GPU allocation budget is
still approximate. GPU packet speedup is unverified; barriers and divergence
may make some views slower. Run cargo test --test shaders to validate WGSL
without needing the tree assets. GPU execution must be tested on hardware.

### Persistent planetary selection

`planetary::PlanetHierarchy` stores the sparse 16/64/256/1024/4096 m hierarchy and exact weighted parent reductions. Build it from sorted placements and a geometry pack with `PlanetHierarchy::build`, then install it with `PlanetScene::replace_hierarchy`. That method returns the previous hierarchy and keeps the scene's per-cell and per-tree hysteresis and global budget controller. Use the same geometry pack for `select` and shadow baking as for the installed hierarchy. `PlanetScene::update_sites` is available for synchronous placement updates against the same pack. Selection results contain GPU-ready instance and job words.

Planetary shadows use `PlanetShadow::bake`, the original opacity raster with a local sun-projected tangent basis, 512x512 at 1m per texel and four 8m height bands. The host can cache/rebuild asynchronously and use the shared visibility function for both point color and opaque-scene modulation. Original demo shader and default planar field remain unchanged. This is an approximate static canopy field, not animated per-leaf shadow casting.
