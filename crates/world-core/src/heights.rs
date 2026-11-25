use std::{path::Path, sync::Arc};

use bevy_ecs::prelude::Resource;
use bevy_math::{DVec2, DVec3, IVec3};

use crate::{
    coordinates::{height_source_uv, source_face_uv, surface_uv},
    inland_water::{smoothstep, InlandWater, LakeTile, RiverReach, WaterKind, WaterSample},
    planet::{
        decode_height, HEIGHT_MAP_SIZE, HEIGHT_SOURCE_PATH, LAKE_MASK_PATH, OCEAN_ATLAS_PATH,
        RADIUS, RIVER_ATLAS_PATH, WATER_SURFACE_ELEVATION,
    },
    terrain_detail::{self, Basin, BASIN_CELL},
};

type HeightFace = image::ImageBuffer<image::Luma<u16>, Vec<u16>>;
const FACE_BLEND_WIDTH: f64 = 0.0025;

/// Read-only terrain height and water queries shared by simulation and rendering.
///
/// Clones share the immutable raster data with streaming workers.
#[derive(Clone, Resource)]
pub struct SurfaceHeights {
    faces: Option<Arc<[HeightFace; 6]>>,
    rivers: Option<Arc<image::GrayImage>>,
    lakes: [Option<Arc<image::GrayImage>>; 6],
    inland: Arc<InlandWater>,
    ocean: Option<Arc<image::GrayImage>>,
}

/// A shallow pool enclosed by a procedural wetland basin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalPool {
    /// Unit radial direction at the center of the basin.
    pub center: DVec3,
    /// Radius in meters enclosing the pool and its dry rim.
    pub radius: f64,
    /// Constant water elevation in meters above the reference sphere.
    pub elevation: f64,
}

#[derive(Clone, Copy)]
struct TerrainInput {
    base: f64,
    river: f64,
    lake: f64,
}

impl TerrainInput {
    fn elevation(self, direction: DVec3) -> f64 {
        if self.base <= 1.5 {
            return self.base;
        }
        (self.base + terrain_detail::detail(direction, self.base, self.river, self.lake)).max(1.5)
    }
}

impl SurfaceHeights {
    /// Load terrain, coverage masks and baked water geometry from an asset directory.
    /// Height queries require all six TIFF faces. Inland meshes are optional.
    pub fn load(assets: &Path) -> Self {
        let source = assets.join(HEIGHT_SOURCE_PATH);
        let faces: Option<Vec<_>> = (0..6)
            .map(|face| {
                image::open(source.join(format!("face{face}.tif")))
                    .ok()
                    .map(|image| image.into_luma16())
            })
            .collect();
        let rivers = image::open(assets.join(RIVER_ATLAS_PATH))
            .ok()
            .and_then(|image| {
                let image = image.into_rgba8();
                let side = image.width() / 3;
                if side == 0 || image.width() != side * 3 || image.height() != side * 2 {
                    return None;
                }
                let (width, height) = image.dimensions();
                let coverage = image
                    .into_raw()
                    .chunks_exact(4)
                    .map(|pixel| pixel[3])
                    .collect();
                image::GrayImage::from_raw(width, height, coverage)
            });
        Self {
            inland: Arc::new(InlandWater::load(assets)),
            ocean: image::open(assets.join(OCEAN_ATLAS_PATH))
                .ok()
                .map(|image| image.into_luma8())
                .filter(|image| {
                    image.width() >= 3
                        && image.width() % 3 == 0
                        && image.height() == (image.width() / 3) * 2
                })
                .map(Arc::new),
            faces: faces.and_then(|faces| faces.try_into().ok()).map(Arc::new),
            rivers: rivers.map(Arc::new),
            lakes: std::array::from_fn(|face| {
                image::open(assets.join(LAKE_MASK_PATH).join(format!("face{face}.png")))
                    .ok()
                    .map(|image| Arc::new(image.into_luma8()))
            }),
        }
    }

