//! Canonical cube-sphere clumps seeded independently from the procedural ground cover.
use std::collections::HashSet;

use bevy::{
    math::{DVec2, DVec3},
    prelude::*,
};
use world_core::{
    coordinates::{cube_direction, surface_uv},
    planet::RADIUS,
    SurfaceHeights,
};

use crate::forest::{noise3, tangent_basis, Random};

pub(super) const MAX_PLANTS: usize = 16;
pub(super) const PATCH_ALLOWANCE: f64 = 40.0;
const CELLS: f64 = 2.0 * RADIUS / 48.0;
const SHADE_REACH: f64 = 14.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub(super) struct Patch {
    face: usize,
    x: u32,
    y: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Plant {
    pub position: DVec3,
    pub rotation: Quat,
    pub scale: f32,
}

impl Patch {
    fn at(direction: DVec3) -> Self {
        let (face, uv) = surface_uv(direction);
        let cell = (uv * CELLS).min(DVec2::splat(CELLS - 1.0)).as_uvec2();
        Self {
            face,
            x: cell.x,
            y: cell.y,
        }
    }

    fn direction(self, offset: DVec2) -> DVec3 {
        let uv = (DVec2::new(f64::from(self.x), f64::from(self.y)) + offset) / CELLS;
        cube_direction(self.face, uv)
    }

    pub fn center(self) -> DVec3 {
        self.direction(DVec2::splat(0.5))
    }

    pub fn around(up: DVec3, reach: f64) -> Vec<Self> {
        let (east, north) = tangent_basis(up);
        let spacing = 16.0;
        let extent = (reach / spacing).ceil() as i32;
        let mut keys = HashSet::new();
        for y in -extent..=extent {
            for x in -extent..=extent {
                let offset = (east * f64::from(x) + north * f64::from(y)) * (spacing / RADIUS);
                keys.insert(Self::at((up + offset).normalize()));
            }
        }
        let mut keys: Vec<_> = keys
            .into_iter()
            .filter(|key| key.center().distance(up) * RADIUS <= reach)
            .collect();
        keys.sort_unstable_by(|a, b| {
            a.center()
                .distance_squared(up)
                .total_cmp(&b.center().distance_squared(up))
                .then(a.cmp(b))
        });
        keys
    }

    pub fn grow(self, heights: &SurfaceHeights, trees: &[DVec3]) -> Vec<Plant> {
        let seed = 0x54a4_7ac9_b054_9217
            ^ (self.face as u64) << 48
            ^ u64::from(self.x) << 24
            ^ u64::from(self.y);
        let mut rng = Random::new(seed);
        let mut plants: Vec<Plant> = Vec::new();
        for cell in [DVec2::ZERO, DVec2::X, DVec2::Y, DVec2::ONE] {
            let offset = cell * 0.5 + DVec2::new(rng.unit64(), rng.unit64()) * 0.31 + 0.095;
            let center = self.direction(offset);
            let Some((height, _)) = habitat(heights, center, trees) else {
                continue;
            };
            let damp = dampness(center * (RADIUS + height));
            let chance = ((damp - 0.40) * 1.65 + 0.25).clamp(0.0, 0.77);
            if rng.unit() > chance {
                continue;
            }

            let (east, north) = tangent_basis(center);
            for index in 0..6 {
                let turn = rng.unit64() * std::f64::consts::TAU;
                let radius = if index == 0 {
                    0.0
                } else {
                    0.85 + rng.unit64() * 2.5
                };
                let (sin, cos) = turn.sin_cos();
                let direction = (center + radius / RADIUS * (cos * east + sin * north)).normalize();
                if Self::at(direction) != self {
                    continue;
                }
                let Some((elevation, normal)) = habitat(heights, direction, trees) else {
                    continue;
                };
                let position = direction * (RADIUS + elevation);
                if plants
                    .iter()
                    .any(|plant| plant.position.distance_squared(position) < 0.82_f64.powi(2))
                {
                    continue;
                }
                let scale = rng.range(0.69, 1.32);
                let rotation = Quat::from_rotation_arc(Vec3::Y, normal)
                    * Quat::from_rotation_y(rng.range(0.0, std::f32::consts::TAU));
                plants.push(Plant {
                    position,
                    rotation,
                    scale,
                });
                if plants.len() == MAX_PLANTS {
                    return plants;
                }
            }
        }
        plants
    }
}

pub(super) fn nearby_trunks(
    origin: DVec3,
    positions: impl IntoIterator<Item = DVec3>,
) -> Vec<DVec3> {
    positions
        .into_iter()
        .filter(|&position| {
            position.distance_squared(origin) <= (PATCH_ALLOWANCE + SHADE_REACH).powi(2)
        })
        .collect()
}

fn dampness(position: DVec3) -> f32 {
    let weather = noise3(position.normalize().as_vec3() * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let pockets = noise3(position.as_vec3() * 0.075 + Vec3::new(-5.1, 2.7, 7.2));
    (0.58 + 0.94 * weather + 0.27 * pockets).clamp(0.0, 1.0)
}

fn habitat(heights: &SurfaceHeights, up: DVec3, trees: &[DVec3]) -> Option<(f64, Vec3)> {
    if heights.construction_clears(up) {
        return None;
    }
    let elevation = heights.sample(up)?;
    if !(2.7..=1950.0).contains(&elevation) || heights.open_water_at(up, elevation).is_some() {
        return None;
    }
    let position = up * (RADIUS + elevation);
    if dampness(position) < 0.43 {
        return None;
    }

    let mut shaded = false;
    for tree in trees {
        let delta = *tree - position;
        let radial = delta.dot(up);
        let lateral = delta.length_squared() - radial * radial;
        if lateral < 1.1_f64.powi(2) {
            return None;
        }
        if radial.abs() <= 8.0 && lateral < SHADE_REACH.powi(2) {
            shaded = true;
        }
    }
    if !shaded {
        return None;
    }

    let (east, north) = tangent_basis(up);
    let grade = |axis: DVec3| {
        let offset = axis * (2.5 / RADIUS);
        let lower = heights.sample((up - offset).normalize())?;
        let higher = heights.sample((up + offset).normalize())?;
        Some((higher - lower) / 5.0)
    };
    let grade_east = grade(east)?;
    let grade_north = grade(north)?;
    if grade_east.hypot(grade_north) > 0.63 {
        return None;
    }
    let normal = (up - (east * grade_east + north * grade_north) * 0.28).normalize();
    Some((elevation, normal.as_vec3()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forest::placement::PatchKey;
    use std::path::Path;
    use world_core::coordinates::terrain_aligned_direction;

    #[test]
    fn starter_woodland_grows_stable_clumps_on_dry_ground() {
        let heights = SurfaceHeights::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        let up = terrain_aligned_direction(world_regions::STARTER_REGION.spawn.direction);
        let trees: Vec<_> = PatchKey::shade_neighborhood(up)
            .flat_map(|patch| patch.sites(&heights))
            .map(|tree| tree.position)
            .collect();
        let (patch, trunks, plants) = Patch::around(up, 150.0)
            .into_iter()
            .find_map(|patch| {
                let up = patch.center();
                let anchor = up * (RADIUS + heights.sample(up)?);
                let trunks = nearby_trunks(anchor, trees.iter().copied());
                let plants = patch.grow(&heights, &trunks);
                (plants.len() >= 2).then_some((patch, trunks, plants))
            })
            .expect("starter region has shaded, humid clumps");
        assert_eq!(plants, patch.grow(&heights, &trunks));
        assert!(patch.grow(&heights, &[]).is_empty());
        assert!(plants.iter().any(|a| plants.iter().any(|b| {
            let distance = a.position.distance(b.position);
            (0.82..4.0).contains(&distance)
        })));
        for plant in plants {
            let up = plant.position.normalize();
            let ground = heights.sample(up).unwrap();
            assert!((plant.position.length() - RADIUS - ground).abs() < 1e-7);
            assert!(!heights.is_river(up));
            assert_eq!(Patch::at(up), patch);
        }
    }
}
