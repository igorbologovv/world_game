# Igor renderer library

Imported from `igorbologovv/Igor_renderer` main at
`eea26ccba51c006a6137d45038ae6f59143975cd`.

`geometry.rs` comes from `src/scene_lod_geometry.rs`. It accepts the host's
indexed material parts and alpha sampler in place of the standalone GLB atlas.
`selection.rs` exposes the rank, hysteresis, and reversible phase advancement
from `src/scene_canopy.rs`. The host supplies projected tree and cell sizes.