    /// Sample elevation in meters above the reference sphere along a radial direction.
    pub fn sample(&self, direction: DVec3) -> Option<f64> {
        let (face, uv) = face_uv(direction)?;
        let faces = self.faces.as_ref()?;
        if faces[0].width() != HEIGHT_MAP_SIZE {
            return Some(sample_height(&faces[face], uv));
        }
        let up = direction.normalize();
        let input = self.terrain_input(direction, face, uv, faces);
        let mut height = input.elevation(up);
        let lake = self.lake_level(up, face, uv);
        if let Some(stage) = lake {
            height = terrain_detail::sediment_shelf(up, input.base, height, stage, false);
        } else if (WATER_SURFACE_ELEVATION - 4.0..=WATER_SURFACE_ELEVATION + 7.0)
            .contains(&input.base)
            && self.ocean_near(face, uv)
        {
            height = terrain_detail::sediment_shelf(
                up,
                input.base,
                height,
                WATER_SURFACE_ELEVATION,
                true,
            );
        }
        height = self.inland.grade_ground(up, height, lake);
        Some(height)
    }

    /// Directed rivers decoded from the same assets used for terrain and gameplay.
    pub fn river_reaches(&self) -> &[RiverReach] {
        &self.inland.rivers
    }

    /// Spill-elevation tiles decoded from the baked lake surface.
    pub fn lake_tiles(&self) -> &[LakeTile] {
        &self.inland.lakes
    }

    /// Return freshwater influence at a direction and its terrain height.
    /// Zero denotes dry terrain and one denotes saturated ground.
    pub fn freshwater_influence(&self, up: DVec3, ground: f64) -> f64 {
        let Some((face, uv)) = face_uv(up) else {
            return 0.0;
        };
        let river = self.inland.channel(up).map_or(0.0, |channel| {
            let exposed = ground - channel.stage;
            channel.weight * (1.0 - smoothstep(0.0, 2.8, exposed))
        });
        let lake = self.lake_level(up, face, uv).map_or_else(
            || self.lake_at(face, uv) * 0.38,
            |stage| 1.0 - smoothstep(0.1, 3.1, ground - stage),
        );
        river.max(lake)
    }

    /// Sample the highest lake, river or ocean surface covering this point.
    /// Supply the ground elevation from [Self::sample] to avoid resampling it.
    pub fn open_water_at(&self, direction: DVec3, ground: f64) -> Option<WaterSample> {
        let (face, uv) = face_uv(direction)?;
        let up = direction.normalize();
        let mut best = None;
        let mut consider = |elevation, current, kind| {
            let depth = elevation - ground;
            if depth > 0.012
                && best.is_none_or(|previous: WaterSample| previous.elevation < elevation)
            {
                best = Some(WaterSample {
                    elevation,
                    depth,
                    current,
                    kind,
                });
            }
        };
        if ground < WATER_SURFACE_ELEVATION && self.ocean_near(face, uv) {
            consider(WATER_SURFACE_ELEVATION, DVec3::ZERO, WaterKind::Ocean);
        }
        if let Some(stage) = self.lake_level(up, face, uv) {
            consider(stage, DVec3::ZERO, WaterKind::Lake);
        }
        if let Some(channel) = self
            .inland
            .channel(up)
            .filter(|channel| channel.distance <= 1.0)
        {
            consider(channel.stage, channel.current, WaterKind::River);
        }
        best
    }

    /// Sample the highest water surface covering this point, including local marsh pools.
    /// Supply the ground elevation from [Self::sample] to avoid resampling it.
    pub fn water_at(&self, direction: DVec3, ground: f64) -> Option<WaterSample> {
        let open_water = self.open_water_at(direction, ground);
        if open_water.is_some() || !(2.5..=904.0).contains(&ground) {
            return open_water;
        }
        let point = direction.normalize_or_zero() * RADIUS;
        let basin = Basin::at_cell((point / BASIN_CELL).floor().as_ivec3())?;
        if point.distance_squared(basin.center * RADIUS) > basin.radius * basin.radius {
            return None;
        }
        let pool = self.fill_basin(basin)?;
        let depth = pool.elevation - ground;
        (depth > 0.012).then_some(WaterSample {
            elevation: pool.elevation,
            depth,
            current: DVec3::ZERO,
            kind: WaterKind::Wetland,
        })
    }

