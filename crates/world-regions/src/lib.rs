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

#[cfg(test)]
mod tests {
    use world_core::{coordinates::terrain_aligned_direction, planet::WATER_SURFACE_ELEVATION};

    use super::*;

    #[test]
    fn starter_has_a_distinct_heading_above_water() {
        let spawn = STARTER_REGION.spawn;
        let up = terrain_aligned_direction(spawn.direction);
        let look_at = terrain_aligned_direction(spawn.look_at);
        assert!(up.is_normalized() && look_at.is_normalized());
        assert!(up.cross(look_at).length() > 0.0);
        assert!(spawn.elevation > WATER_SURFACE_ELEVATION);
    }
}
