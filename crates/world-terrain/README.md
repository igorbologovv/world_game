# World terrain

## Biome surface materials

The terrain uses two 512 × 512 variants per material family. The shader derives
biome and prospective vegetation cover from the same temperature, rainfall,
elevation, and grove fields used by tree placement. It combines those fields
with terrain grade and local patch noise. River coverage, baked lake masks,
streamed pool basins, and nearby sea-level terrain supply the water influence.
Dry forest litter gives way to damp leaves, moss, peat, and mud as soil
moisture rises. Above the tree line and on steep slopes, talus and bedrock
gain coverage.

The texture arrays contain sRGB albedo and linear height in one RGBA texture
and OpenGL normals and perceptual roughness in the other. They have complete
mip chains and occupy about 91 MiB of GPU texture memory. The shader chooses
up to three layers per pixel and uses height to bias their existing overlap.
Height also adds a small amount of ambient occlusion in recesses. The sampled
normal, roughness, and color all use the same material weights.

The renderer uses triplanar projection with 2 m and 8 m periods for most
materials and 4 m and 16 m periods for bedrock. Planet-space variation shifts
texture phase and rotates paired variants. Vertex coordinates retain
sub-meter precision by subtracting a shared-period grid origin in double
precision before interpolation. Neighboring chunk origins differ by whole
texture periods. The material fades by pixel footprint and viewing distance
between 310 m and 850 m into the original planetary shading. Existing terrain
draw submissions carry both shading paths. The original detail textures also
provide the fallback while biome assets load or if either array is unavailable.

### Texture sources and licenses

