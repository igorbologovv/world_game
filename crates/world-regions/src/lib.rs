#![warn(missing_docs)]
//! Authored planetary region definitions shared by clients and tooling.

use bevy_math::DVec3;
use world_core::planet::{ExplorerSpawn, RegionId};

/// A named region with an initial explorer placement.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    /// Stable authored identifier.
    pub id: RegionId,
    /// Starting position and orientation in source terrain coordinates.
    pub spawn: ExplorerSpawn,
}

/// The original explorer starting region beside the river.
pub const STARTER_REGION: Region = Region {
    id: RegionId("starter-river"),
    spawn: ExplorerSpawn {
        direction: DVec3::new(-0.35249159, 0.82882267, 0.43451429),
        look_at: DVec3::new(-0.35203830, 0.82923209, 0.43410042),
        elevation: 494.13,
    },
};

/// Sunlit coastal starting point facing a streamed coconut palm grove.
pub const PALM_COAST_REGION: Region = Region {
    id: RegionId("palm-coast"),
    spawn: ExplorerSpawn {
        direction: DVec3::new(-0.97684968336687, 0.21339114535822, 0.01512994344821),
        look_at: DVec3::new(-0.97687041136832, 0.21331238468388, 0.01490053467382),
        elevation: 8.14831769,
    },
};

#[cfg(test)]
mod tests {
    use world_core::{coordinates::terrain_aligned_direction, planet::WATER_SURFACE_ELEVATION};

    use super::*;

    #[test]
    fn regions_have_distinct_headings_above_water() {
        for region in [STARTER_REGION, PALM_COAST_REGION] {
            let spawn = region.spawn;
            let up = terrain_aligned_direction(spawn.direction);
            let look_at = terrain_aligned_direction(spawn.look_at);
            assert!(up.is_normalized() && look_at.is_normalized());
            assert!(up.cross(look_at).length() > 0.0);
            assert!(spawn.elevation > WATER_SURFACE_ELEVATION);
        }
    }
}
