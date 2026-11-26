# Experimental Igor point forest bridge

Base: feature/farm-gameplay, retaining existing terrain, water, farm and
canopy streaming. Standalone reference: Igor_renderer 9d50c07.

Run from repository root:
```sh
cargo test -p world-vegetation
IGOR_POINT_FOREST=1 cargo run --release -p world-client -- --spawn starter-river
```

Without the environment variable, the original path is unchanged.
The opt-in path uses single-sample HDR (MSAA disabled for the world
camera). It renders fully resident individually streamed trees only;
hero oak, missing models, partially transitioned trees, budget
overflows and distant impostors retain their original mesh path.

The existing impostor source provides decoded geometry, UVs and alpha
cutout/material images. The first bridge bakes alpha-tested point
samples and normals/colors, then builds five local detail ranges.
Unlike the standalone renderer, it does not yet retain per-splat
source UV/patch references: color and normal are baked at sample
centers. This is intentionally a first integration, not pixel-identical
to the standalone 22-level renderer. Shared geometry records occupy
16 bytes each; coordinates are signed 1/256 m, radius 1/4096 m.
A hard 3.5M selected point cap (override IGOR_POINT_BUDGET) and 5M
resident record cap bound the experimental work. If the cap is reached
the relevant tree keeps its original mesh. The buffer uploads only
when a shared model is first baked, not on camera movement. Instance
positions are camera-relative, computed in f64 on the CPU; rotation is
the existing spherical-tree quaternion.

Three compute passes project oriented disks, publish reverse-Z depth
and owner, then shade the winning sample. The fullscreen composite
writes real Bevy depth and color after opaque geometry and before
transparent geometry. No second window/device, ray traversal or
polygonal tree draw in the point path. The wind moves the 3D disks;
the existing planet canopy shadow field remains active on terrain,
with inexpensive crown interior shading in the point resolve. The
standalone fixed-sun height-band shadow map is not ported in this step.
No sound, TAA, projection-cache or GPU LOD port yet. Startup shader
compilation and unsupported view configurations fall back to meshes.
The first implementation needs compilation and visual/GPU validation.

## Verify activation

The bottom-right IGOR overlay reports pipeline status, selected trees/points,
eligible vs resident detailed trees, baked models, and GPU-submitted trees.
Set `IGOR_POINT_DEBUG=1` along with `IGOR_POINT_FOREST=1` to tint only
compute-rendered pixels magenta. Mesh/impostor fallbacks retain normal colors.
The terminal logs pipeline failures and unsupported view configurations.

## Expanded point coverage

The authored model sources are retained for point baking instead of consumed
by the impostor atlas task. All eligible detailed trees, including hero oak
and partial dither transitions, can use compute. Missing library assets
share the same procedural variant fallback as the mesh renderer. The near
impostor transition is extended to 145–190 m at reference resolution, and
up to 900 detailed trees may be streamed (rather than 300). The CPU admits
nearer trees first and chooses from 22 nominal spatial widths based on focal
length, distance, scale and point budget. Per-template memory caps alias
some widths to shared coarser ranges. The projection cache reuses tiny
pixel/depth and larger bounding rectangles between compute passes.
Terrain, water, buildings and distant canopy retain their existing render
paths. Full per-pixel source texture mapping, standalone light-space
shadow map and exact standalone LOD remain future work.

## Pure point forest mode

With IGOR_POINT_FOREST=1, canonical placement discovery is the only part of
legacy forest streaming that runs. Neither atlas construction, impostor/parent
mesh baking, billboard uploads nor individual mesh-tree spawning runs. There
is no silent polygon fallback. The point pass selects visible sites directly
from streamed patch arrays out to the existing forest range (default 2 km),
with conservative frustum/horizon checks and a global projected-size threshold
that coarsens the whole visible forest to fit the 3.5M point budget. The finest
and coarsest template levels are shared across instances; very distant trees
fade into the existing terrain canopy color field. This is per-tree coarse
LOD, not yet the standalone world-level merged cluster hierarchy.

Legacy fern/card and ground-cover rendering is disabled in this mode for a
clean forest comparison. IGOR_POINT_UNDERGROWTH=1 explicitly re-enables it.
The original mode remains available by omitting IGOR_POINT_FOREST. The palm
label requires a legacy entity and is absent in pure mode. While models or GPU
pipelines load the point forest may be temporarily empty. The standalone
per-pixel UV/alpha material system and directional shadow field are still not
fully ported; terrain canopy shading remains enabled. GPU testing required.