    fn lake_level(&self, direction: DVec3, face: usize, uv: DVec2) -> Option<f64> {
        self.inland.lake_level(face, uv).or_else(|| {
            if uv.min(DVec2::ONE - uv).min_element() > 0.000_002 {
                return None;
            }
            seam_neighbors(direction, face)
                .find_map(|(face, uv, _)| self.inland.lake_level(face, uv))
        })
    }

    fn ocean_near(&self, face: usize, uv: DVec2) -> bool {
        let Some(mask) = &self.ocean else {
            return self.inland.lakes.is_empty();
        };
        let side = mask.width() / 3;
        let source = height_source_uv(uv, DVec2::splat(f64::from(side)));
        let x_offset = (face as u32 % 3) * side;
        let y_offset = (face as u32 / 3) * side;
        // Filter inside one cube face, exactly as the bathymetry shader does.
        // Older atlases used a lower intermediate value for inland basins.
        bilinear(source, side, side, |x, y| {
            f64::from(u8::from(
                mask.get_pixel(x + x_offset, y + y_offset).0[0] >= 238,
            ))
        }) >= 0.025
    }

    fn terrain_input(
        &self,
        direction: DVec3,
        face: usize,
        uv: DVec2,
        faces: &[HeightFace; 6],
    ) -> TerrainInput {
        let mut input = TerrainInput {
            base: sample_height(&faces[face], uv),
            river: 0.0,
            lake: 0.0,
        };
        let near_edge = uv.min(DVec2::ONE - uv).min_element() < FACE_BLEND_WIDTH;
        if input.base > 1.5 || near_edge {
            input.river = self.river_at(face, uv);
            input.lake = self.lake_at(face, uv);
        }
        if !near_edge {
            return input;
        }
        let mut weighted = input.base;
        let mut weight_sum = 1.0;
        let mut low = input.base;
        for (neighbor, uv, weight) in seam_neighbors(direction, face) {
            let base = sample_height(&faces[neighbor], uv);
            weighted += base * weight;
            weight_sum += weight;
            low = low.min(input.base + (base - input.base) * weight);
            // Keep the channel mask from whichever face carries the baked reach.
            input.river = input.river.max(self.river_at(neighbor, uv) * weight);
            input.lake = input.lake.max(self.lake_at(neighbor, uv) * weight);
        }
        let average = weighted / weight_sum;
        let wet = (input.river / 255.0).max(input.lake);
        input.base = average + (low - average) * wet;
        input
    }

    /// Find enclosed shallow pools with centers within `distance` meters of a direction.
    /// Intended for local streaming jobs. Search distance is capped at 512 meters.
    pub fn local_pools(&self, direction: DVec3, distance: f64) -> Vec<LocalPool> {
        if face_uv(direction).is_none()
            || !distance.is_finite()
            || distance <= 0.0
            || !self
                .faces
                .as_ref()
                .is_some_and(|f| f[0].width() == HEIGHT_MAP_SIZE)
        {
            return Vec::new();
        }
        let distance = distance.min(512.0);
        let point = direction.normalize() * RADIUS;
        let offset = DVec3::splat(distance);
        let first = ((point - offset) / BASIN_CELL).floor().as_ivec3();
        let last = ((point + offset) / BASIN_CELL).floor().as_ivec3();
        let mut pools = Vec::new();
        for z in first.z..=last.z {
            for y in first.y..=last.y {
                for x in first.x..=last.x {
                    let Some(basin) = Basin::at_cell(IVec3::new(x, y, z)) else {
                        continue;
                    };
                    if point.distance_squared(basin.center * RADIUS) <= distance * distance {
                        pools.extend(self.fill_basin(basin));
                    }
                }
            }
        }
        pools
    }

