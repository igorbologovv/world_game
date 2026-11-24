# World terrain

The spherical terrain material uses planet-space triplanar projection. Its
close-range layers add gravelly sand to coasts and bare soil, rocky terrain to
meadows and exposed ground, lichen rock to outcrops and cliffs, and frosted ice
to patches of snow. The shader blends those layers by slope, biome weight, and
pixel footprint. It applies the photos' local contrast around the original
biome colors so a change in viewing distance leaves the landscape palette
stable. The base terrain also renders while the detail pair loads or if the
pair is missing or incompatible.

The shipped detail arrays have four 512 × 512 layers, in the order above.
Albedo is sRGB. The linear companion array packs the OpenGL tangent-space
normal in RGB and perceptual roughness in alpha. The triplanar shader accounts
for the OpenGL normal's upward green axis when sampling textures with a
downward V axis. Both arrays include mips down to 1 × 1. The terrain mesh
supplies the surface shape.

To rebuild both terrain arrays and the Hero Oak bark maps, run the material
baker with NumPy, ImageMagick 7 with OpenEXR support, and zstd installed.

```sh
python3 tools/build_surface_textures.py
```

The source packs are `gravelly_sand_2k.blend/`,
`rocky_terrain_02_4k.blend/`, `lichen_rock_4k.blend/`,
`Ice003_1K-JPG/`, and `jolcham_oak_bark_01_4k.blend/` at the checkout
root. Use `--source-root` when those packs are in a separate directory.

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
