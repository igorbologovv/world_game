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

Forest streaming records changed patch keys at placement arrival, replacement,
and eviction. When the worker slot opens, the frame captures the latest batch
or removal for each dirty key. The worker updates its patch-to-cell index and
rebuilds affected 16 m blocks and their 64, 256, 1024, and 4096 m ancestors. Each
block owns a contiguous local tree array, and each branch stores child references
in spatial key order. Snapshots share untouched allocations.

The selector uses the published hierarchy and its matching pack until a
compatible result is ready. One hierarchy task and one geometry bake can be in
flight, and obsolete snapshots are retired on a worker. Geometry changes
rebuild all blocks using cached placements. Terrain or model generation changes
and a dirty set exceeding 4,096 keys trigger a complete placement capture.

PlanetScene retains per-tree and per-cell LOD history and the budget controller
across swaps. A stationary view can continue to reuse its selection and uploaded
instance and job buffers after the library's refinement interval. Workers also
collect shadow casters and bake the same opacity field from the matching
hierarchy and geometry.

Set `IGOR_SELECTION_WORKERS` to `1` for the serial reference or to a number from
`2` to `8` for a maximum lane count. Unset or `auto` uses up to eight lanes,
limited by the available CPUs and Bevy compute threads plus the caller.
The planner uses projected subtree sizes to split larger branches below the
root. Small or heavily culled selections stay serial.

The selector assigns contiguous ranges of the original traversal order to
lanes. The main thread runs one lane while Bevy's compute pool runs the others.
One scoped join completes the probe before the budget decision. Lanes borrow
the same snapshot and previous LOD history and write to separate scratch
buffers. Only the accepted probe merges items in traversal order and commits
history updates. The final stable instance ordering, point and job budgets,
and GPU job generation use the existing rules.

The frame still captures patch and portrait references, polls task readiness,
and swaps completed snapshots. Workload partitioning, budget retries, history
commit, and instance and job generation are serial. Shadow caster collection
and geometry baking run on background workers. GPU uploads and render passes
remain in the render world.

Memory overhead comes from reference-counted nodes and the worker's patch-to-cell
index. Per-tree leaf indices and cached per-tree summaries are gone, and the
worker's per-patch placement arrays take the place of the game's previous flat
placement list. During an incremental update the old and new snapshots share
untouched blocks and branches. A full rebuild can temporarily retain two
complete hierarchies.

For Tracy, build with `--features bevy/trace_tracy` and set
`IGOR_POINT_FOREST=1` and `RUST_LOG=world_vegetation=trace,igor_renderer=trace`.
Compare `point_forest.capture_site_batches` and `point_forest.publish_snapshot`
with the worker's `point_forest.build_hierarchy` and its placement and hierarchy
stages. `PlanetHierarchy::incremental_update` and
`PlanetHierarchy::full_rebuild` emit per-build counters for rebuilt, reused, and
removed blocks, updated ancestors, and full rebuilds. The revision messages link
stream changes, scheduling, and publication.

For selection, compare `PlanetScene::partition_workloads`,
`PlanetScene::parallel_traversal`, `PlanetScene::worker_workload`,
`PlanetScene::synchronize_workloads`, and `PlanetScene::merge_workloads`.
The synchronization zone includes the caller's lane. Compare these with
`PlanetScene::traverse_hierarchy` and `PlanetScene::select` across worker counts.

For the LOD quality trial, launch with `IGOR_POINT_FOREST=1` and set
`IGOR_LOD_PIXELS` to `0.75`, `1.0`, `1.5`, or `2.2`. The default is `2.2` px.
The game reads the setting at startup, clamps finite values to `0.25–512` px,
and uses the default for invalid values. Keep `IGOR_POINT_BUDGET` at its default
of 6,000,000 for the threshold comparison. The GPU job limit also depends on
the device storage limit for the padded projection cache. The settlement HUD is
below the FPS readout.

The Igor HUD shows the base and accepted thresholds, selected and raw points,
jobs and job limit, and lane utilization (selected points divided by 64 lanes
per issued job). Probe rejections are counted for the last selection and show
which hard limit was exceeded, or whether a refinement missed the 85% point
headroom target. Set `RUST_LOG=igor_renderer=trace` for individual rejected
probes with thresholds, counts, and limits.

Shadows and legacy undergrowth are enabled by default in point mode. Set IGOR_POINT_SHADOWS=0 or IGOR_POINT_UNDERGROWTH=0 independently. The library bakes local canopy opacity asynchronously from the original 1m LOD; a ground modulation pass samples opaque scene depth before point composition. The shadow field covers a local 512m region, not the entire planet. Undergrowth is the existing renderer for grass, ferns, shrubs and flowers, not yet converted to points. Tree budget stays 6M.

The Bevy adapter keeps the camera's MSAA sample count and samples opaque shadow receivers per sample. Splat lighting uses exposed HDR direct light, a hemispherical sky approximation, and distance haze. Color stays in half-float until it reaches the game's HDR target.

The leaf flutter phase is anchored in planet coordinates. A replacement tree pack preserves LOD history for existing placements, and canopy fields fade over 0.4 seconds. Point colors use unjittered reprojection with depth rejection and neighborhood clipping. Newly uncovered pixels use the current frame immediately.

Opaque depth seeds the existing point depth pass. With MSAA, the farthest opaque sample provides the bound, so a splat can still cover openings at terrain and object edges. The owner pass reuses only projection entries that accepted at least one sample. Set `IGOR_POINT_OCCLUSION=0` to compare opaque-depth seeding with the original depth clear.
