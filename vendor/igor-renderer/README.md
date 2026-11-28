# Igor hybrid forest

This crate embeds the multi-species `--hybrid-scene` renderer from
[Igor_renderer at `f75f11e`](https://github.com/igorbologovv/Igor_renderer/tree/f75f11e79d3825de1d526034026e8248af8df929).
The embedded shader uses the host's reverse-Z projection and leaves
aerial perspective to the host's depth-based atmosphere pass.

`scene_canopy.rs` retains the upstream Surface Nets field, coverage,
LOD thresholds, ranked hysteresis, and transition ranges. Its build
step is split into shared model templates and streamed spatial regions. The
original static shadow samples and rasterization come from `splat.rs`; the
host supplies the tree placements and local sun direction. Surface Nets
store albedo and shell normals so mesh and canopy LODs use the same diffuse
lighting and shadow texture. Baked shadows fade as the live sun moves away
from their bake direction. The GLB importer
and texture atlas come from `trees.rs` and `tree_texture.rs`.

The host supplies its wgpu device, queue, viewport, projection, and color and
depth attachments. The draw code uses Igor's indexed mesh and canopy
pipelines with the host's sample count.
