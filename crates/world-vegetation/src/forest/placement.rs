//! Canonical tree sites. Representation and task completion order do not affect placement.
use bevy::{math::DVec3, prelude::*};
use world_core::{planet::RADIUS, SurfaceHeights};

use super::{hero_oak, tree_height, tree_site, Random, Species};

pub(super) const PATCH_METERS: f64 = 145.0;
const ANGLE: f64 = PATCH_METERS / RADIUS;
// Cap prospective sites per 145 m patch; density masks then create wide
// genuine clearings instead of stacking dozens of trees in every patch.
const CANDIDATES: usize = 64;
const GROUP: i32 = 4;
const HALF_PI: f64 = std::f64::consts::FRAC_PI_2;
const TAU: f64 = std::f64::consts::TAU;
const LAST_ROW: i32 = (std::f64::consts::PI / ANGLE) as i32;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct PatchKey(pub i32, pub i32);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct NodeKey(i32, i32);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Site {
    pub position: DVec3,
    pub species: Species,
    pub variant: usize,
    pub scale: f32,
    pub yaw: f32,
}

impl Site {
    pub fn height(self) -> f32 {
        tree_height(self.species, self.variant as u64) * self.scale
    }

    pub fn rotation(self) -> Quat {
        Quat::from_rotation_arc(Vec3::Y, self.position.normalize().as_vec3())
            * Quat::from_rotation_y(self.yaw)
    }
}

impl PatchKey {
    pub fn parent(self) -> NodeKey {
        let band = self.0 / GROUP;
        // At Mars radius there are >100k longitude columns. Multiply
        // in i64: i32 would silently overflow when grouping patches.
        NodeKey(
            band,
            (i64::from(self.1) * i64::from(node_columns(band)) / i64::from(columns(self.0))) as i32,
        )
    }

    /// Candidate patches whose canonical trees may shade the given surface tile.
    /// The surrounding rows include shade cast across neighboring trees' patch edges.
    pub fn shade_neighborhood(direction: DVec3) -> impl Iterator<Item = Self> {
        let latitude = direction.y.clamp(-1.0, 1.0).asin();
        let longitude = direction.z.atan2(direction.x).rem_euclid(TAU);
        let row = ((latitude + HALF_PI) / ANGLE).floor() as i32;
        ((row - 2).max(0)..=(row + 2).min(LAST_ROW)).flat_map(move |r| {
            let count = columns(r);
            let col = (longitude * f64::from(count) / TAU).floor() as i32;
            (-2..=2).map(move |offset| Self(r, (col + offset).rem_euclid(count)))
        })
    }

    pub fn center(self) -> DVec3 {
        direction(
            (f64::from(self.0) + 0.5) * ANGLE - HALF_PI,
            (f64::from(self.1) + 0.5) * TAU / f64::from(columns(self.0)),
        )
    }

    pub fn sites(self, heights: &SurfaceHeights) -> Vec<Site> {
        let latitude = f64::from(self.0) * ANGLE - HALF_PI;
        let step = TAU / f64::from(columns(self.0));
        let mut seed = ((self.0 as u64) << 32) ^ self.1 as u64;
        seed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        seed = (seed ^ (seed >> 27)).wrapping_mul(0x94d049bb133111eb);
        seed ^= seed >> 31;
        (0..CANDIDATES)
            .filter_map(|candidate| {
                let mut rng =
                    Random::new(seed ^ (candidate as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15));
                let up = direction(
                    (latitude + rng.unit64() * ANGLE).clamp(-HALF_PI, HALF_PI),
                    (f64::from(self.1) + rng.unit64()) * step,
                );
                let (ground, species) = tree_site(heights, up, &mut rng)?;
                let variant = rng.next_u32() as usize % species.variants();
                let scale = rng.range(0.84, 1.28);
                let yaw = rng.range(0.0, std::f32::consts::TAU);
                let sink = match (species, variant) {
                    (Species::Oak, _) => hero_oak::ROOT_SINK * f64::from(scale),
                    (Species::Palm, _) | (Species::Jungle, 1) => 0.35 * f64::from(scale),
                    _ => 1.6,
                };
                Some(Site {
                    position: up * (RADIUS + ground - sink),
                    species,
                    variant,
                    scale,
                    yaw,
                })
            })
            .collect()
    }
}

impl NodeKey {
    pub fn center(self) -> DVec3 {
        direction(
            (f64::from(self.0 * GROUP) + 2.0) * ANGLE - HALF_PI,
            (f64::from(self.1) + 0.5) * TAU / f64::from(node_columns(self.0)),
        )
    }

    pub fn leaves(self) -> Vec<PatchKey> {
        let parents = node_columns(self.0);
        (self.0 * GROUP..=(self.0 * GROUP + GROUP - 1).min(LAST_ROW))
            .flat_map(|row| {
                let count = columns(row);
                let edge = |col: i32| {
                    ((i64::from(col) * i64::from(count) + i64::from(parents) - 1)
                        / i64::from(parents)) as i32
                };
                (edge(self.1)..edge(self.1 + 1)).map(move |col| PatchKey(row, col))
            })
            .collect()
    }

