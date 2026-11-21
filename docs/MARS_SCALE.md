# Mars-scale planetary streaming (experimental branch)

This branch changes the reference planet radius from 100 km to **3,389.5 km**.
It is deliberately separate from `master` until the Vulkan gameplay check and
Codex review. It does not regenerate or rewrite the checked-in world assets.

## Terrain pipeline

- The six 1024² baked height faces remain a **coarse, global source** of
  continental shape, existing biome regions, and legacy water masks.
- `SurfaceHeights::sample` adds deterministic 3D, meter-anchored multiscale
  relief (regional mountain ridges, valleys, rolling hills, coastal dunes and meter-scale features). This is the **same**
  height function used by terrain mesh generation, player motion, and forest
  placement; there is no visual-only displacement.
- Detail fades near existing coastlines and river-mask coverage; underwater
  and baked channel shapes are kept intact until the hydrology phase.
- Rendering retains a bounded six-face quadtree, but permits depth 18
  (~1 m near-field grid spacing at the equator), a maximum of 960 resident
  chunks, and only 4 splits / 8 merges per frame. Detail meshes are generated
  **only for requested tiles**. Far tiles are lower LOD and the far side of the
  planet does not receive fine refinement.
- Retired terrain entities now also release their `Assets<Mesh>` handles,
  preventing unbounded GPU/CPU mesh asset accumulation as the camera moves.
- Procedural noise is independent of camera and tile ID: rebuilding the same
  coordinates gives the same height, including at cube-face boundaries.
- The `big_space` floating origin remains in charge of render transforms.

## Landscape / vegetation appearance update

- Physical terrain relief now contains broad 260-km tectonic belts, 58-km
  ridges, 21-km foothills and valleys, plus kilometer-scale terrain detail.
  Raised coastal hills and dunes retain the baked waterline.
- Forests use independent double-precision *physical-space* fields at
  78-km, 8.4-km and 510-m scales: open plains, meadows and clustered groves
  instead of blanket planting. Species and climate thresholds still apply.
- The old terrain shader overlay that painted crowns everywhere regardless
  of spawned trees was disabled. Real terrain is visible in cleared areas.
- Detailed tree meshes remain visible to approximately 165–225 m, and
  authored impostor portraits were increased to 256 px. The 600-tree
  near-detail cap remains a hard bound.
- The original Palm Coast look-at target came from 100-km planting and is
  not a stable marker for one exact new tree on the 3389.5-km sphere.

Performance caveat: the new CPU noise and extra close-range tree geometry
require a real frame-time profile. Exact tree density and visual quality
are provisional until the client has been run and inspected on Vulkan.

## Vegetation

Latitude/longitude patch keys for the much larger radius exceeded `i32`
when multiplying patch indices to derive parent IDs. Those intermediate
products now use `i64`. Forest placement still has the same streaming limits
and biome/species rules; palms and authored trees from the current master
are retained.

## Provisional water compatibility

Existing `river_surface.bin` and `lake_surface.bin` contain positions baked
against the previous **100 km** planet. The runtime loader radially remaps
those mesh vertices onto the Mars radius while retaining their original
elevation above the datum. This is **not** the new hydrology pipeline.

**Known remaining water issues:** ocean patches still have fixed tessellation,
river networks and lake shorelines are still generated at low fixed
resolution, and the old water shader works with very large f32 positions.
These need shoreline-conforming meshes, local water streaming and a
precision-aware water shading path in the subsequent water phase.
Do not regenerate hydrology into shipped `assets/` as a validation shortcut.

## Validation

1. `cargo check --workspace --all-targets`
2. `cargo test --workspace`
3. Run `world-client` on RADV/Vulkan and inspect a surface view, orbit/globe
   view, elevation changes at walking distance, foliage density and frame
   time while flying.
4. Check a revisit to the same area for consistent terrain and no growth in
   `Assets<Mesh>` resident count. Verify adjacent LOD tile boundaries.
5. Only after the above: review and merge the branch with Codex.

The connected GitHub-edit environment cannot execute Bevy or Cargo. The tests
above are **required verification**, not claimed to have passed here.

## Palm Coast visual repair (needs local verification)

- Decreased forest candidates from 112 to 56 per physical 145 m patch,
  lowered acceptance probabilities, and made multi-kilometer meadow and
  grove masks sparser. A 240 m radius authored clearing surrounds the
  Palm Coast explorer start.
- Ground-cover patch resolution is now derived from 25 m physical patches,
  not a fixed 8192 cells per face (which became ~827 m patches on Mars).
  Microplant noise is sampled using double-precision physical coordinates.
- Coastal hills now ramp in from the baked waterline over 10 m elevation;
  near-coast heightfields include 1300 m and 360 m relief, rather than
  staying almost flat wherever the legacy atlas samples ~8 m.
- **Tree appearance is not yet fully fixed.** The imported leaf cutout
  textures, alpha masking, near LOD0 geometry and GPU aliasing still need
  direct screenshot/mesh inspection on Vulkan. The 256px portraits only
  improve billboard LOD, not the quality of close-up imported leaves.
- The first run should verify that the clearing is present, nearby terrain
  varies at visible distances, and tree populations are significantly lower.
  The full Cargo test suite and live client still must run outside this
  GitHub authoring environment.