    fn fill_basin(&self, basin: Basin) -> Option<LocalPool> {
        let (face, uv) = face_uv(basin.center)?;
        let faces = self.faces.as_ref()?;
        let input = self.terrain_input(basin.center, face, uv, faces);
        let TerrainInput { base, river, lake } = input;
        if !(5.0..=900.0).contains(&base) || river > 85.0 || lake > 0.0 {
            return None;
        }
        let center_height = self.sample(basin.center)?;
        if self.open_water_at(basin.center, center_height).is_some() {
            return None;
        }
        let (temperature, moisture) = terrain_detail::climate(basin.center, center_height);
        let wet = terrain_detail::wetland_weight(
            temperature,
            moisture,
            center_height,
            river,
            basin.center * RADIUS,
        );
        if wet < 0.21 {
            return None;
        }
        let east = basin.center.any_orthonormal_vector();
        let north = basin.center.cross(east);
        let ring = |fraction: f64, count: u32| {
            (0..count).map(move |i| {
                let angle = std::f64::consts::TAU * f64::from(i) / f64::from(count);
                let (sin, cos) = angle.sin_cos();
                (basin.center + (east * cos + north * sin) * (fraction * basin.radius / RADIUS))
                    .normalize()
            })
        };
        let floor = ring(0.27, 8)
            .filter_map(|up| self.sample(up))
            .fold(center_height, f64::min);
        let mut rim = f64::INFINITY;
        for up in ring(1.0, 48) {
            let (face, uv) = face_uv(up)?;
            let input = self.terrain_input(up, face, uv, faces);
            let ground = self.sample(up)?;
            if input.lake > 0.0 || input.river > 100.0 || self.open_water_at(up, ground).is_some() {
                return None;
            }
            rim = rim.min(ground);
        }
        let elevation = (floor + 0.70).min(rim - 0.16);
        (elevation >= floor + 0.16).then_some(LocalPool {
            center: basin.center,
            radius: basin.radius,
            elevation,
        })
    }

    /// Interpolate only the pre-baked global terrain, without procedural detail.
    /// Hydrology still uses these values until the regional water rebuild.
    pub fn sample_base(&self, direction: DVec3) -> Option<f64> {
        let (face, uv) = face_uv(direction)?;
        Some(sample_height(&self.faces.as_ref()?[face], uv))
    }

    /// Borrow a complete legacy height face if it matches the requested square resolution.
    pub fn face_texels(&self, face: usize, side: u32) -> Option<&[u16]> {
        let image = self.faces.as_ref()?.get(face)?;
        (image.dimensions() == (side, side)).then_some(image.as_raw().as_slice())
    }

    /// Bilinearly sampled legacy river mask, in 0..=255 coverage units.
    /// Coverage tapers across cube-face edges to follow baked channels through seams.
    pub fn river_coverage(&self, direction: DVec3) -> f64 {
        face_uv(direction).map_or(0.0, |(face, uv)| {
            let coverage = self.river_at(face, uv);
            if uv.min(DVec2::ONE - uv).min_element() >= FACE_BLEND_WIDTH {
                return coverage;
            }
            seam_neighbors(direction, face).fold(coverage, |coverage, (face, uv, weight)| {
                coverage.max(self.river_at(face, uv) * weight)
            })
        })
    }

    fn river_at(&self, face: usize, uv: DVec2) -> f64 {
        let Some(map) = &self.rivers else {
            return 0.0;
        };
        let uv = height_source_uv(uv, DVec2::splat(f64::from(HEIGHT_MAP_SIZE)));
        let side = map.width() / 3;
        let x_offset = (face as u32 % 3) * side;
        let y_offset = (face as u32 / 3) * side;
        bilinear(uv, side, side, |x, y| {
            f64::from(map.get_pixel(x + x_offset, y + y_offset).0[0])
        })
    }

    fn lake_at(&self, face: usize, uv: DVec2) -> f64 {
        self.lakes[face].as_ref().map_or(0.0, |map| {
            let uv = height_source_uv(
                uv,
                DVec2::new(f64::from(map.width()), f64::from(map.height())),
            );
            bilinear(uv, map.width(), map.height(), |x, y| {
                f64::from(map.get_pixel(x, y).0[0]) / 255.0
            })
        })
    }

    /// Query river coverage at a terrain-aligned radial direction.
    pub fn is_river(&self, direction: DVec3) -> bool {
        self.river_coverage(direction) >= 156.0
    }
}

