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