    pub fn around(up: DVec3, reach: f64) -> Vec<Self> {
        let longitude = up.z.atan2(up.x).rem_euclid(TAU);
        let latitude = up.y.clamp(-1.0, 1.0).asin();
        let row = ((latitude + HALF_PI) / (ANGLE * f64::from(GROUP))) as i32;
        let span = (reach / (PATCH_METERS * f64::from(GROUP))).ceil() as i32 + 1;
        let mut keys = Vec::new();
        for band in (row - span).max(0)..=(row + span).min(LAST_ROW / GROUP) {
            let count = node_columns(band);
            let col = (longitude * f64::from(count) / TAU) as i32;
            // Also avoid enumerating duplicate cells where a polar band wraps.
            let width = (2 * span + 1).min(count);
            for delta in -width / 2..(width - width / 2) {
                let key = Self(band, (col + delta).rem_euclid(count));
                if key.center().distance(up) * RADIUS <= reach {
                    keys.push(key);
                }
            }
        }
        keys
    }
}

fn columns(row: i32) -> i32 {
    let latitude = -HALF_PI + (f64::from(row) + 0.5) * ANGLE;
    ((TAU * latitude.cos().abs().max(0.08) / ANGLE).round() as i32).max(1)
}

fn node_columns(band: i32) -> i32 {
    (columns(band * GROUP + 2) + GROUP - 1) / GROUP
}

fn direction(latitude: f64, longitude: f64) -> DVec3 {
    let (sin, cos) = latitude.clamp(-HALF_PI, HALF_PI).sin_cos();
    DVec3::new(cos * longitude.cos(), sin, cos * longitude.sin())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, path::Path};
    use world_core::coordinates::terrain_aligned_direction;

    #[test]
    fn hierarchy_covers_wrapped_rows_with_no_duplicate_sites() {
        for band in [0, 131, LAST_ROW / GROUP] {
            let keys: Vec<_> = (0..node_columns(band))
                .flat_map(|col| NodeKey(band, col).leaves())
                .collect();
            assert_eq!(
                keys.len(),
                keys.iter().copied().collect::<HashSet<_>>().len()
            );
            let count: i32 = (band * GROUP..=(band * GROUP + GROUP - 1).min(LAST_ROW))
                .map(columns)
                .sum();
            assert_eq!(keys.len(), count as usize);
            for col in 0..node_columns(band) {
                let node = NodeKey(band, col);
                let leaves = node.leaves();
                assert!(leaves.len() <= 24);
                assert!(leaves.into_iter().all(|patch| patch.parent() == node));
            }
        }
    }

    #[test]
    fn equatorial_patch_groups_do_not_overflow_at_mars_scale() {
        let band = LAST_ROW / GROUP / 2;
        let row = band * GROUP + 2;
        let col = columns(row) / 2;
        assert!(
            i64::from(col) * i64::from(node_columns(band)) > i64::from(i32::MAX),
            "this test must exercise the original i32 overflow"
        );
        let patch = PatchKey(row, col);
        let parent = patch.parent();
        assert!(parent.leaves().contains(&patch));
    }

    #[test]
    fn palm_coast_spawn_stays_on_dry_ground() {
        let heights = SurfaceHeights::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        let spawn = world_regions::PALM_COAST_REGION.spawn;
        let up = terrain_aligned_direction(spawn.direction);
        let ground = heights.sample(up).unwrap();
        assert!(ground > world_core::planet::WATER_SURFACE_ELEVATION);
        // The authored start still refers to the original continental map.
        assert!((heights.sample_base(up).unwrap() - spawn.elevation).abs() < 0.02);
        assert!(!heights.is_river(up));

        // The old look_at marker aimed at an exact 100-km-scale palm site.
        // That position is not meaningful after repatching a Mars sphere;
        // selecting a particular random tree would incorrectly pin a test
        // to one vegetation distribution rather than testing the coast.
        let (east, north) = super::super::tangent_basis(up);
        assert!(super::super::near_sea(up, east, north, |dir| heights.sample_base(dir)));
    }

    #[test]
    fn sites_survive_rebuilding_with_the_same_biome_and_pose() {
        let heights = SurfaceHeights::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        let up = terrain_aligned_direction(world_regions::STARTER_REGION.spawn.direction);
        let (patch, sites) = NodeKey::around(up, 1_800.0)
            .into_iter()
            .flat_map(NodeKey::leaves)
            .find_map(|patch| {
                let sites = patch.sites(&heights);
                (!sites.is_empty()).then_some((patch, sites))
            })
            .expect("the fixture contains woodland");
        assert_eq!(sites, patch.sites(&heights));
        assert!(sites.iter().all(|site| {
            site.position.normalize().distance(patch.center()) * RADIUS < PATCH_METERS
        }));
    }
}
