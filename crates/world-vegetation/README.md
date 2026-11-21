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
  material palette. Shared mesh and material handles allow Bevy 0.14 to batch
  matching detailed instances on supported render backends.
- `impostor` bakes mipmapped portraits from the existing procedural tree
  meshes and reuses the exported Hero Oak silhouette. `proxy` merges one
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
