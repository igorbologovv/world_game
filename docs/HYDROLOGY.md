# Connected planetary hydrology

The offline baker keeps the original terrain seeds and connected ocean. Priority
flood assigns a minimum spill elevation and an acyclic path for every land cell
to the ocean. The spill elevation is routing data. It no longer raises the
terrain throughout rejected depressions.

## Inland water

1. Identify flooded basins on the first watershed graph. Lower eligible spillways
   within the configured depth and sample budgets and reroute on the edited
   terrain. The ocean mask and every ocean elevation sample are preserved.
2. Retain larger, deeper lakes and smaller pools where a selected channel needs
   a standing-water connection. Each retained basin passes its accumulated runoff
   through one downstream outlet. Ordinary headwaters start at the configured
   catchment area.
3. Grade a continuous bed and water level along selected reaches. Match the
   diagonal bed to the terrain's bilinear sampling. At cube seams, connect the
   two source texel centers through the shared edge. Limit river width where a
   bank would hang over dry terrain, and slope junction caps with the reach.
4. Write the river and lake meshes, lake face masks, and combined wet-area
   atlas. The existing water shaders render them. River guide lines remain in
   map mode so they stay out of the free-camera landscape view.

The `WLWATER1` mesh header marks baked terrain positions. Tagged rivers keep
the 30-vertex reach layout. Tagged lakes use six vertices per outward-facing
tile with zero flow. The client also reads untagged meshes and applies the
legacy source-direction alignment to them.

## Bake and tune

Run the generator from the directory that should receive the output under
`assets/`. From the workspace root, this regenerates the packaged world.
Save a separate baseline when comparing profiles.

```sh
cargo run --locked -p world-generator
cargo run --locked -p world-generator -- --help
cargo run --locked -p world-generator -- --river-catchment-km2 65 --spillway-drawdown-m 210
```

| Option | Default | Effect |
| --- | ---: | --- |
| `--river-catchment-km2` | 65 | Contributing area before a regular headwater appears |
| `--spillway-min-area-km2` | 1 | Minimum original basin area to consider for an outlet cut |
| `--spillway-drawdown-m` | 210 | Maximum lowering from the original sill at this planet scale |
| `--spillway-max-cells` | 192 | Cap on cut samples per spillway |
| `--lake-min-area-km2` | 5 | Minimum footprint of a lake without a required throughflow |
| `--lake-min-depth-m` | 8 | Minimum deepest point for an ordinary lake |
| `--river-reference-width-m` | 82 | Nominal full width at a 15 km² catchment |
| `--river-max-width-m` | 275 | Nominal full-width cap |
| `--river-bank-depth-limit-m` | 8 | Bank depth check used to narrow ribbons over dry terrain |

## Before and after

The baseline is a fresh bake of the starting connected-hydrology source.
Its inland meshes and masks matched the starting workspace assets. The
footprint is a cube-sphere area-weighted union of the wet atlas at the
client's alpha cutoff, excluding the ocean mask. Lake area uses the source
grid. River length comes from the rendered centerlines. The count of river
heads excludes lake outlets, and the two invalid baseline grid links are
excluded from length and bed measurements.

| Metric | Before | Refined |
| --- | ---: | ---: |
| Inland wet footprint | 13,384.79 km² | 6,710.90 km² |
| Share of non-ocean surface | 21.66% | 10.86% |
| Lake area | 12,792.63 km² | 6,011.62 km² |
| River heads | 296 | 53 |
| River mouths at the ocean | 332 | 169 |
| Rendered river length | 3,891.77 km | 3,899.79 km |
| River width at the 90th percentile | 278 m | 275 m |
| Highest rendered river source | 3,356 m | 2,712 m |

Inland coverage fell **49.9%**. The rendered network has zero disconnected
reaches and 23 cube-face crossings. The longest refined route to the ocean is
193.35 km, and the 808 retained lakes each have a routed outlet. All seven ocean
PNGs and all 3,195,051 ocean elevation samples match the baseline.

The refined bake was sampled at 388,968 dry-channel centerline points using
the client's terrain alignment and bilinear height lookup. The median depth
was 2.22 m and the maximum was 5.87 m. The baseline had 7,316 samples more than
8 m above the bed. The refined bake had zero. There were 29 refined samples
more than 0.5 m below the terrain. Shorelines, steep reaches, and intersections
still reflect the 1024 × 1024 source grid per face.

The Bevy client rendered an inland river, a cube-face crossing, and a lake
outlet offscreen with Mesa llvmpipe. All 44 workspace tests passed. The
client startup test checks the explorer against the baked terrain, which
samples at 509.39 m at the starter position in both bakes.

To repeat the measurements, use Python with NumPy and Pillow.

```sh
python3 tools/measure_hydrology.py /path/to/baseline/assets --lake-record-vertices 30
python3 tools/measure_hydrology.py assets
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
```
