//! Shared sampling of baked inland water in planet-centered coordinates.

use std::{collections::HashMap, fs, path::Path};

use bevy_math::{DVec2, DVec3, IVec3};

use crate::{
    coordinates::{height_source_uv, surface_uv, terrain_aligned_direction},
    planet::{
        BAKED_WATER_MESH_HEADER, HEIGHT_MAP_SIZE, LAKE_SURFACE_PATH, LEGACY_BAKED_RADIUS, RADIUS,
        RIVER_REACH_VERTICES, RIVER_SURFACE_PATH,
    },
};

const BIN_SIZE: f64 = 8192.0;
const VALLEY_RANGE: f64 = 2800.0;
const LAKE_SIDE: usize = HEIGHT_MAP_SIZE as usize;

/// Classification of a water surface encountered on the planet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaterKind {
    /// Connected saltwater at the planet's sea level.
    Ocean,
    /// A graded channel with a downstream current.
    River,
    /// An inland basin with a common spill elevation.
    Lake,
    /// A shallow, sheltered pool within a wetland.
    Wetland,
}

/// A water surface at a radial direction, with velocities in planet coordinates.
#[derive(Clone, Copy, Debug)]
pub struct WaterSample {
    /// Surface elevation in meters above the reference sphere.
    pub elevation: f64,
    /// Vertical distance from surface to terrain in meters.
    pub depth: f64,
    /// Tangential current in meters per second.
    pub current: DVec3,
    /// Hydrological classification of the surface.
    pub kind: WaterKind,
}

/// A directed reach of the baked channel network.
#[derive(Clone, Copy, Debug)]
pub struct RiverReach {
    start: DVec3,
    end: DVec3,
    axis: DVec3,
    width: DVec2,
    stage: DVec2,
    length: f64,
    bend: f64,
}

impl RiverReach {
    fn from_vertices(vertices: &[DVec3]) -> Option<Self> {
        let start = vertices[6].normalize();
        let end = (vertices[1].normalize() + vertices[5].normalize()).normalize();
        let length = start.distance(end) * RADIUS;
        if length < 0.5 {
            return None;
        }
        let width = DVec2::new(
            vertices[0].distance(vertices[2]) * 0.5,
            vertices[1].distance(vertices[5]) * 0.5,
        );
        // Vary bend handedness between reaches. Zero displacement at both
        // junctions keeps tributaries and cube-face crossings connected.
        let bend = (start.dot(DVec3::new(9219.72, -25714.54, 12719.06))).sin()
            * (length * 0.035).min(width.min_element() * 0.28).min(95.0);
        Some(Self {
            start,
            end,
            axis: start.cross(end).normalize(),
            width,
            stage: DVec2::new(
                vertices[6].length() - RADIUS,
                ((vertices[1].length() + vertices[5].length()) * 0.5 - RADIUS)
                    .min(vertices[6].length() - RADIUS),
            ),
            length,
            bend,
        })
    }

    /// Centerline length in meters.
    pub fn length(&self) -> f64 {
        self.length
    }

    /// Unit direction on the reach at fraction `t` and a signed bank offset.
    /// An offset of -1 or 1 follows the nominal left or right bank.
    pub fn direction(&self, t: f64, bank: f64) -> DVec3 {
        let lateral = self.offset(t) + bank * self.half_width(t);
        (self.start.lerp(self.end, t) + self.axis * (lateral / RADIUS)).normalize()
    }

    /// Half width of the ribbon at fraction `t`, in meters.
    pub fn half_width(&self, t: f64) -> f64 {
        let width = self.width.x + (self.width.y - self.width.x) * t;
        let variation = (t * std::f64::consts::PI).sin()
            * (t * std::f64::consts::TAU * 2.0 + self.bend * 0.07).sin();
        width + variation * (width * 0.12).min(45.0)
    }

    /// Unit normal to the centerline plane, pointing across the river.
    pub fn across(&self) -> DVec3 {
        self.axis
    }

    /// Surface elevation at fraction `t` of the reach.
    pub fn elevation(&self, t: f64) -> f64 {
        self.stage.x + (self.stage.y - self.stage.x) * t
    }

    /// Tangent current at this direction, in meters per second.
    pub fn current(&self, up: DVec3) -> DVec3 {
        let grade = ((self.stage.x - self.stage.y) / self.length).max(0.0);
        self.axis.cross(up) * (0.48 + 15.0 * grade.sqrt()).min(3.6)
    }

    fn offset(&self, t: f64) -> f64 {
        self.bend
            * (t * std::f64::consts::PI).sin()
            * (t * std::f64::consts::TAU + self.bend * 0.04).cos()
    }

