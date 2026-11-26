# External Igor renderer dependency

The game uses Igor_renderer pinned to d552eb12791240c14b939fa53afe76632304266b, with default features disabled.
The shared library owns the CPU point baker, template LODs, material atlas,
aggregation and the planetary compute shader source. The game owns Bevy
resource allocation, spherical placement/selection and depth composition.
The duplicated local renderer crate and full compute shader have been removed.
The library assembles the previous planetary shader variant byte-for-byte
from its unchanged standalone shader and host adaptation splices.

Run:
```sh
IGOR_POINT_FOREST=1 cargo run --release -p world-client
```

Look for IGOR HIERARCHY in the overlay.
No second wgpu device, window or legacy billboard fallback is used.

The game refreshes placements when sampled terrain patches or model sources change. PlanetScene owns tree LOD, aggregation, and threshold hysteresis. After the library's refinement interval, a stationary view reuses the selection and its uploaded GPU instance and job buffers. The overlay counts represented sites, including aggregated groups.

Quality trial: game default is 6,000,000 selected points per frame (override with IGOR_POINT_BUDGET). The original 22 LOD levels and selection policy remain unchanged. The job limit also respects the actual device storage limit for the padded projection cache. The settlement HUD is below the FPS readout.

Shadows and legacy undergrowth are enabled by default in point mode. Set IGOR_POINT_SHADOWS=0 or IGOR_POINT_UNDERGROWTH=0 independently. The library bakes local canopy opacity asynchronously from the original 1m LOD; a ground modulation pass samples opaque scene depth before point composition. The shadow field covers a local 512m region, not the entire planet. Undergrowth is the existing renderer for grass, ferns, shrubs and flowers, not yet converted to points. Tree budget stays 6M.

The Bevy adapter keeps the camera's MSAA sample count and samples opaque shadow receivers per sample. Splat lighting uses exposed HDR direct light, a hemispherical sky approximation, and distance haze. Color stays in half-float until it reaches the game's HDR target.

The leaf flutter phase is anchored in planet coordinates. A replacement tree pack preserves LOD history for existing placements, and canopy fields fade over 0.4 seconds. Point colors use unjittered reprojection with depth rejection and neighborhood clipping. Newly uncovered pixels use the current frame immediately.

Opaque depth seeds the existing point depth pass. With MSAA, the farthest opaque sample provides the bound, so a splat can still cover openings at terrain and object edges. The owner pass reuses only projection entries that accepted at least one sample. Set `IGOR_POINT_OCCLUSION=0` to compare opaque-depth seeding with the original depth clear.
