# Vendored Igor renderer

The game builds the local [Igor renderer crate](../vendor/igor-renderer) with
default features disabled. The crate contains the upstream source from
`d552eb12791240c14b939fa53afe76632304266b`. Its standalone demo remains
outside the game workspace and uses its own assets and dependencies.

The library owns the CPU point baker, template LODs, material atlas,
aggregation and the planetary compute shader source. The game owns Bevy
resource allocation, spherical placement/selection and depth composition.
The library assembles the planetary shader from the standalone shader.
The Bevy adapter applies the game's lighting, reprojection and depth changes.

Run:
```sh
IGOR_POINT_FOREST=1 cargo run --release -p world-client
```

Look for IGOR HIERARCHY in the overlay.
No second wgpu device, window or legacy billboard fallback is used.

The game captures shared site batches when sampled terrain patches or model
sources change. Async compute workers collect and sort placements, build the
immutable hierarchy, and bake geometry packs as needed. The selector continues
using the published hierarchy and its matching pack until a compatible result
is ready. There is one hierarchy task and one geometry bake at a time. New
placement revisions are captured when the hierarchy worker slot opens. Results
from the current terrain and model generations can publish in stream order
while patches arrive. An obsolete hierarchy is retired on a worker.

PlanetScene retains per-tree and per-cell LOD history and the budget controller
across swaps. A stationary view can continue to reuse its selection and uploaded
instance and job buffers after the library's refinement interval. Workers also
collect shadow casters and bake the same opacity field from the matching
hierarchy and geometry.

The frame still captures patch and portrait references, polls task readiness,
and swaps completed snapshots. Camera culling, LOD and budget selection, and
instance and job generation run on the main thread when the view or hierarchy
requires a new selection. GPU uploads and render passes remain in the render
world.

For Tracy, build with `--features bevy/trace_tracy` and set
`IGOR_POINT_FOREST=1` and `RUST_LOG=world_vegetation=trace,igor_renderer=trace`.
Compare `point_forest.capture_site_batches` and `point_forest.publish_snapshot`
with the worker's `point_forest.build_hierarchy` and its placement and hierarchy
stages. The revision messages link stream changes, scheduling, and publication.

Quality trial: game default is 6,000,000 selected points per frame (override with IGOR_POINT_BUDGET). The original 22 LOD levels and selection policy remain unchanged. The job limit also respects the actual device storage limit for the padded projection cache. The settlement HUD is below the FPS readout.

Shadows and legacy undergrowth are enabled by default in point mode. Set IGOR_POINT_SHADOWS=0 or IGOR_POINT_UNDERGROWTH=0 independently. The library bakes local canopy opacity asynchronously from the original 1m LOD; a ground modulation pass samples opaque scene depth before point composition. The shadow field covers a local 512m region, not the entire planet. Undergrowth is the existing renderer for grass, ferns, shrubs and flowers, not yet converted to points. Tree budget stays 6M.

The Bevy adapter keeps the camera's MSAA sample count and samples opaque shadow receivers per sample. Splat lighting uses exposed HDR direct light, a hemispherical sky approximation, and distance haze. Color stays in half-float until it reaches the game's HDR target.

The leaf flutter phase is anchored in planet coordinates. A replacement tree pack preserves LOD history for existing placements, and canopy fields fade over 0.4 seconds. Point colors use unjittered reprojection with depth rejection and neighborhood clipping. Newly uncovered pixels use the current frame immediately.

Opaque depth seeds the existing point depth pass. With MSAA, the farthest opaque sample provides the bound, so a splat can still cover openings at terrain and object edges. The owner pass reuses only projection entries that accepted at least one sample. Set `IGOR_POINT_OCCLUSION=0` to compare opaque-depth seeding with the original depth clear.
