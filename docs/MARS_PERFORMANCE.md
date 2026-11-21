# Mars-scale performance and rendering

## Reproduction

```sh
cargo check --workspace --all-targets
cargo test --workspace
WGPU_BACKEND=vulkan cargo test --locked -p world-client mars_frame_times -- --ignored --nocapture
```

The ignored benchmark requires Vulkan timestamp queries. Set `MARS_BENCH_CSV`
to a destination file for per-frame results. Set `MARS_BENCH_SCREENSHOT` to a
file prefix for captures at four points along the route and a settled view of
the Palm Coast spawn.

The comparison uses the workspace's default test profile, a Ryzen 7 5700X,
and a Radeon RX 6750 XT with RADV (Mesa 26.2.5). The camera renders to a
960 × 540 texture with a fixed 20 ms simulation step. The benchmark records
60 startup frames at Palm Coast and 300 travel frames, moving along the
coast, jumping to the Starter River highlands, and revisiting the coast.
App update time includes CPU-side render submission. Wall time also waits
for GPU completion each frame. GPU timings sum Bevy's reported leaf pass
spans. The route exercises streaming and shading without presentation,
interactive input, or the on-screen rendering pipeline's frame overlap.

## Results

Baseline commit `764d060` ran the same route with benchmark instrumentation
and the original terrain and vegetation code. Times are milliseconds.

| Measurement | Before P50 | After P50 | Before P95 | After P95 | Before P99 | After P99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Travel wall time | 160.74 | 25.92 | 204.84 | 30.54 | 221.15 | 40.27 |
| Travel app update | 149.47 | 16.11 | 193.85 | 20.64 | 210.48 | 30.02 |
| Travel GPU pass sum | 9.35 | 7.75 | 10.62 | 8.25 | 11.38 | 8.99 |
| Startup wall time | 100.28 | 8.16 | 156.81 | 24.15 | 9,752.66 | 1,124.83 |

A second optimized run measured travel wall P95/P99 of 30.33/34.10 ms and
GPU pass P95/P99 of 8.81/9.05 ms. The two optimized runs peaked at 937 and
1,094 main-world mesh asset entries, down from 1,194. Process RSS
high-water marks were 883,544 and 869,544 KiB, down from 1,046,276 KiB.
Worker completion timing changes peak residency between runs. These
measurements cover the scripted route and exclude direct measurement of
GPU allocator residency.

### CPU and streaming

A 149 Hz `perf` CPU-clock profile on the baseline collected 9,123 samples.
Procedural value noise and its lattice hash used 30.4% of sampled CPU time.
Nested inclusive-range iterator code in the same sampler used about 26%.
The terrain system was building split/merge meshes on the frame thread,
including four extra height evaluations per vertex for normals. Merge scans
also repeatedly sampled parent elevations. Inland water mesh initialization
sampled terrain for every vertex of the baked triangle stream, including
repeated lake vertices. Forest placement and authored tree portraits also
ran synchronously.

- Terrain keeps depth 18, its 20 × 20 tile grid, the 960-leaf cap, and
  double-precision planet coordinates. Six pending jobs share the immutable
  height rasters. Each frame submits up to eight prepared meshes. The
  outgoing tiles stay visible until the replacement set is ready, and
  superseded jobs and retired assets are released.
- Height sampling reuses the cube-face projection, skips zero-weight relief
  bands, and evaluates the same eight lattice corners in the original
  order. Coarse terrain normals reuse neighboring vertex samples, with a
  consistent footprint at tile edges. Mountain, coastline, and
  meter-scale positioning still use the shared deterministic heightfield.
- Rivers, indexed lake preparation, and ocean generation run on workers.
  Ocean submission is capped at 12 patches per frame, and immutable water
  meshes release CPU-side geometry after upload. Lake indexing samples each
  distinct baked position once.
- Forest placement keeps the existing node, detail entity, and upload
  budgets. It now allows up to 12 placement jobs and prioritizes nearby
  impostor batches. Tree positions use continuous patch coordinates. The
  previous baked texel remap could move trees well outside their own patch
  on the Mars sphere. Fern placement also uses physical-size cells. Seeds,
  biome selection, and climate thresholds are preserved. Individual sites
  move as a result of correcting their coordinates.

### Rendering and imported trees

The shipped prefabs each contain bark and foliage geometry for three LODs.
The Bevy import test loads all four prefabs, checks triangle counts and
mip levels, and verifies that their generated impostor portraits contain
the imported cutouts.

| GLB path under `assets/models/vegetation` | In-game variant | Leaf mask cutoff | Texture inputs |
| --- | --- | ---: | --- |
| `coconut_palm/tree_lod{0,1,2}.glb` | Coconut palm | 0.258 | Bark and leaves use base color, normals, and metallic/roughness |
| `tree_part9/tree_lod{0,1,2}.glb` | Tropical broadleaf | 0.70 | Bark base color and normals, plus leaf base color with alpha |
| `woodland/woodland_tree_00_lod{0,1,2}.glb` | Woodland broadleaf 00 | 0.42 | Bark base color, normals, and metallic/roughness, plus leaf base color and normals |
| `woodland/woodland_tree_01_lod{0,1,2}.glb` | Woodland broadleaf 01 | 0.42 | Same texture channels as woodland broadleaf 00 |

The foliage path preserves GLB mask thresholds and PBR inputs, creates
coverage-preserving alpha mipmaps, and uses anisotropic filtering. Authored
portraits and mipmaps are prepared one prefab at a time. The detailed mesh
now holds farther at small viewport sizes. Complementary dither blends it
into the batched portrait, with species-sized portrait framing to avoid
cropping wide crowns. The closest 24 trees within the local shadow range
can cast alpha-tested shadows, and terrain and near canopies receive
them. The Palm Coast guide follows a streamed palm instance.

The largest timed GPU passes were light-probe irradiance, the opaque pass,
atmosphere LUTs, and light-probe radiance. Reducing the atmosphere
environment probe from a 128-pixel face to 64 reduced average irradiance
from 2.75 to 1.82 ms and radiance from 1.13 to 0.86 ms. The terrain mip
gradients now use camera-relative render coordinates, while terrain layer
placement stays anchored to the planet across floating-origin shifts.
Direct transfer timing was outside the GPU pass measurements. Uploads
are bounded by the terrain, water, and forest submission budgets.

## Remaining limits

The first frame still includes synchronous height raster loading and
initial asset work. The startup capture can precede visible local terrain
and tree pipelines. The timed coast and highland captures include rendered
terrain, foliage, and tree shadows. The settled spawn capture shows the
coconut palm and the guide at the clearing. Distant impostors use one view
per variant, and pine and acacia still use procedural geometry near the
camera.
The baked water compatibility and precision limitations in
[Mars-scale planetary streaming](MARS_SCALE.md#provisional-water-compatibility)
still apply. These results are for one scripted route and one GPU.
