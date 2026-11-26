# Original Igor renderer integration (native core)

This branch vendors the CPU source from Igor_renderer commit
9d50c07f471b5ddaf9ee6c50b4e3657df0d387f1 in crates/igor-renderer-core.
The original tree subdivision, 16-byte raw and LOD records, patch and
material layout, texture atlas and 22-level template hierarchy are used
directly. The game's only source adapter provides decoded LOD0 geometry,
textures, and placement. No 78k sample cap, per-template quotas, or silent
color-only material fallback. The original compute shader projection,
coverage, alpha test, early-Z, cache, owner and color functions are ported
with quaternion transforms and a reversed-Z composite for Bevy. The pack
is baked asynchronously for the visible model set; previously baked pack
remains active during rebuild. Device storage limits are reported explicitly.

Run:
```sh
IGOR_POINT_FOREST=1 cargo run --release -p world-client
```
The overlay starts with IGOR NATIVE; UV is the actual raw record count.
The pure point stream still bypasses all tree meshes and billboards.
Existing terrain canopy shading remains active. The demo's fixed-plane
shadow field and world-level cluster hierarchy are not yet adapted to the
planet; the existing terrain/sky/water and undergrowth policy are unchanged.
Original optional Hi-Z/TAA/GPU LOD are not ported. GPU and build validation
are required. The core retains the original CPU demo APIs, but is headless:
no second wgpu device, window or texture upload dependency.
