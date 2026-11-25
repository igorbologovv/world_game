//! Canonical plants for a ground-cover patch, independent of loaded LODs.

use std::f32::consts::TAU;

use bevy::{
    math::{DVec2, DVec3},
    prelude::*,
};
use world_core::{
    planet::{RADIUS, SEA_LEVEL, WATER_SURFACE_ELEVATION},
    SurfaceHeights,
};

use super::Patch;
use crate::forest::{grove_density, lod::smoothstep, near_sea, noise3, tangent_basis, Random};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    Grass,
    Flower,
    Shrub,
    Fern,
    Coastal,
}

impl Family {
    pub(super) const ALL: [Self; 5] = [
        Self::Grass,
        Self::Flower,
        Self::Shrub,
        Self::Fern,
        Self::Coastal,
    ];

    pub(super) const fn directory(self) -> &'static str {
        match self {
            Self::Grass => "grasses",
            Self::Flower => "flowers",
            Self::Shrub => "shrubs",
            Self::Fern => "ferns",
            Self::Coastal => "coastal",
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Species {
    pub(super) name: &'static str,
    pub(super) family: Family,
    communities: [f32; 5],
    moisture: [f32; 2],
    temperature: [f32; 2],
    max_elevation: f32,
    max_grade: f32,
}

const fn plant(
    name: &'static str,
    family: Family,
    communities: [f32; 5],
    moisture: [f32; 2],
    temperature: [f32; 2],
    max_elevation: f32,
    max_grade: f32,
) -> Species {
    Species {
        name,
        family,
        communities,
        moisture,
        temperature,
        max_elevation,
        max_grade,
    }
}

use Family::{Coastal, Fern, Flower, Grass, Shrub};

// Affinities for meadow, forest understory, freshwater margin, coast and scrub.
#[rustfmt::skip]
pub(super) const SPECIES: [Species; 40] = [
    plant("blue_fescue",          Grass, [1.0, 0.2, 0.0, 0.1, 0.9], [0.23, 0.57], [0.18, 0.65], 3050., 0.77),
    plant("red_bentgrass",        Grass, [1.3, 0.3, 0.3, 0.0, 0.2], [0.38, 0.71], [0.27, 0.77], 2750., 0.62),
    plant("silver_oat_grass",     Grass, [0.9, 0.1, 0.0, 0.1, 0.9], [0.22, 0.49], [0.42, 0.87], 2600., 0.82),
    plant("purple_moor_grass",    Grass, [0.4, 0.3, 1.5, 0.1, 0.0], [0.62, 0.98], [0.25, 0.69], 1700., 0.42),
    plant("golden_foxtail",       Grass, [1.2, 0.1, 0.2, 0.1, 0.7], [0.23, 0.66], [0.50, 1.00], 2050., 0.68),
    plant("prairie_dropseed",     Grass, [1.1, 0.0, 0.0, 0.0, 1.0], [0.18, 0.52], [0.37, 0.88], 2900., 0.74),
    plant("ribbon_grass",         Grass, [0.3, 0.6, 1.4, 0.0, 0.0], [0.65, 1.00], [0.22, 0.83], 1550., 0.43),
    plant("spear_grass",          Grass, [0.8, 0.1, 0.0, 0.2, 1.4], [0.12, 0.49], [0.42, 1.00], 2800., 0.87),
    plant("oxeye_daisy",          Flower, [1.2, 0.1, 0.0, 0.0, 0.1], [0.38, 0.65], [0.28, 0.77], 2460., 0.60),
    plant("scarlet_poppy",        Flower, [0.9, 0.0, 0.0, 0.2, 1.0], [0.19, 0.51], [0.42, 0.91], 2280., 0.63),
    plant("harebell",             Flower, [0.8, 0.4, 0.0, 0.1, 0.6], [0.30, 0.66], [0.16, 0.57], 3220., 0.77),
    plant("pink_yarrow",          Flower, [1.1, 0.1, 0.0, 0.1, 0.5], [0.25, 0.61], [0.28, 0.85], 2900., 0.65),
    plant("blue_lupine",          Flower, [0.8, 0.3, 0.0, 0.1, 0.4], [0.33, 0.62], [0.26, 0.73], 2700., 0.65),
    plant("yellow_buttercup",     Flower, [0.7, 0.1, 1.1, 0.0, 0.0], [0.55, 0.86], [0.24, 0.79], 2100., 0.49),
    plant("wild_carrot",          Flower, [0.9, 0.1, 0.0, 0.2, 0.5], [0.27, 0.61], [0.32, 0.89], 2400., 0.65),
    plant("meadow_cranesbill",    Flower, [0.7, 0.8, 0.4, 0.0, 0.0], [0.46, 0.80], [0.26, 0.72], 2050., 0.63),
    plant("silver_sagebrush",     Shrub, [0.1, 0.0, 0.0, 0.2, 1.3], [0.10, 0.40], [0.24, 0.96], 3100., 0.95),
    plant("flowering_heather",    Shrub, [0.3, 0.7, 0.3, 0.2, 0.7], [0.35, 0.72], [0.17, 0.61], 2700., 0.82),
    plant("juniper_scrub",        Shrub, [0.1, 0.5, 0.0, 0.1, 1.0], [0.22, 0.54], [0.14, 0.62], 3200., 0.98),
    plant("red_dogwood",          Shrub, [0.0, 0.9, 0.9, 0.0, 0.0], [0.56, 0.93], [0.24, 0.72], 1500., 0.53),
    plant("bog_rosemary",         Shrub, [0.0, 0.3, 1.2, 0.0, 0.0], [0.70, 1.00], [0.13, 0.60], 1450., 0.40),
    plant("wild_boxwood",         Shrub, [0.0, 1.0, 0.0, 0.1, 0.5], [0.37, 0.79], [0.46, 0.99], 2220., 0.91),
    plant("sweet_myricaria",      Shrub, [0.0, 0.1, 1.0, 0.0, 0.2], [0.62, 0.96], [0.30, 0.87], 2400., 0.58),
    plant("scrub_cinquefoil",     Shrub, [0.3, 0.3, 0.2, 0.0, 1.0], [0.28, 0.67], [0.16, 0.78], 3050., 0.84),
    plant("sword_fern",           Fern, [0.0, 1.5, 0.4, 0.0, 0.0], [0.55, 0.94], [0.43, 0.82], 2100., 0.74),
    plant("lady_fern",            Fern, [0.0, 1.2, 0.7, 0.0, 0.0], [0.62, 0.95], [0.27, 0.69], 1980., 0.63),
    plant("maidenhair_fern",      Fern, [0.0, 1.2, 0.8, 0.0, 0.0], [0.63, 1.00], [0.44, 0.86], 2300., 0.82),
    plant("bracken_fern",         Fern, [0.4, 1.2, 0.0, 0.0, 0.2], [0.44, 0.80], [0.29, 0.94], 2400., 0.80),
    plant("painted_fern",         Fern, [0.0, 1.4, 0.3, 0.0, 0.0], [0.57, 0.94], [0.24, 0.73], 1950., 0.64),
    plant("hart_tongue_fern",     Fern, [0.0, 1.2, 0.3, 0.0, 0.0], [0.51, 0.87], [0.36, 0.85], 2050., 0.87),
    plant("oak_fern",             Fern, [0.0, 1.5, 0.4, 0.0, 0.0], [0.51, 0.85], [0.15, 0.60], 2260., 0.76),
    plant("bird_nest_fern",       Fern, [0.0, 1.7, 0.6, 0.2, 0.0], [0.65, 1.00], [0.66, 1.00], 1300., 0.62),
    plant("dune_marram",       Coastal, [0.0, 0.0, 0.0, 1.5, 0.0], [0.19, 0.65], [0.25, 1.00],   55., 0.72),
    plant("sand_sedge",        Coastal, [0.0, 0.0, 0.0, 1.4, 0.0], [0.30, 0.83], [0.18, 0.87],   45., 0.55),
    plant("sea_lavender",      Coastal, [0.0, 0.0, 0.0, 1.1, 0.0], [0.49, 0.92], [0.28, 0.97],   31., 0.49),
    plant("beach_morning_glory", Coastal, [0.0, 0.0, 0.0, 1.3, 0.0], [0.39, 0.85], [0.63, 1.00], 45., 0.72),
    plant("sea_rocket",        Coastal, [0.0, 0.0, 0.0, 0.9, 0.0], [0.30, 0.73], [0.33, 1.00],   32., 0.61),
    plant("red_glasswort",     Coastal, [0.0, 0.0, 0.0, 1.0, 0.0], [0.60, 1.00], [0.32, 1.00],   17., 0.38),
    plant("beach_pea",         Coastal, [0.0, 0.0, 0.0, 1.0, 0.0], [0.35, 0.76], [0.23, 0.74],   44., 0.67),
    plant("sea_thrift",        Coastal, [0.0, 0.0, 0.0, 1.0, 0.0], [0.31, 0.71], [0.16, 0.77],   49., 0.83),
];

pub(super) const VARIANTS: usize = 3;
const PROBES: usize = 5;
const CLUSTERS: usize = 6;
pub(super) const MAX_SITES: usize = 22;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Site {
    pub(super) species: usize,
    pub(super) variant: usize,
    pub(super) position: DVec3,
    pub(super) rotation: Quat,
    pub(super) scale: Vec3,
    pub(super) phase: f32,
}

#[derive(Clone, Copy)]
struct Habitat {
    moisture: f32,
    temperature: f32,
    shade: f32,
    fresh_water: f32,
    coast: f32,
    elevation: f32,
    grade: f32,
}

impl Habitat {
    fn communities(self) -> [f32; 5] {
        let open = 1.0 - self.shade;
        let moist = smoothstep(0.33, 0.74, self.moisture);
        let cold_or_steep =
            smoothstep(0.29, 0.76, self.grade).max(1.0 - smoothstep(0.15, 0.36, self.temperature));
        [
            open * (0.28 + moist * 0.82) * (1.0 - 0.79 * self.coast.max(self.fresh_water)),
            self.shade * (0.25 + moist * 1.35) * (1.0 - 0.62 * self.coast),
            (self.fresh_water * 2.7 + smoothstep(0.70, 0.93, self.moisture) * 0.38)
                * (1.0 - smoothstep(0.25, 0.62, self.grade))
                * (1.0 - 0.83 * self.coast),
            self.coast * 3.6,
            open * (0.14 + (1.0 - moist) * 0.70 + cold_or_steep * 0.58)
                * (1.0 - 0.70 * self.coast.max(self.fresh_water)),
        ]
    }

    fn suitability(self, species: Species, community: usize) -> f32 {
        if self.grade > species.max_grade
            || self.elevation > species.max_elevation
            || (species.family == Coastal && self.coast < 0.10)
        {
            return 0.0;
        }
        let band = |value, [min, max]: [f32; 2], margin| {
            smoothstep(min - margin, min, value) * (1.0 - smoothstep(max, max + margin, value))
        };
        species.communities[community]
            * band(self.moisture, species.moisture, 0.20)
            * band(self.temperature, species.temperature, 0.13)
            * (1.0 - 0.65 * smoothstep(species.max_grade * 0.58, species.max_grade, self.grade))
    }
}

fn weighted(weights: &[f32], rng: &mut Random) -> Option<usize> {
    let total = weights.iter().sum::<f32>();
    if total <= 0.0 {
        return None;
    }
    let mut roll = rng.unit() * total;
    let mut last = None;
    for (index, &weight) in weights
        .iter()
        .enumerate()
        .filter(|&(_, weight)| *weight > 0.0)
    {
        last = Some(index);
        if roll < weight {
            return last;
        }
        roll -= weight;
    }
    last
}

fn land(heights: &SurfaceHeights, up: DVec3) -> Option<(f64, f32, Vec3)> {
    let elevation = heights.sample(up)?;
    if !(WATER_SURFACE_ELEVATION + 0.28..=3300.0).contains(&elevation)
        || heights.open_water_at(up, elevation).is_some()
    {
        return None;
    }
    let (east, north) = tangent_basis(up);
    let rise = |axis: DVec3| {
        heights
            .sample((up + axis * (2.2 / RADIUS)).normalize())
            .map(|h| (h - elevation) / 2.2)
    };
    let (e, n) = (rise(east)?, rise(north)?);
    let grade = e.hypot(n) as f32;
    if grade > 0.98 {
        return None;
    }
    let normal = (up - (east * e + north * n) * 0.24).normalize().as_vec3();
    Some((elevation, grade, normal))
}

fn coastal_strength(heights: &SurfaceHeights, up: DVec3, elevation: f64) -> f32 {
    if elevation > 58.0 {
        return 0.0;
    }
    let (east, north) = tangent_basis(up);
    let near = [east, north, -east, -north].into_iter().any(|axis| {
        heights
            .sample_base((up + axis * (320.0 / RADIUS)).normalize())
            .is_some_and(|height| height < SEA_LEVEL)
    });
    let shore = if near {
        1.0
    } else if near_sea(up, east, north, |p| heights.sample_base(p)) {
        // The legacy coast is resolved on the angular raster. Give its broad
        // coastal lowlands a weaker salt influence than the immediate shore.
        0.62
    } else {
        0.0
    };
    shore * (1.0 - smoothstep(9.0, 58.0, elevation as f32))
}

fn habitat(heights: &SurfaceHeights, up: DVec3, elevation: f64, grade: f32, coast: f32) -> Habitat {
    let direction = up.as_vec3();
    let weather = noise3(direction * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let rain = noise3(direction * 17.0 + Vec3::new(-2.1, 9.4, 4.6));
    let p = up * (RADIUS + elevation);
    let pockets = forest_field(p / 16.0, 0x8fa2_763a) as f32;
    let fresh_water = heights.freshwater_influence(up, elevation) as f32 * (1.0 - 0.77 * coast);
    let regional_moisture = 0.50 + 0.96 * weather + 0.32 * rain;
    let moisture =
        (regional_moisture + 0.24 * pockets + 0.33 * fresh_water + 0.16 * coast).clamp(0.0, 1.0);
    let temperature = (1.07 - 0.78 * direction.y.abs() - elevation.max(0.0) as f32 / 16_000.0
        + 0.13 * noise3(direction * 4.1 + Vec3::new(3.2, 18.6, -8.4)))
    .clamp(0.0, 1.0);
    let shade = canopy_shade(
        direction,
        regional_moisture.clamp(0.0, 1.0),
        temperature,
        elevation as f32,
        grade,
    ) * (0.84 + 0.16 * pockets);
    Habitat {
        moisture,
        temperature,
        shade,
        fresh_water,
        coast,
        elevation: elevation as f32,
        grade,
    }
}

// Match the canopy's climate bands and tree height limit on this branch.
fn canopy_shade(direction: Vec3, moisture: f32, temperature: f32, height: f32, grade: f32) -> f32 {
    let tropical = smoothstep(0.655, 0.705, temperature) * smoothstep(0.465, 0.525, moisture);
    let temperate = smoothstep(0.265, 0.315, temperature)
        * (1.0 - smoothstep(0.655, 0.705, temperature))
        * smoothstep(0.385, 0.445, moisture);
    let boreal = smoothstep(0.115, 0.155, temperature)
        * (1.0 - smoothstep(0.435, 0.485, temperature))
        * smoothstep(0.245, 0.295, moisture);
    let savanna = smoothstep(0.635, 0.695, temperature)
        * smoothstep(0.165, 0.215, moisture)
        * (1.0 - smoothstep(0.58, 0.64, moisture));
    let potential = (tropical * 0.98)
        .max(temperate * 0.94)
        .max(boreal * 0.87)
        .max(savanna * 0.34);
    smoothstep(0.075, 0.76, potential * grove_density(direction))
        * smoothstep(3.5, 11.0, height)
        * (1.0 - smoothstep(2200.0, 2600.0, height))
        * (1.0 - smoothstep(0.40, 0.62, grade))
        * (1.0 - smoothstep(0.93, 0.96, direction.y.abs()))
}

fn forest_hash(x: i64, y: i64, z: i64, seed: u64) -> f64 {
    let mut v = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B1_85EB_CA87)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    v ^= v >> 30;
    v = v.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    v ^= v >> 27;
    v = v.wrapping_mul(0x94D0_49BB_1331_11EB);
    v ^= v >> 31;
    (v as f64 / u64::MAX as f64) * 2.0 - 1.0
}

/// Smooth seeded vegetation noise in `[-1, 1]`.
///
/// Divide a planet-local position by the desired feature size before sampling.
fn forest_field(p: DVec3, seed: u64) -> f64 {
    let base = p.floor();
    let f = p - base;
    let t = f * f * (DVec3::splat(3.0) - f * 2.0);
    let x = base.x as i64;
    let y = base.y as i64;
    let z = base.z as i64;
    let mut value = 0.0;
    for dz in 0..=1 {
        for dy in 0..=1 {
            for dx in 0..=1 {
                let weight = (if dx == 0 { 1.0 - t.x } else { t.x })
                    * (if dy == 0 { 1.0 - t.y } else { t.y })
                    * (if dz == 0 { 1.0 - t.z } else { t.z });
                value += weight * forest_hash(x + dx, y + dy, z + dz, seed);
            }
        }
    }
    value
}

/// Rebuild the same clusters and part choices from the physical patch key.
pub(super) fn layout(key: Patch, heights: &SurfaceHeights, origin: DVec3) -> Vec<Site> {
    let up = origin.normalize();
    let (east, north) = tangent_basis(up);
    let coast = coastal_strength(heights, up, origin.length() - RADIUS);
    let mut sites: Vec<(usize, Site)> = Vec::with_capacity(MAX_SITES);
    for cluster in 0..CLUSTERS {
        let mut rng =
            Random::new(key.seed() ^ ((cluster as u64 + 1).wrapping_mul(0xbe4a_4c46_ac21_5ded)));
        let cell = DVec2::new((cluster % 3) as f64, (cluster / 3) as f64);
        let jitter = 0.16 + 0.68 * DVec2::new(rng.unit64(), rng.unit64());
        let center = key.direction((cell + jitter) / DVec2::new(3.0, 2.0));
        let Some((elevation, grade, _)) = land(heights, center) else {
            continue;
        };
        let habitat = habitat(heights, center, elevation, grade, coast);
        let clumping = forest_field(center * (RADIUS / 7.0), 0xbd85_2acc) as f32;
        let density = (0.47 + 0.46 * habitat.moisture + 0.19 * habitat.coast + 0.45 * clumping
            - 0.36 * grade)
            * (0.35 + 0.65 * smoothstep(0.10, 0.36, habitat.temperature));
        if rng.unit() > density || habitat.temperature < 0.09 {
            continue;
        }
        let Some(community) = weighted(&habitat.communities(), &mut rng) else {
            continue;
        };
        let weights = SPECIES.map(|species| habitat.suitability(species, community));
        let Some(primary) = weighted(&weights, &mut rng) else {
            continue;
        };
        let mut accepted = 0;
        let mut members = Vec::with_capacity(PROBES);
        for member in 0..PROBES {
            let turn = rng.unit64() * std::f64::consts::TAU;
            let radius = if member == 0 {
                0.0
            } else {
                0.65 + 2.6 * rng.unit64().sqrt()
            };
            let (sin, cos) = turn.sin_cos();
            let direction = (center + (cos * east + sin * north) * (radius / RADIUS)).normalize();
            if Patch::at(direction) != key {
                continue;
            }
            let Some((elevation, grade, normal)) = land(heights, direction) else {
                continue;
            };
            let first = if rng.unit() < 0.59 {
                primary
            } else {
                weighted(&weights, &mut rng).unwrap_or(primary)
            };
            let local = Habitat {
                grade,
                elevation: elevation as f32,
                ..habitat
            };
            let species = if local.suitability(SPECIES[first], community) > 0.0 {
                first
            } else if let Some(index) = weighted(
                &SPECIES.map(|species| local.suitability(species, community)),
                &mut rng,
            ) {
                index
            } else {
                continue;
            };
            let position = direction * (RADIUS + elevation);
            if members
                .iter()
                .chain(sites.iter())
                .any(|(_, site): &(usize, Site)| {
                    site.position.distance_squared(position) < 0.52_f64.powi(2)
                })
            {
                continue;
            }
            let size = rng.range(0.76, 1.26);
            let phase = rng.range(0.0, TAU);
            members.push((
                accepted,
                Site {
                    species,
                    variant: rng.next_u32() as usize % VARIANTS,
                    position,
                    rotation: Quat::from_rotation_arc(Vec3::Y, normal)
                        * Quat::from_rotation_y(phase),
                    scale: Vec3::new(
                        size * rng.range(0.83, 1.17),
                        size,
                        size * rng.range(0.83, 1.17),
                    ),
                    phase,
                },
            ));
            accepted += 1;
        }
        sites.extend(members);
    }
    // Admit the first plant of each clump before filling in its neighbors.
    sites.sort_by_key(|&(order, _)| order);
    sites
        .into_iter()
        .take(MAX_SITES)
        .map(|(_, site)| site)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn habitats_favor_the_expected_communities() {
        let meadow = Habitat {
            moisture: 0.61,
            temperature: 0.53,
            shade: 0.11,
            fresh_water: 0.0,
            coast: 0.0,
            elevation: 400.0,
            grade: 0.10,
        };
        let forest = Habitat {
            moisture: 0.77,
            shade: 0.87,
            ..meadow
        };
        let wetland = Habitat {
            moisture: 0.90,
            shade: 0.19,
            fresh_water: 0.92,
            ..meadow
        };
        let coast = Habitat {
            temperature: 0.82,
            elevation: 8.0,
            coast: 0.83,
            ..meadow
        };
        let rocky = Habitat {
            moisture: 0.28,
            temperature: 0.31,
            elevation: 2900.0,
            grade: 0.84,
            ..meadow
        };
        for (expected, habitat) in [meadow, forest, wetland, coast, rocky]
            .into_iter()
            .enumerate()
        {
            let scores = habitat.communities();
            assert!(scores
                .iter()
                .enumerate()
                .all(|(i, &score)| i == expected || score < scores[expected]));
        }
        let total = |habitat: Habitat, family, community| {
            SPECIES
                .iter()
                .copied()
                .filter(|s| s.family == family)
                .map(|s| habitat.suitability(s, community))
                .sum::<f32>()
        };
        assert!(total(forest, Fern, 1) > total(forest, Grass, 1));
        assert!(total(meadow, Flower, 0) > 2.0 && total(meadow, Grass, 0) > 2.0);
        assert!(total(wetland, Grass, 2) > 1.5 && total(wetland, Shrub, 2) > 1.5);
        assert!(total(coast, Coastal, 3) > 3.0 && total(wetland, Coastal, 3) == 0.0);
        assert_eq!(total(rocky, Fern, 1), 0.0);
        assert!(total(rocky, Shrub, 4) > 0.0);
        assert!(SPECIES.into_iter().all(|species| {
            let ideal = Habitat {
                moisture: (species.moisture[0] + species.moisture[1]) / 2.0,
                temperature: (species.temperature[0] + species.temperature[1]) / 2.0,
                elevation: 9.0,
                ..coast
            };
            (0..5).any(|community| ideal.suitability(species, community) > 0.0)
        }));
    }

    #[test]
    fn starter_and_palm_coast_grow_stable_mixed_clusters_on_land() {
        use std::{collections::HashSet, path::Path};
        use world_core::coordinates::terrain_aligned_direction;
        use world_regions::{PALM_COAST_REGION, STARTER_REGION};

        let heights = SurfaceHeights::load(Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        for region in [STARTER_REGION, PALM_COAST_REGION] {
            let up = terrain_aligned_direction(region.spawn.direction);
            let mut families = HashSet::new();
            let mut models = HashSet::new();
            let mut clustered = false;
            for patch in super::super::visible_patches(up) {
                let center = patch.direction(DVec2::splat(0.5));
                let origin = center * (RADIUS + heights.sample(center).unwrap());
                let sites = layout(patch, &heights, origin);
                assert_eq!(sites, layout(patch, &heights, origin));
                assert!(sites.len() <= MAX_SITES);
                for site in &sites {
                    let direction = site.position.normalize();
                    let (height, grade, _) = land(&heights, direction).unwrap();
                    assert_eq!(Patch::at(direction), patch);
                    assert!((site.position.length() - RADIUS - height).abs() < 1e-7);
                    let species = SPECIES[site.species];
                    assert!(
                        grade <= species.max_grade && height <= f64::from(species.max_elevation)
                    );
                    families.insert(species.family as usize);
                    models.insert((site.species, site.variant));
                    clustered |= sites
                        .iter()
                        .any(|other| (0.52..3.5).contains(&site.position.distance(other.position)));
                }
            }
            assert!(
                clustered && families.len() >= 2 && models.len() >= 8,
                "{}: {families:?}; {models:?}",
                region.id.0
            );
            if region.id == PALM_COAST_REGION.id {
                assert!(
                    families.contains(&(Coastal as usize)),
                    "Palm Coast should contain salt-tolerant plants"
                );
            }
        }
    }
}
