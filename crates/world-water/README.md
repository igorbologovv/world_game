# Planetary hydrology

The world generator uses a priority flood across all six cube faces to find
connected drainage and basin spill elevations. Water takes the steepest
available downhill outlet. Flats follow the flood path toward their sill,
so downstream traversal remains acyclic. Large basins can spill through a
bounded cut. Retained lakes have a single routed outlet and channels follow
contributing catchment area in square kilometers.

The runtime shares the baked reaches and lake stages through
`world_core::SurfaceHeights`. Its channel profile adds meanders, gravel bars,
shallow toes and floodplains to the global heights. Sediment shelves near lake
and ocean stages add shore detail in meters. Wetlands form deterministic pools
whose water level stays below the measured rim. Vegetation uses the open-water
query to keep trees and upland plants out of active channels.

The water renderer refines spherical inland patches near the camera and
streams coarser geometry toward the horizon. It keeps the prior patch until
its replacement mesh is ready. Asset and terrain positions use double
precision until conversion to patch-local coordinates, which keeps ripples
and shorelines stable through floating-origin shifts. Rivers advect ripples
and foam with their flow. Lakes and marshes have calmer, separately tinted
water. The water shades real terrain through the shallows. The ocean blends
its local spherical surface into the coarse shell outside the shoreline view.

Walk into shallow water to wade or use **WASD** to swim in deeper water.
A river current moves the explorer downstream while swimming. Route travel
can cross shallows and stops at deep water. **F** uses the local surface
elevation for takeoff and clearance.

## Rebuilding and validating

A bake updates the binary meshes, atlas, lake masks and graded height faces
together. Generate into a scratch directory first to inspect the result.
The legacy `WLWATER1` record layout is preserved, and the reader also
supports older water meshes authored against the 100 km reference sphere.

```sh
cargo build --release --locked -p world-generator
generator="$PWD/target/release/generate_world"
scratch="$(mktemp -d)"
(cd "$scratch" && "$generator")
python tools/measure_hydrology.py "$scratch/assets"
```

After installing the complete set of outputs, rebuild the [orbital canopy](../world-vegetation/README.md#orbital-canopy) against the new terrain.
The freshwater material uses a generated 256 × 256 tileable slope, foam and
sediment texture. Rebuild it with NumPy and zstd using
`python tools/build_freshwater_texture.py`.

Run the GPU capture sweep to review the bank, shore, marsh and surf:

```sh
TERRAIN_CAPTURE_CASE=river-bank,lake-shore,wet-lowland,coastline \
TERRAIN_CAPTURE_DIR=target/hydrology-captures \
cargo test --locked -p world-client terrain_material_sweep -- --ignored --nocapture
```

## Ocean storms

The ocean starts calm. Press **F7** to start a storm centered on the world camera's current location. The storm stays fixed to the planet as the camera moves. Press **F7** again to ease it out. Both fades take three seconds. After it clears, pressing **F7** starts a new storm at the camera's new location.

The storm has a 22 km core and blends into calm water by 65 km from its center. It increases the height and roughness of the existing swells, adds a longer swell band, and strengthens breakers and foam over shallow shelves. Wind-driven mist is shaded along the sight line immediately above the water. Storm effects use the existing ocean mesh, bathymetry, and texture set.