    fn sample(&self, up: DVec3, point: DVec3) -> Channel {
        let span = self.end - self.start;
        let t = ((up - self.start).dot(span) / span.length_squared()).clamp(0.0, 1.0);
        let center = self.direction(t, 0.0);
        let signed = self.axis.dot(point) - self.offset(t);
        let distance = point.distance(center * RADIUS);
        let width = self.half_width(t);
        let shore = (width * 0.43).clamp(9.0, 100.0);
        let stage = self.elevation(t);
        let weight = 1.0 - smoothstep(0.0, shore * 2.3, distance - width);
        if weight == 0.0 {
            return Channel {
                stage,
                current: DVec3::ZERO,
                distance: distance - width,
                bottom: stage,
                weight,
                bank: shore,
            };
        }
        let along = t * self.length;
        // Lateral bars create shoals inside the nominal ribbon, where the
        // terrain clips the water. Taper them out at shared reach endpoints.
        let ends = smoothstep(0.0, 95.0, along.min(self.length - along));
        let phase = along * 0.049 + self.bend * 0.3 + if signed < 0.0 { 2.1 } else { 0.0 };
        let bar_width = (width * 0.18).min(9.0)
            * ends
            * smoothstep(-0.55, 0.82, phase.sin() + (phase * 2.17).sin() * 0.29);
        let effective_width = width - bar_width;
        let normalized = distance / effective_width;
        // The low-flow bed is asymmetric. A broad bar fades into a deeper
        // outer bank, with rounded toes at the active waterline.
        let side = (signed / width).clamp(-1.0, 1.0);
        let bar = (1.0 + side * (self.bend * 0.08).tanh() * 0.30)
            * (0.75 + (width / 78.0).sqrt()).min(4.8);
        let profile = if distance < effective_width {
            -bar * (1.0 - smoothstep(0.13, 1.0, normalized)) - 0.04
        } else {
            1.55 * smoothstep(0.0, shore, distance - effective_width) - 0.04
        };
        Channel {
            stage,
            current: self.current(up),
            distance: distance - width,
            bottom: stage + profile,
            weight,
            bank: shore,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Channel {
    pub stage: f64,
    pub current: DVec3,
    pub distance: f64,
    pub bottom: f64,
    pub weight: f64,
    bank: f64,
}

impl Channel {
    fn cut_valley(self, ground: f64) -> f64 {
        let distance = self.distance.max(0.0);
        // Give the channel a valley before adding the inner banks. Very small
        // baked ribbons can cross kilometre-scale procedural mountain detail.
        // The graded floodplain expands into that relief over physical meters.
        let valley = 1.0 - smoothstep(260.0, VALLEY_RANGE, distance);
        let terrace = distance * (0.12 + 0.34 * smoothstep(85.0, 1050.0, distance));
        let cut = ground.min(self.stage + 2.15 + terrace);
        ground + (cut - ground) * valley
    }

    fn bed(self, up: DVec3, ground: f64) -> f64 {
        let distance = self.distance.max(0.0);
        let toe = self.stage + 0.28 + smoothstep(0.0, 65.0, distance);
        let ground = ground
            + (toe - ground).max(0.0) * (1.0 - smoothstep(self.bank, self.bank * 2.3, distance));
        let point = up * RADIUS;
        let deposit = crate::terrain_detail::value_noise(point / 29.0, 0x9707_A4C3);
        let gravel = crate::terrain_detail::value_noise(point / 4.4, 0x7823_5F8C);
        let relief =
            (deposit * 0.30 + gravel * 0.13) * (1.0 - smoothstep(0.0, self.bank, distance));
        ground + (self.bottom + relief - ground) * self.weight
    }
}

/// One baked lake tile at its basin's spill level.
#[derive(Clone, Copy, Debug)]
pub struct LakeTile {
    /// Four corners in perimeter order, as radial unit directions.
    pub corners: [DVec3; 4],
    /// Flat water elevation in meters above the reference sphere.
    pub elevation: f64,
}

#[derive(Default)]
pub(crate) struct InlandWater {
    pub rivers: Vec<RiverReach>,
    bins: HashMap<IVec3, Vec<u32>>,
    pub lakes: Vec<LakeTile>,
    lake_levels: [Option<Vec<f32>>; 6],
}

impl InlandWater {
    pub(crate) fn load(assets: &Path) -> Self {
        let mut result = Self::default();
        for_each_record(
            &assets.join(RIVER_SURFACE_PATH),
            |vertices: [DVec3; RIVER_REACH_VERTICES]| {
                if let Some(reach) = RiverReach::from_vertices(&vertices) {
                    result.add_reach(reach);
                }
            },
        );
        for_each_record(&assets.join(LAKE_SURFACE_PATH), |vertices: [DVec3; 6]| {
            let order = if vertices[2] == vertices[4] {
                [0, 1, 2, 5]
            } else {
                [0, 2, 1, 4]
            };
            let corners = order.map(|i| vertices[i].normalize());
            let elevation = vertices.iter().map(|p| p.length() - RADIUS).sum::<f64>() / 6.0;
            let (face, uv) = surface_uv(corners.iter().sum::<DVec3>());
            let index = lake_pixel(uv).round().as_uvec2();
            let levels = result.lake_levels[face]
                .get_or_insert_with(|| vec![f32::NAN; LAKE_SIDE * LAKE_SIDE]);
            levels[index.y as usize * LAKE_SIDE + index.x as usize] = elevation as f32;
            result.lakes.push(LakeTile { corners, elevation });
        });
        result
    }

    fn add_reach(&mut self, reach: RiverReach) {
        let margin = reach.width.max_element() * 1.18 + reach.bend.abs() + VALLEY_RANGE + 30.0;
        let first = bin(reach.start.min(reach.end) * RADIUS - DVec3::splat(margin));
        let last = bin(reach.start.max(reach.end) * RADIUS + DVec3::splat(margin));
        let key = self.rivers.len() as u32;
        for z in first.z..=last.z {
            for y in first.y..=last.y {
                for x in first.x..=last.x {
                    self.bins.entry(IVec3::new(x, y, z)).or_default().push(key);
                }
            }
        }
        self.rivers.push(reach);
    }

    fn channels_at(&self, up: DVec3) -> impl Iterator<Item = Channel> + '_ {
        let point = up * RADIUS;
        self.bins
            .get(&bin(point))
            .into_iter()
            .flatten()
            .map(move |&index| self.rivers[index as usize].sample(up, point))
            .filter(|sample| sample.distance < VALLEY_RANGE)
    }

    pub(crate) fn channel(&self, up: DVec3) -> Option<Channel> {
        self.channels_at(up)
            .min_by(|a, b| a.distance.total_cmp(&b.distance))
    }

    pub(crate) fn grade_ground(&self, up: DVec3, ground: f64, lake: Option<f64>) -> f64 {
        let mut cut = ground;
        let mut bed = None;
        for channel in self.channels_at(up) {
            // Leave submerged lake deltas at the basin's own level.
            if lake.is_some_and(|stage| stage >= channel.stage - 0.08 && ground < stage) {
                continue;
            }
            cut = cut.min(channel.cut_valley(ground));
            if channel.weight > 0.0
                && bed.is_none_or(|old: Channel| channel.distance < old.distance)
            {
                bed = Some(channel);
            }
        }
        bed.map_or(cut, |channel| channel.bed(up, cut))
    }

    pub(crate) fn lake_level(&self, face: usize, uv: DVec2) -> Option<f64> {
        let levels = self.lake_levels[face].as_ref()?;
        let index = lake_pixel(uv).round().as_uvec2();
        let level = levels[index.y as usize * LAKE_SIDE + index.x as usize];
        level.is_finite().then_some(f64::from(level))
    }
}

fn lake_pixel(uv: DVec2) -> DVec2 {
    height_source_uv(uv, DVec2::splat(LAKE_SIDE as f64)) * (LAKE_SIDE - 1) as f64
}

fn bin(point: DVec3) -> IVec3 {
    (point / BIN_SIZE).floor().as_ivec3()
}

fn for_each_record<const N: usize>(path: &Path, mut use_record: impl FnMut([DVec3; N])) {
    let Ok(source) = fs::read(path) else {
        return;
    };
    let baked = source.strip_prefix(BAKED_WATER_MESH_HEADER);
    let bytes = baked.unwrap_or(&source);
    if bytes.len() % (12 * N) != 0 {
        return;
    }
    for record in bytes.chunks_exact(12 * N) {
        let vertices = std::array::from_fn(|i| {
            let value = |j: usize| {
                let k = i * 12 + j * 4;
                f64::from(f32::from_le_bytes(record[k..k + 4].try_into().unwrap()))
            };
            let position = DVec3::new(value(0), value(1), value(2));
            let radius = position.length();
            let up = position / radius;
            let direction = if baked.is_some() {
                up
            } else {
                terrain_aligned_direction(up)
            };
            let elevation = if (radius - LEGACY_BAKED_RADIUS).abs() < 30_000.0 {
                radius - LEGACY_BAKED_RADIUS
            } else {
                radius - RADIUS
            };
            direction * (RADIUS + elevation)
        });
        if vertices.iter().all(|p| p.is_finite()) {
            use_record(vertices);
        }
    }
}

pub(crate) fn smoothstep(low: f64, high: f64, value: f64) -> f64 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
