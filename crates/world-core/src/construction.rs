//! Stable planet coordinates and earthwork for local settlement construction.

use bevy_math::DVec3;

use crate::planet::RADIUS;

/// Width of the graded and vegetation cleared shoulder around a building pad.
pub const CONSTRUCTION_BLEND_M: f64 = 3.0;

/// A one meter tangent grid anchored to a fixed radial direction.
#[derive(Clone, Copy, Debug)]
pub struct ConstructionGrid {
    /// Unit direction of the grid origin.
    pub origin: DVec3,
    /// Cell coordinate corresponding to the origin.
    pub origin_cell: i32,
}

impl ConstructionGrid {
    /// Map an integer cell and elevation to a planet centered position in meters.
    pub fn position(self, x: i32, y: i32, elevation: f64) -> DVec3 {
        self.position_at(f64::from(x), f64::from(y), elevation)
    }

    /// Map a fractional cell position to a planet centered position in meters.
    pub fn position_at(self, x: f64, y: f64, elevation: f64) -> DVec3 {
        let up = self.origin.normalize();
        let east = DVec3::Y.cross(up).normalize();
        let north = up.cross(east);
        let offset =
            east * (x - f64::from(self.origin_cell)) + north * (y - f64::from(self.origin_cell));
        let direction = (up * RADIUS + offset).normalize();
        direction * (RADIUS + elevation)
    }

    /// Unit radial direction at a grid cell.
    pub fn direction(self, x: i32, y: i32) -> DVec3 {
        self.position(x, y, 0.0).normalize()
    }
}

/// Persistent flattened footprint, with a smooth shoulder around its edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConstructionEdit {
    /// Planet centered position of the footprint center.
    pub center: DVec3,
    /// Eastward half extent in meters.
    pub half_width: f64,
    /// Northward half extent in meters.
    pub half_height: f64,
    /// Graded shoulder width in meters.
    pub blend: f64,
    /// Finished pad elevation above the reference sphere.
    pub elevation: f64,
}

impl ConstructionEdit {
    /// Whether a plant root lies on the cleared pad or its graded shoulder.
    pub fn clears(self, direction: DVec3) -> bool {
        let up = self.center.normalize();
        let east = DVec3::Y.cross(up).normalize();
        let north = up.cross(east);
        let delta = direction.normalize() * RADIUS - up * RADIUS;
        delta.dot(east).abs() <= self.half_width + self.blend
            && delta.dot(north).abs() <= self.half_height + self.blend
    }

    /// Blend the original ground elevation into the flattened pad.
    pub fn height(self, direction: DVec3, original: f64) -> f64 {
        let up = self.center.normalize();
        let east = DVec3::Y.cross(up).normalize();
        let north = up.cross(east);
        let point = direction.normalize() * RADIUS;
        let center = up * RADIUS;
        let delta = point - center;
        let distance = (delta.dot(east).abs() - self.half_width)
            .max(delta.dot(north).abs() - self.half_height)
            .max(0.0);
        if distance >= self.blend {
            return original;
        }
        let t = (distance / self.blend).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        self.elevation + (original - self.elevation) * smooth
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_remains_stable_at_planet_scale() {
        let grid = ConstructionGrid {
            origin: DVec3::new(-0.97, 0.2, 0.01).normalize(),
            origin_cell: 500_000,
        };
        let a = grid.position(500_000, 500_000, 10.0);
        let b = grid.position(500_005, 500_000, 10.0);
        assert!((a.distance(b) - 5.0).abs() < 0.001);
        assert!((a.length() - RADIUS - 10.0).abs() < 1e-8);
    }

    #[test]
    fn pad_blends_without_edge_jump() {
        let edit = ConstructionEdit {
            center: DVec3::X * RADIUS,
            half_width: 2.5,
            half_height: 2.5,
            blend: 3.0,
            elevation: 10.0,
        };
        assert_eq!(edit.height(DVec3::X, 12.0), 10.0);
        let outside = (DVec3::X * RADIUS + DVec3::Y * 6.0).normalize();
        assert_eq!(edit.height(outside, 12.0), 12.0);
    }
}
