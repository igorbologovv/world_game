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

The terrain plugin renders drifting cumulus between 1,650 and 3,850 meters
above the reference sphere. A generated 3D noise texture shapes the cloud
fields in planet coordinates, so they stay in place across floating origin
shifts. The shader samples cloud density along each view ray and toward the
terrain sun, giving the same clouds shaded undersides from the surface and
lit tops from orbit.