fn seam_neighbors(direction: DVec3, primary: usize) -> impl Iterator<Item = (usize, DVec2, f64)> {
    let distance = direction.abs();
    let relative = distance / distance.max_element();
    [
        (if direction.x < 0.0 { 0 } else { 3 }, relative.x),
        (if direction.y < 0.0 { 5 } else { 2 }, relative.y),
        (if direction.z < 0.0 { 4 } else { 1 }, relative.z),
    ]
    .into_iter()
    .filter_map(move |(face, axis)| {
        let t = 1.0 - (1.0 - axis) / FACE_BLEND_WIDTH;
        (face != primary && t > 0.0).then(|| {
            let uv = DVec2::from_array(source_face_uv(face, direction.to_array()));
            (
                face,
                uv.clamp(DVec2::ZERO, DVec2::ONE),
                t * t * (3.0 - 2.0 * t),
            )
        })
    })
}

fn face_uv(direction: DVec3) -> Option<(usize, DVec2)> {
    (direction != DVec3::ZERO && direction.is_finite()).then(|| surface_uv(direction))
}

fn sample_height(image: &HeightFace, uv: DVec2) -> f64 {
    let dimensions = DVec2::new(f64::from(image.width()), f64::from(image.height()));
    let uv = height_source_uv(uv, dimensions);
    decode_height(bilinear(uv, image.width(), image.height(), |x, y| {
        f64::from(image.get_pixel(x, y).0[0])
    }))
}

