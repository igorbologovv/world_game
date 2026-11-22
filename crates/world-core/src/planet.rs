//! Reference sphere, generated asset formats, and region identifiers.

use bevy_math::DVec3;

/// Radius of the planetary reference sphere in meters (Mars mean radius).
///
/// Terrain is sampled through a multiresolution height provider; the 1024²
/// source maps only describe the global shape, not local surface detail.
pub const RADIUS: f64 = 3_389_500.0;

/// Radius in meters used when the currently checked-in binary river/lake
/// positions were baked. These files are migrated in memory until the new
/// hydrology baker produces assets at the current planet radius.
pub const LEGACY_BAKED_RADIUS: f64 = 100_000.0;
/// Datum used by the generator to classify land and bodies of water.
pub const SEA_LEVEL: f64 = 0.0;
/// Runtime water surface elevation in meters above the reference sphere.
pub const WATER_SURFACE_ELEVATION: f64 = 1.1;
/// Lowest representable height in legacy heightmaps.
pub const MIN_HEIGHT: f64 = -12_000.0;
/// Highest representable height in legacy heightmaps.
pub const MAX_HEIGHT: f64 = 9_000.0;
/// Side length of each generated TIFF height face in pixels.
pub const HEIGHT_MAP_SIZE: u32 = 1024;
/// Heightmap directory relative to the game assets.
pub const HEIGHT_SOURCE_PATH: &str = "terrains/spherical/source/height";
/// River mask path relative to the game assets.
pub const RIVER_ATLAS_PATH: &str = "world/river_atlas.png";
/// Ocean and inland basin mask path relative to the game assets.
pub const OCEAN_ATLAS_PATH: &str = "world/ocean_atlas.png";
/// Directory containing the baked inland lake mask for each cube face.
pub const LAKE_MASK_PATH: &str = "world/basins";
/// River mesh path relative to the game assets.
pub const RIVER_SURFACE_PATH: &str = "world/river_surface.bin";
/// Generated inland lake meshes at their individual spill elevations.
pub const LAKE_SURFACE_PATH: &str = "world/lake_surface.bin";
/// Inland-water mesh magic for vertices already aligned to the baked terrain.
pub const BAKED_WATER_MESH_HEADER: &[u8; 8] = b"WLWATER1";
/// Number of unindexed vertices per reach in the legacy river mesh.
pub const RIVER_REACH_VERTICES: usize = 30;

/// Encode elevation in meters into a legacy heightmap sample.
pub fn encode_height(height: f64) -> u16 {
    let height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    let normalized = (height - MIN_HEIGHT) / (MAX_HEIGHT - MIN_HEIGHT);
    (normalized * f64::from(u16::MAX)).round() as u16
}

/// Decode a legacy heightmap sample or interpolated sample into meters.
pub fn decode_height(encoded: f64) -> f64 {
    MIN_HEIGHT + encoded / f64::from(u16::MAX) * (MAX_HEIGHT - MIN_HEIGHT)
}

/// Stable identifier for an authored planetary region.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegionId(pub &'static str);

/// Initial explorer placement specified in generated source coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ExplorerSpawn {
    /// Radial direction of the spawn before baked terrain alignment.
    pub direction: DVec3,
    /// Radial direction used to establish the explorer's starting heading.
    pub look_at: DVec3,
    /// Initial elevation in meters, until the runtime samples the local terrain.
    pub elevation: f64,
}
