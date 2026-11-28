//! Static canopy shadow baking from Igor_renderer's 1 m alpha-tested point templates.
use crate::{
    env,
    trees::{pack_normal, TreeGeometry},
    Placed,
};
use bytemuck::{Pod, Zeroable};
use glam::{UVec3, Vec2, Vec3};
use std::collections::HashMap;
const LEVELS: usize = 22;
// Units are template-local meters. Densify the mid-field only; keep
// the original near levels and distant 4/8/16 m hierarchy.
const LOCAL_WIDTHS: [f32; LEVELS] = [
    0.0, 0.0625, 0.09375, 0.125, 0.1875, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5,
    1.75, 2.0, 2.5, 3.0, 3.5, 4.0, 8.0, 16.0,
];
const ORIGINAL_LEVELS: [usize; 8] = [0, 5, 7, 11, 15, 19, 20, 21];
const DENSE_LEVELS: [usize; 15] = [0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 15, 19, 20, 21];
const EXTRA_LEVELS: [usize; 7] = [1, 2, 3, 4, 6, 9, 13];
const MID_LEVELS: [usize; 7] = [8, 10, 12, 14, 16, 17, 18];

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(crate) struct Point {
    pub xy: u32,
    pub zr: u32,
    pub rgba: u32,
    pub normal: u32,
}
impl Point {
    fn encode(p: Vec3, r: f32, c: Vec3, n: Vec3, coverage: f32) -> Self {
        let q = (p.clamp(Vec3::ZERO, Vec3::splat(15.9997)) * 4096.0)
            .round()
            .as_uvec3();
        let r = (r * 4096.0).ceil().clamp(1.0, 65535.0) as u32;
        let rgb = c
            .clamp(Vec3::ZERO, Vec3::ONE)
            .to_array()
            .map(|v| (v * 255.0 + 0.5) as u32);
        let alpha = (coverage.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        Self {
            xy: q.x | (q.y << 16),
            zr: q.z | (r << 16),
            rgba: rgb[0] | (rgb[1] << 8) | (rgb[2] << 16) | (alpha << 24),
            normal: pack_normal(n.normalize_or(Vec3::Y)),
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct Aggregate {
    weighted_pos: Vec3,
    weighted_color: Vec3,
    weighted_normal: Vec3,
    area: f32,
    lo: Vec3,
    hi: Vec3,
}
#[derive(Clone, Copy, Debug)]
struct Sample {
    pos: Vec3,
    color: Vec3,
    normal: Vec3,
    radius: f32,
    coverage: f32,
}
impl Aggregate {
    fn add(&mut self, s: Sample) {
        let area = (s.radius * s.radius * s.coverage).max(1e-8);
        let lo = s.pos - Vec3::splat(s.radius);
        let hi = s.pos + Vec3::splat(s.radius);
        if self.area == 0.0 {
            self.lo = lo;
            self.hi = hi;
        } else {
            self.lo = self.lo.min(lo);
            self.hi = self.hi.max(hi);
        }
        self.weighted_pos += s.pos * area;
        self.weighted_color += s.color * area;
        // Opposite-facing surfaces can cancel; normalize only after aggregation.
        self.weighted_normal += s.normal * area;
        self.area += area;
    }
    fn sample(self) -> Option<Sample> {
        if self.area == 0.0 {
            return None;
        }
        let p = self.weighted_pos / self.area;
        let extent = (p - self.lo).max(self.hi - p);
        let bound = extent.length().max(0.001);
        // A representative disk is not an opaque cube. Keep an estimate of
        // covered area separately; radius/coverage are approximate at each LOD.
        let radius = bound
            .min(self.area.sqrt().max(extent.max_element() * 0.45))
            .max(0.001);
        Some(Sample {
            pos: p,
            color: self.weighted_color / self.area,
            normal: self.weighted_normal.normalize_or(Vec3::Y),
            radius,
            coverage: (self.area / (radius * radius)).clamp(0.02, 1.0),
        })
    }
}
#[derive(Clone, Copy, Debug, Default)]
struct Range {
    pub offset: u32,
    pub count: u32,
}
fn point_position(p: &Point) -> Vec3 {
    Vec3::new(
        (p.xy & 65535) as f32,
        (p.xy >> 16) as f32,
        (p.zr & 65535) as f32,
    ) / 4096.0
}
fn point_radius(p: &Point) -> f32 {
    (p.zr >> 16) as f32 / 4096.0
}
/// Static directional shadow field, indexed by projection onto y=0.
/// RGBA channels represent four height bands in the streamed region.
pub(crate) struct ShadowField {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pub transform: [f32; 4],
    pub projection: Vec2,
    pub band_base: f32,
    pub band_height: f32,
    pub sun_direction: Vec3,
}
const SHADOW_PAD: f32 = 24.0;

fn project(p: Vec3, shear: Vec2, base: f32) -> Vec2 {
    Vec2::new(p.x, p.z) + (p.y - base) * shear
}

fn add_shadow_sample(
    pixels: &mut [u8],
    w: u32,
    h: u32,
    step: f32,
    texel: Vec2,
    band: usize,
    radius: f32,
    coverage: f32,
) {
    if coverage <= 0.0 {
        return;
    }
    let x = texel.x;
    let z = texel.y;
    let r = (radius * 1.35 / step).max(0.03);
    let span = r + 0.75;
    let x0 = ((x - span).floor() as i32).max(0);
    let z0 = ((z - span).floor() as i32).max(0);
    let x1 = ((x + span).ceil() as i32).min(w as i32 - 1);
    let z1 = ((z + span).ceil() as i32).min(h as i32 - 1);
    // Area-aware soft footprint, accumulated as approximate opacity.
    let opacity = (coverage * r * r * 0.80).min(0.80);
    for iz in z0..=z1 {
        for ix in x0..=x1 {
            let d = ((ix as f32 - x).powi(2) + (iz as f32 - z).powi(2)).sqrt();
            let a = (1.0 - d / span).clamp(0.0, 1.0) * opacity;
            if a <= 0.0 {
                continue;
            }
            let i = (iz as usize * w as usize + ix as usize) * 4 + band;
            let old = f32::from(pixels[i]) / 255.0;
            pixels[i] = ((old + (1.0 - old) * a) * 255.0 + 0.5) as u8;
        }
    }
}
fn append_sparse_level(samples: &[Sample], width: f32, lod: &mut Vec<Point>) -> Range {
    let side = (16.0 / width).ceil() as u32;
    let mut cells = HashMap::<usize, Aggregate>::with_capacity(samples.len().min(65536));
    for &s in samples {
        cells
            .entry(cell_index(s.pos, width, side))
            .or_default()
            .add(s);
    }
    let mut cells: Vec<_> = cells.into_iter().collect();
    cells.sort_unstable_by_key(|(key, _)| *key);
    let start = lod.len() as u32;
    for (_, a) in cells {
        if let Some(s) = a.sample() {
            lod.push(Point::encode(
                s.pos, s.radius, s.color, s.normal, s.coverage,
            ));
        }
    }
    Range {
        offset: start,
        count: lod.len() as u32 - start,
    }
}
fn cell_index(p: Vec3, width: f32, side: u32) -> usize {
    let c = (p / width).floor().as_uvec3().min(UVec3::splat(side - 1));
    (c.x + side * (c.y + side * c.z)) as usize
}

pub(crate) fn template(geo: &TreeGeometry) -> Vec<Point> {
    let mut lod = Vec::new();
    let dense_lod = std::env::var("IGOR_DENSE_LOD").as_deref() != Ok("0");
    let mid_lod = dense_lod && std::env::var("IGOR_MID_LOD").as_deref() != Ok("0");
    let local_levels: Vec<usize> = if mid_lod {
        (0..LEVELS).collect()
    } else if dense_lod {
        DENSE_LEVELS.to_vec()
    } else {
        ORIGINAL_LEVELS.to_vec()
    };
    let mut ranges = [Range::default(); LEVELS];
    // Dense direct-address grid of 0.25 m, per template (64^3).
    // Coarser levels are reductions of it, not resampled ray images.
    let mut side = 64u32;
    let mut grid = vec![Aggregate::default(); (side * side * side) as usize];
    let mut samples = Vec::new();
    for s in &geo.surfels {
        let (center, r) = s.center_radius();
        let patch = &geo.patches[s.patch as usize];
        let n = Vec3::from_slice(&patch.plane).normalize_or(Vec3::Y);
        let tangent = n
            .cross(if n.y.abs() < 0.9 { Vec3::Y } else { Vec3::X })
            .normalize_or(Vec3::X);
        let bitangent = n.cross(tangent);
        // Only the coarser hierarchy uses sampled color and coverage.
        // Multiple samples retain more leaf detail than one center tap.
        let mut pos_sum = Vec3::ZERO;
        let mut color_sum = Vec3::ZERO;
        let mut normal_sum = Vec3::ZERO;
        let mut count = 0u32;
        for pos in [
            center,
            center + tangent * r * 0.35,
            center - tangent * r * 0.35,
            center + bitangent * r * 0.35,
            center - bitangent * r * 0.35,
        ] {
            let b = patch.coordinates(pos);
            if b.min_element() >= -0.0001
                && b.element_sum() <= 1.0001
                && geo.alpha_visible(patch, b)
            {
                let (color, normal) = geo.surface(patch, b);
                pos_sum += pos;
                color_sum += color;
                normal_sum += normal;
                count += 1;
            }
        }
        if count > 0 {
            let inv = 1.0 / count as f32;
            let sample = Sample {
                pos: pos_sum * inv,
                color: color_sum * inv,
                normal: normal_sum.normalize_or(n),
                radius: r,
                coverage: count as f32 / 5.0,
            };
            grid[cell_index(sample.pos, 0.25, side)].add(sample);
            if dense_lod {
                samples.push(sample);
            }
        }
    }
    for &level in &ORIGINAL_LEVELS[1..] {
        let start = lod.len() as u32;
        for a in &grid {
            if let Some(s) = a.sample() {
                lod.push(Point::encode(
                    s.pos, s.radius, s.color, s.normal, s.coverage,
                ));
            }
        }
        ranges[level] = Range {
            offset: start,
            count: lod.len() as u32 - start,
        };
        if side > 1 {
            let half = side / 2;
            let mut parent = vec![Aggregate::default(); (half * half * half) as usize];
            for (i, a) in std::mem::take(&mut grid).into_iter().enumerate() {
                if a.area == 0.0 {
                    continue;
                }
                let i = i as u32;
                let x = i % side;
                let y = i / side % side;
                let z = i / (side * side);
                let j = (x / 2 + half * (y / 2 + half * (z / 2))) as usize;
                let dst = &mut parent[j];
                // Exact reduction of weighted sums and bounds.
                if dst.area == 0.0 {
                    *dst = a;
                } else {
                    dst.weighted_pos += a.weighted_pos;
                    dst.weighted_color += a.weighted_color;
                    dst.weighted_normal += a.weighted_normal;
                    dst.area += a.area;
                    dst.lo = dst.lo.min(a.lo);
                    dst.hi = dst.hi.max(a.hi);
                }
            }
            grid = parent;
            side = half;
        }
    }
    if dense_lod {
        for level in EXTRA_LEVELS {
            ranges[level] = append_sparse_level(&samples, LOCAL_WIDTHS[level], &mut lod);
        }
        if mid_lod {
            for level in MID_LEVELS {
                ranges[level] = append_sparse_level(&samples, LOCAL_WIDTHS[level], &mut lod);
            }
        }
        // Independent grids need not be nested. Repair inversions
        // only between active levels, preserving the 15-level
        // baseline when mid LOD is disabled. Never alias raw.
        for pair in local_levels[1..].windows(2).rev() {
            let fine = pair[0];
            let coarse = pair[1];
            if ranges[fine].count < ranges[coarse].count {
                ranges[fine] = ranges[coarse];
            }
        }
    }
    let level = LOCAL_WIDTHS.iter().position(|&x| x == 1.0).unwrap();
    let range = ranges[level];
    let start = range.offset as usize;
    lod[start..start + range.count as usize].to_vec()
}

pub(crate) fn field(
    placed: &[Placed],
    templates: &[Vec<Point>],
    bounds: crate::Bounds,
    sun_direction: Vec3,
) -> ShadowField {
    // Limit the projection footprint for very low sun. The shader shades out
    // direct light as it approaches the horizon.
    let projection = -Vec2::new(sun_direction.x, sun_direction.z) / sun_direction.y.max(0.25);
    let band_base = bounds.lo.y;
    let band_height = ((bounds.hi.y - band_base) / 4.0).max(4.0);
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for p in placed.iter().flat_map(|tree| tree.bounds.corners()) {
        let point = project(p, projection, band_base);
        min = min.min(point);
        max = max.max(point);
    }
    let extent = max - min;
    let max_dim = 2048;
    let requested_step = env("IGOR_SHADOW_METERS", 0.75f32).clamp(0.5, 8.0);
    let step = requested_step.max((extent.max_element() + 2.0 * SHADOW_PAD) / max_dim as f32);
    // Anchor the texel grid to the same origin regardless of patch contents.
    let origin = (min / step).floor() * step;
    let w = (((max.x - origin.x + 2.0 * SHADOW_PAD) / step).ceil() as u32).min(max_dim);
    let h = (((max.y - origin.y + 2.0 * SHADOW_PAD) / step).ceil() as u32).min(max_dim);
    let mut pixels = vec![0u8; w as usize * h as usize * 4];
    if sun_direction.y > 0.0 {
        for tree in placed {
            for pt in &templates[tree.model] {
                let model_position = point_position(pt);
                if model_position.y * tree.instance.translation[3] <= 0.12 {
                    continue;
                }
                let p = tree.instance.world(model_position);
                let texel = (project(p, projection, band_base) - origin + SHADOW_PAD) / step - 0.5;
                let band = ((p.y - band_base) / band_height).clamp(0.0, 3.0) as usize;
                let r = (point_radius(pt) * tree.instance.translation[3]).min(2.5);
                let a = (pt.rgba >> 24) as f32 / 255.0;
                add_shadow_sample(&mut pixels, w, h, step, texel, band, r, a);
            }
        }
    }
    ShadowField {
        width: w,
        height: h,
        pixels,
        transform: [
            SHADOW_PAD - origin.x,
            SHADOW_PAD - origin.y,
            1.0 / (w as f32 * step),
            1.0 / (h as f32 * step),
        ],
        projection,
        band_base,
        band_height,
        sun_direction,
    }
}