fn bilinear(uv: DVec2, width: u32, height: u32, sample: impl Fn(u32, u32) -> f64) -> f64 {
    let last_x = width - 1;
    let last_y = height - 1;
    let pixel = uv * DVec2::new(f64::from(last_x), f64::from(last_y));
    let x = pixel.x.floor() as u32;
    let y = pixel.y.floor() as u32;
    let right = (x + 1).min(last_x);
    let down = (y + 1).min(last_y);
    let top_left = sample(x, y);
    let bottom_left = sample(x, down);
    let dx = pixel.x.fract();
    let top = top_left + (sample(right, y) - top_left) * dx;
    let bottom = bottom_left + (sample(right, down) - bottom_left) * dx;
    top + (bottom - top) * pixel.y.fract()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinates::terrain_aligned_direction;
    use std::sync::OnceLock;

    fn shipped_terrain() -> &'static SurfaceHeights {
        static HEIGHTS: OnceLock<SurfaceHeights> = OnceLock::new();
        HEIGHTS.get_or_init(|| {
            let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
            SurfaceHeights::load(&assets)
        })
    }

    fn coast() -> DVec3 {
        terrain_aligned_direction(DVec3::new(-0.97685, 0.21339, 0.01513))
    }

    #[test]
    fn height_and_river_queries_use_the_same_cube_face() {
        let mut rivers = image::GrayImage::new(6, 4);
        for y in 2..4 {
            for x in 0..2 {
                rivers.put_pixel(x, y, image::Luma([255]));
            }
        }
        let heights = SurfaceHeights {
            faces: Some(Arc::new(std::array::from_fn(|face| {
                HeightFace::from_pixel(2, 2, image::Luma([20_000 + face as u16]))
            }))),
            rivers: Some(Arc::new(rivers)),
            lakes: Default::default(),
            inland: Default::default(),
            ocean: Default::default(),
        };

        assert_eq!(heights.sample(DVec3::X), Some(decode_height(20_003.0)));
        assert_eq!(heights.sample(DVec3::NEG_X), Some(decode_height(20_000.0)));
        assert_eq!(heights.sample(DVec3::ZERO), None);
        assert!(heights.is_river(DVec3::X));
        assert!(!heights.is_river(DVec3::NEG_X));
    }

    #[test]
    fn shipped_river_stage_and_current_match_the_graded_channel() {
        let heights = shipped_terrain();
        let reach = heights
            .river_reaches()
            .iter()
            .find(|reach| {
                let up = reach.direction(0.49, 0.0);
                let ground = heights.sample(up).unwrap();
                heights
                    .water_at(up, ground)
                    .is_some_and(|water| water.kind == WaterKind::River)
                    && reach.half_width(0.49) > 20.0
                    && reach.elevation(0.49) > 4.0
            })
            .expect("an exposed river reach");
        let up = reach.direction(0.49, 0.0);
        let ground = heights.sample(up).unwrap();
        let water = heights.water_at(up, ground).unwrap();
        assert!(water.depth > 0.5 && water.depth < 6.0);
        assert!(water.current.dot(reach.direction(0.51, 0.0) - up) > 0.0);
        assert!(water.current.dot(up).abs() < 1e-10);
        assert!(reach.elevation(1.0) <= reach.elevation(0.0));
        let bank_up = reach.direction(0.49, 1.0 + 8.0 / reach.half_width(0.49));
        let bank = heights.sample(bank_up).unwrap();
        assert!((bank - water.elevation).abs() < 6.0);
    }

    #[test]
    fn shipped_lake_query_uses_the_baked_surface_level() {
        let heights = shipped_terrain();
        let (tile, water) = heights
            .lake_tiles()
            .iter()
            .find_map(|tile| {
                let up = tile.corners.iter().sum::<DVec3>().normalize();
                let ground = heights.sample(up)?;
                let water = heights.water_at(up, ground)?;
                (water.kind == WaterKind::Lake).then_some((tile, water))
            })
            .expect("a submerged lake tile");
        assert!((water.elevation - tile.elevation).abs() < 0.001);
        assert_eq!(water.current, DVec3::ZERO);
        assert!(water.depth > 0.0);
    }

    #[test]
    fn shipped_coast_has_sub_texel_relief() {
        let heights = shipped_terrain();
        let center = coast();
        let east = center.any_orthonormal_vector();
        let north = center.cross(east);
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for x in -4..=4 {
            for y in -4..=4 {
                let up = (center + (east * f64::from(x) + north * f64::from(y)) * (12.0 / RADIUS))
                    .normalize();
                let (face, uv) = face_uv(up).unwrap();
                let base = heights.sample_base(up).unwrap();
                let before =
                    base + terrain_detail::detail(up, base, heights.river_at(face, uv), 1.0);
                let change = heights.sample(up).unwrap() - before.max(1.5);
                minimum = minimum.min(change);
                maximum = maximum.max(change);
            }
        }
        assert!(
            maximum - minimum > 1.0,
            "coastal relief: {minimum} to {maximum} m"
        );
    }

    #[test]
    fn wetland_pools_have_a_dry_rim_and_deterministic_water_level() {
        let heights = shipped_terrain();
        let direction = coast();
        let pools = heights.local_pools(direction, 240.0);
        assert!(!pools.is_empty());
        assert_eq!(pools, heights.local_pools(direction, 240.0));
        for pool in pools {
            let east = pool.center.any_orthonormal_vector();
            let north = pool.center.cross(east);
            let rim = (0..96)
                .map(|i| {
                    let angle = std::f64::consts::TAU * f64::from(i) / 96.0;
                    let up = (pool.center
                        + (east * angle.cos() + north * angle.sin()) * (pool.radius / RADIUS))
                        .normalize();
                    heights.sample(up).unwrap()
                })
                .fold(f64::INFINITY, f64::min);
            assert!(pool.elevation < rim, "pool exceeds its rim: {pool:?}");
        }
    }

    #[test]
    fn elevation_is_continuous_across_baked_face_height_and_river_disagreements() {
        let heights = shipped_terrain();
        for (edge, axis_a, axis_b) in [
            (DVec3::new(-1.0, -0.5875, 1.0), DVec3::NEG_X, DVec3::Z),
            (DVec3::new(0.7375, -1.0, -1.0), DVec3::NEG_Y, DVec3::NEG_Z),
        ] {
            let a = edge + axis_a * 1e-8;
            let b = edge + axis_b * 1e-8;
            let delta = heights.sample(a).unwrap() - heights.sample(b).unwrap();
            assert!(
                delta.abs() < 0.1,
                "height changes by {delta} m across {edge:?}"
            );
            assert!((heights.river_coverage(a) - heights.river_coverage(b)).abs() < 0.05);
        }
    }
}
