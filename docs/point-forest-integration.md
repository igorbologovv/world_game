# External Igor renderer dependency

The game uses Igor_renderer pinned to c88d818b1ebf74594d40be20b2f0e996ea71b590, with default features disabled.
The shared library owns the CPU point baker, template LODs, material atlas,
aggregation and the planetary compute shader source. The game owns Bevy
resource allocation, spherical placement/selection and depth composition.
The duplicated local renderer crate and full compute shader have been removed.
The library assembles the previous planetary shader variant byte-for-byte
from its unchanged standalone shader and host adaptation splices.
No intended visual or selection changes in this dependency migration.
The original planar shadow field still needs spherical adaptation.

Run:
```sh
IGOR_POINT_FOREST=1 cargo run --release -p world-client
```

Look for IGOR LIB in the overlay. Cargo may update Cargo.lock on first run.
No second wgpu device, window or legacy billboard fallback is used.

The game now sends stable world placements and camera parameters to PlanetScene in the pinned renderer library. It no longer selects individual tree LODs, aggregates distant groups or manages threshold hysteresis. The library produces GPU instance/job words. The overlay reports represented sites (including groups), not a precise per-tree frustum count. Original planar shadow adaptation remains separate.