All input maps are distributed under CC0 1.0. Entries with two names in one
cell are composites. Humus, dry moss, peat, wet litter, wet mud, and alpine
variants have additional albedo or roughness treatment in the baker.
[Poly Haven license](https://polyhaven.com/license) · [ambientCG license](https://docs.ambientcg.com/license/) · [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/)

| Layers | Material | First variant | Second variant |
| --- | --- | --- | --- |
| 0–1 | Forest soil and humus | [Forest Ground 06](https://polyhaven.com/a/forest_ground_06) | [Dirt Floor](https://polyhaven.com/a/dirt_floor) |
| 2–3 | Dry fallen leaves | [Forest Leaves 04](https://polyhaven.com/a/forest_leaves_04) | [Dry Decay Leaves](https://polyhaven.com/a/dry_decay_leaves) |
| 4–5 | Wet fallen leaves | [Brown Mud Leaves 01](https://polyhaven.com/a/brown_mud_leaves_01) | [Mud Forest](https://polyhaven.com/a/mud_forest) |
| 6–7 | Pine needles and conifer litter | [Forest Ground 03](https://polyhaven.com/a/forrest_ground_03) | [Forest Sand 01](https://polyhaven.com/a/forrest_sand_01) |
| 8–9 | Green moss | [Moss 001](https://ambientcg.com/view?id=Moss001) | [Moss 002](https://ambientcg.com/view?id=Moss002) |
| 10–11 | Dry moss | [Moss 001](https://ambientcg.com/view?id=Moss001), [Forest Ground 01](https://polyhaven.com/a/forrest_ground_01) | [Moss 002](https://ambientcg.com/view?id=Moss002), [Withered Grass](https://polyhaven.com/a/withered_grass) |
| 12–13 | Grass-covered soil | [Sparse Grass](https://polyhaven.com/a/sparse_grass) | [Leafy Grass](https://polyhaven.com/a/leafy_grass) |
| 14–15 | Roots and organic debris | [Roots](https://polyhaven.com/a/roots) | [Wood Chip Path](https://polyhaven.com/a/wood_chip_path) |
| 16–17 | Peat and swamp soil | [Forest Ground 05](https://polyhaven.com/a/forest_ground_05), [Mud Forest](https://polyhaven.com/a/mud_forest) | [Brown Mud](https://polyhaven.com/a/brown_mud), [Brown Mud 02](https://polyhaven.com/a/brown_mud_02) |
| 18–19 | Wet mud | [Brown Mud 03](https://polyhaven.com/a/brown_mud_03) | [Ground 094 B](https://ambientcg.com/view?id=Ground094B) |
| 20–21 | Dry cracked mud | [Mud Cracked Dry 03](https://polyhaven.com/a/mud_cracked_dry_03) | [Mud Cracked Dry Riverbed 002](https://polyhaven.com/a/mud_cracked_dry_riverbed_002) |
| 22–23 | Gravel and small stones | [Rocky Gravel](https://polyhaven.com/a/rocky_gravel) | [Gravel Stones](https://polyhaven.com/a/gravel_stones) |
| 24–25 | Sand | [Sand 01](https://polyhaven.com/a/sand_01) | [Dense Sand](https://polyhaven.com/a/dense_sand) |
| 26–27 | Clay-rich soil | [Cracked Red Ground](https://polyhaven.com/a/cracked_red_ground) | [Red Mud Stones](https://polyhaven.com/a/red_mud_stones) |
| 28–29 | Exposed mountain rock | [Dark Rock](https://polyhaven.com/a/dark_rock) | [Dark Rock 02](https://polyhaven.com/a/dark_rock_02) |
| 30–31 | Alpine ground | [Rocky Trail 02](https://polyhaven.com/a/rocky_trail_02) | [Rocky Trail](https://polyhaven.com/a/rocky_trail), [Withered Grass](https://polyhaven.com/a/withered_grass) |
| 32–33 | Coastal sediment | [Damp Beach Sand 02](https://polyhaven.com/a/damp_beach_sand_02) | [Coral Mud 01](https://polyhaven.com/a/coral_mud_01) |

To reproduce the arrays with Python, NumPy, ImageMagick 7, and zstd:

```sh
python3 tools/build_biome_surface_textures.py /tmp/terrain-source-maps --fetch
```

The baker can rebuild offline from the cached maps. The separate
`tools/build_surface_textures.py` continues to build the original detail
arrays and Hero Oak bark from the checked-in source packs.

### GPU validation

The ignored client sweep captures dense forest, conifer forest, wet lowland,
mountain slope, grassland, and coast at ground level and at 62 m above ground.
A 4.8 km view covers the handoff to the original material. Both runs use the
same camera poses, paused sun, and clear skies. The log reports the median
GPU opaque pass time and synchronized frame p50 and p95 for each view.

```sh
WORLD_TERRAIN_LEGACY_DETAIL=1 TERRAIN_CAPTURE_DIR="$PWD/target/terrain-materials/baseline" \
  cargo test --locked -p world-client terrain_material_sweep -- --ignored --nocapture
TERRAIN_CAPTURE_DIR="$PWD/target/terrain-materials/biome" \
  cargo test --locked -p world-client terrain_material_sweep -- --ignored --nocapture
```

Set `TERRAIN_CAPTURE_CASE` to a comma-separated list of scene names to
repeat selected views. The [capture index](../../target/terrain-materials/captures.md)
links to the 1280 × 720 PNGs and the original timings for both runs.

Measured on a Radeon RX 6750 XT with RADV/Vulkan. Times are median opaque-pass
GPU milliseconds for 48 measured frames per view after a 160-frame warmup.
The opaque pass includes vegetation. The mountain and coast used a focused
repeat with the mountain camera below the snow line and the coastal camera
on the tidal flat. The distant PNGs matched byte for byte.

| Scene | Legacy ground | Biome ground | Legacy elevated | Biome elevated |
| --- | ---: | ---: | ---: | ---: |
| Dense forest | 12.36 | 15.64 | 21.28 | 22.73 |
| Conifer forest | 10.62 | 11.66 | 16.32 | 17.07 |
| Wet lowland | 9.83 | 12.16 | 12.93 | 14.54 |
| Mountain slope | 12.71 | 14.17 | 7.78 | 7.83 |
| Open grassland | 9.01 | 10.98 | 7.98 | 9.38 |
| Coastline | 4.64 | 6.56 | 4.62 | 8.71 |
| Distant aerial, 4.8 km above ground | | | 3.10 | 3.67 |

Across the 12 ground and elevated views, the mean opaque-pass median rose
from 10.84 to 12.62 ms. The mean synchronized frame median rose from 32.66 to
34.53 ms. The elevated tidal flat had the largest opaque-pass increase at
4.10 ms.

## Local relief

The shared height sampler layers meter-scale bumps, rolling hills, gullies,
outcrops, dunes, and shallow wetland basins over the baked terrain and mountain
relief. Latitude, regional rainfall, elevation, and river proximity blend the
shapes gradually. The new relief fades at the shoreline and around baked
channels and lakes. Baked heights and water coverage blend at cube-face edges.
Explorer ground contact and vegetation placement use the same meter-scale height
samples as the terrain mesh.

The quadtree prioritizes fine tiles around a low camera and reaches roughly
one-meter vertex spacing at its finest level. The water plugin streams pools
in nearby wetland depressions where the sampled ground forms a closed rim.
Pools use the same terrain heights for their shoreline and are shallow enough
to walk through.

## Clouds

On the experimental branch, **F6** cycles through volumetric clouds, the
legacy renderer, and clear skies. The branch starts in volumetric mode.

The ray march clips against two spherical decks, terrain depth, and the ocean
radius. The liquid deck spans 1,450–4,650 m above the reference sphere and the
ice deck spans 4,950–7,900 m. The weather cubemap controls fronts and where the
liquid deck forms shallow stratocumulus or taller cumulus. Wind shear draws
the upper ice into cirrus filaments. The repeating 3D Worley and fractal
value-noise field packs the cloud mass, billows, edge erosion, and interior
density in one texture. Distorted fine erosion breaks up the billow edges,
and the density varies continuously from thin pockets to opaque cores. Both
fields and their mip chains are generated at startup.

The integrator uses Beer–Lambert extinction and density samples along the
sunlight path for self-shadowing. A narrow Henyey–Greenstein forward lobe
lights the thin edges facing the sun, while broader lobes and approximate
multiple scattering soften interior shadows. Bevy's atmosphere transmittance
LUT filters the sunlight. The view march reuses lighting over neighboring
samples with similar density, skips the gap between the decks and clear space
above local cloud tops, and stops once transmittance falls below 0.8%. The
projected footprint controls step sizes and noise mips.

The renderer marches at half width and height. It reprojects the previous
frame at the cloud's opacity-weighted depth, accounting for camera movement
and wind. Depth rejection and neighborhood clamping limit trails, and history
resets on a camera cut, target change, or renderer switch. Camera deltas are
computed in the planet's double-precision grid before going to the shader.
The final HDR pass uses the full-resolution depth buffer to preserve
foreground edges.

The client sweep uses the gameplay renderer and Palm Coast assets. It saves
matched PNGs in all three modes at orbital, high-altitude, above-cloud,
cloud-layer, ground-level, and backlit viewpoints. It also captures a flyover,
a camera cut, and a 12 km horizontal transect through the liquid deck.
It reports median GPU timestamp measurements per pass. The legacy renderer shares the transparent pass with
water, so estimate its cloud cost by subtracting the clear-mode transparent
pass median. The default output is `target/cloud-volumetric` at 1024 × 576.

```sh
cargo test --locked -p world-client cloud_altitude_sweep -- --ignored --nocapture

CLOUD_CAPTURE_SIZE=1920x1080 CLOUD_CAPTURE_DIR=target/cloud-volumetric-1080 \
  cargo test --locked -p world-client cloud_altitude_sweep -- --ignored --nocapture
```

Measured at 1920 × 1080 on a Radeon RX 6750 XT with RADV/Vulkan. Figures are
milliseconds and sum the median timestamps of the three volumetric passes.
The reference is commit `9e4e290`. The mean gives each matched pose equal weight.

| View | Previous volumetric | Updated volumetric |
| --- | ---: | ---: |
| Orbit | 1.80 | 1.69 |
| 34 km | 2.15 | 2.08 |
| Above cloud, 9 km | 3.76 | 3.19 |
| Cloud-layer edge, 2.35 km | 4.95 | 4.98 |
| Below cloud, 150 m | 4.98 | 4.37 |
| Palm Coast shoreline | 3.18 | 2.57 |
| Backlit view | 3.26 | 3.12 |
| Cloud core, 2.1 km | 7.23 | 5.90 |
| Mean across views | 3.91 | 3.49 |

## Celestial clock

**F2** pauses and resumes the celestial clock. **F3** cycles through 1×, 60×,
600× and 3,600× speed, with the current setting logged on each change.
The clock starts at 1× in daylight at Palm Coast and runs independently
of player movement and weather animation.

The planet spins every 24 hours (sidereal rotation) and follows a circular
365.25-day orbit. Its axis is tilted 23.5 degrees from the orbital normal,
producing seasonal changes in solar declination. The epoch is northern
summer solstice. The two moons follow independent prograde circular orbits
with periods of 4 and 11 days and inclinations of 5 and 12 degrees relative
to the planet's orbital plane. The second orbit has a different line of
nodes. Positions are evaluated analytically in double precision. These
are prescribed orbits, not a gravitational stability simulation.
The gameplay grid stays planet-fixed. The sky rotates into that frame, and
moon meshes use the same floating-origin grid as the camera.

The existing atmosphere and clouds receive the moving sun. Each moon has a
sunlit phase and supplies directional moonlight scaled by that phase. Its
intensity is tuned for the fixed gameplay exposure. The procedural star
cubemap rotates with the sky, becoming visible after twilight and in space.

```sh
CLOUD_CASE=celestial CLOUD_CAPTURE_DIR=target/celestial-cycle \\
  cargo test --locked -p world-client cloud_altitude_sweep -- --ignored --nocapture
```
