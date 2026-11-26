use bytemuck::{Pod, Zeroable};
use glam::{IVec3, UVec3, Vec3};
use std::{collections::VecDeque, path::Path, sync::Arc};

mod tree_texture;
mod trees;
pub mod splat;
pub mod render_api;
pub mod planetary;
mod planetary_shader;
#[cfg(feature = "gpu")]
pub mod compute_splat;
#[cfg(feature = "gpu")]
pub use wgpu;
pub use glam;
use trees::{bin_index, BINS_PER_TREE, BIN_SIDE, BIN_WIDTH, BIN_COUNT, ACCEL_STRIDE, PROXY_SIDE};
pub use trees::{Batch, Material, Patch, Surfel, TreeGeometry};

pub const CELL: f32 = 16.0;
pub const SCALES: [u32; 3] = [1, 4, 16];
pub const DEFAULT_SIDE: u32 = 512;
pub const DEFAULT_DENSITY: u32 = 95;
pub const TREE_ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/trees");

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, Pod, Zeroable)]
pub struct Block {
    // Zero = empty. Lower and upper 16 bits hold the canopy and understory
    // instances. Each instance stores (template index * 4 + quarter-turn) + 1.
    pub batch: u32,
    // Chebyshev distance in THIS level's cells to nearest occupied cell.
    pub distance: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Level {
    pub offset: usize,
    pub dims: UVec3,
    pub scale: u32,
}

impl Level {
    pub fn index(&self, c: UVec3) -> usize {
        self.offset + (c.x + self.dims.x * (c.y + self.dims.y * c.z)) as usize
    }
    pub fn len(&self) -> usize {
        (self.dims.x * self.dims.y * self.dims.z) as usize
    }
    pub fn cell(&self, i: usize) -> UVec3 {
        let i = i as u32;
        UVec3::new(
            i % self.dims.x,
            i / self.dims.x % self.dims.y,
            i / (self.dims.x * self.dims.y),
        )
    }
    pub fn uniform(&self) -> [u32; 4] {
        [self.offset as u32, self.dims.x, self.dims.y, self.dims.z]
    }
}

#[derive(Clone)]
pub struct Scene {
    pub levels: [Level; 3],
    pub blocks: Vec<Block>,
    pub geometry: Arc<TreeGeometry>,
    pub occupied_sites: usize,
    pub trees: usize,
    pub instanced_surfels: u64,
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}
fn random(x: u32) -> f32 {
    (hash(x) >> 8) as f32 / 16_777_216.0
}

// Exact discrete Chebyshev distance transform (26-neighbor multi-source BFS).
// distance d means all cells with max-axis offset < d are empty.
// If the level is entirely empty, a large sentinel is safe when clipped to bounds.
fn build_distances(level: Level, blocks: &mut [Block]) {
    let mut q = VecDeque::new();
    for i in 0..level.len() {
        let b = &mut blocks[level.offset + i];
        b.distance = if b.batch == 0 { u32::MAX } else { 0 };
        if b.batch != 0 {
            q.push_back(i);
        }
    }
    if q.is_empty() {
        for b in &mut blocks[level.offset..level.offset + level.len()] {
            b.distance = level.dims.max_element() + 1;
        }
        return;
    }
    while let Some(i) = q.pop_front() {
        let p = level.cell(i).as_ivec3();
        let next = blocks[level.offset + i].distance + 1;
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let c = p + IVec3::new(dx, dy, dz);
                    if c.min_element() < 0 || !c.cmplt(level.dims.as_ivec3()).all() {
                        continue;
                    }
                    let idx = level.index(c.as_uvec3());
                    if blocks[idx].distance == u32::MAX {
                        blocks[idx].distance = next;
                        q.push_back(idx - level.offset);
                    }
                }
            }
        }
    }
}

impl Scene {
    // A regular forest with a deterministic density mask. side=1024, density=100
    // gives 1,048,576 sites, each with two shared local surfel templates.
    pub fn forest(side: u32, density: u32) -> Self {
        Self::try_forest(side, density).expect("load detailed tree assets")
    }

    pub fn try_forest(side: u32, density: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let geometry = TreeGeometry::load(Path::new(TREE_ASSETS))?;
        Ok(Self::with_geometry(side, density, Arc::new(geometry)))
    }

    pub fn try_forest_splats(side:u32,density:u32)->Result<Self,Box<dyn std::error::Error>> {
        let geometry=TreeGeometry::load_mode(Path::new(TREE_ASSETS),false)?;
        Ok(Self::with_geometry_mode(side,density,Arc::new(geometry),false))
    }
    pub fn with_geometry(side: u32, density: u32, geometry: Arc<TreeGeometry>) -> Self {Self::with_geometry_mode(side,density,geometry,true)}
    /// Create a demo-grid scene from caller-loaded models without ray acceleration.
    pub fn with_geometry_splats(side:u32,density:u32,geometry:Arc<TreeGeometry>)->Self{
        Self::with_geometry_mode(side,density,geometry,false)
    }
    fn with_geometry_mode(side:u32,density:u32,geometry:Arc<TreeGeometry>,ray_data:bool)->Self {
        assert!((1..=16383).contains(&geometry.batches.len()));
        assert!((1..=1024).contains(&side) && density <= 100);
        let fine = UVec3::new(side, 4, side);
        let mut offset = 0;
        let levels = SCALES.map(|scale| {
            let dims = (fine + UVec3::splat(scale - 1)) / UVec3::splat(scale);
            let l = Level {
                offset,
                dims,
                scale,
            };
            offset += l.len();
            l
        });
        let mut blocks = vec![Block::default(); offset];
        let mut occupied_sites = 0;
        let mut instanced_surfels = 0;
        for z in 0..side {
            for x in 0..side {
                let key = x.wrapping_add(z.wrapping_mul(65537));
                if hash(key ^ 0x13579bdf) % 100 >= density {
                    continue;
                }
                let c = UVec3::new(x, 0, z);
                let mut instance = |model_seed: u32, rotation_seed: u32| {
                    let template = hash(key ^ model_seed) % geometry.batches.len() as u32;
                    let rotation = hash(key ^ rotation_seed) & 3;
                    instanced_surfels += geometry.batches[template as usize].count as u64;
                    1 + template * 4 + rotation
                };
                let canopy = instance(0xa912abcd, 0x14b9a761);
                let understory = instance(0x29fd1c65, 0x8d64ce93);
                blocks[levels[0].index(c)].batch = canopy | (understory << 16);
                occupied_sites += 1;
                for l in &levels[1..] {
                    blocks[l.index(c / UVec3::splat(l.scale))].batch = 1;
                }
            }
        }
        if ray_data {for l in levels {build_distances(l,&mut blocks);}}
        Self {
            levels,
            blocks,
            geometry,
            occupied_sites,
            trees: occupied_sites * 2,
            instanced_surfels,
        }
    }

    pub fn extent(&self) -> Vec3 {
        self.levels[0].dims.as_vec3() * CELL
    }

    pub fn bytes(&self) -> usize {
        std::mem::size_of_val(self.blocks.as_slice())
            + std::mem::size_of_val(self.geometry.batches.as_slice())
            + std::mem::size_of_val(self.geometry.surfels.as_slice())
            + std::mem::size_of_val(self.geometry.patches.as_slice())
            + std::mem::size_of_val(self.geometry.materials.as_slice())
            + std::mem::size_of_val(self.geometry.bin_offsets.as_slice())
            + std::mem::size_of_val(self.geometry.bin_indices.as_slice())
            + std::mem::size_of_val(self.geometry.acceleration.as_slice())
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Stats {
    pub blocks: u64,
    pub jumps: u64,
    pub surfels: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub color: Vec3,
    pub normal: Vec3,
}

pub fn slab(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<(f32, f32)> {
    let mut a = f32::NEG_INFINITY;
    let mut b = f32::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-8 {
            if o[i] < lo[i] || o[i] >= hi[i] {
                return None;
            }
        } else {
            let p = (lo[i] - o[i]) / d[i];
            let q = (hi[i] - o[i]) / d[i];
            a = a.max(p.min(q));
            b = b.min(p.max(q));
        }
    }
    (a <= b).then_some((a, b))
}

fn cell_at(p: Vec3, d: Vec3, dims: UVec3, width: f32) -> IVec3 {
    let g = p / width;
    let mut c = g.floor().as_ivec3();
    // On exact boundaries negative rays enter the previous cell.
    for i in 0..3 {
        if d[i] < 0.0 && g[i] == g[i].floor() {
            c[i] -= 1;
        }
    }
    c.clamp(IVec3::ZERO, dims.as_ivec3() - IVec3::ONE)
}

// Exit a known region in fine-cell coordinates; use explicit crossed-face
// indices, not a large epsilon that could jump over a thin intersection.
fn advance(
    o: Vec3,
    d: Vec3,
    t: f32,
    lo: IVec3,
    hi: IVec3,
    dims: UVec3,
    width: f32,
) -> (f32, IVec3) {
    let mut ts = Vec3::splat(f32::INFINITY);
    for i in 0..3 {
        if d[i].abs() >= 1e-8 {
            let edge = if d[i] > 0.0 { hi[i] } else { lo[i] };
            ts[i] = (edge as f32 * width - o[i]) / d[i];
        }
    }
    let exit = ts.min_element();
    let mut next = cell_at(o + d * exit, d, dims, width).clamp(lo, hi - IVec3::ONE);
    for i in 0..3 {
        if ts[i] == exit {
            next[i] = if d[i] > 0.0 { hi[i] } else { lo[i] - 1 };
        }
    }
    (exit.max(t), next)
}

fn empty_region(c: IVec3, l: Level, distance: u32, fine: UVec3) -> (IVec3, IVec3) {
    let s = l.scale as i32;
    let g = c / s;
    let r = distance.saturating_sub(1) as i32;
    let lo = ((g - IVec3::splat(r)) * s).max(IVec3::ZERO);
    let hi = ((g + IVec3::splat(r + 1)) * s).min(fine.as_ivec3());
    (lo, hi)
}

pub fn trace(
    scene: &Scene,
    o: Vec3,
    d: Vec3,
    _pixel_scale: f32,
    skip: bool,
    stats: &mut Stats,
) -> Option<Hit> {
    let dims = scene.levels[0].dims;
    let extent = scene.extent();
    let ground_t = if d.y < -1e-8 {
        -o.y / d.y
    } else {
        f32::INFINITY
    };
    let mut ground = None;
    if ground_t.is_finite() && ground_t >= 0.0 {
        let p = o + d * ground_t;
        if p.x >= 0.0 && p.z >= 0.0 && p.x < extent.x && p.z < extent.z {
            ground = Some(Hit {
                t: ground_t,
                color: Vec3::new(0.16, 0.24, 0.09),
                normal: Vec3::Y,
            });
        }
    }
    let Some((enter, leave)) = slab(o, d, Vec3::ZERO, extent) else {
        return ground;
    };
    let mut t = enter.max(0.0);
    let end = leave.min(ground.map_or(f32::INFINITY, |h| h.t));
    if t >= end {
        return ground;
    }
    let mut c = cell_at(o + d * t, d, dims, CELL);
    for _ in 0..(dims.x + dims.y + dims.z + 8) {
        if c.min_element() < 0 || !c.cmplt(dims.as_ivec3()).all() || t >= end {
            break;
        }
        let fine = scene.blocks[scene.levels[0].index(c.as_uvec3())];
        stats.blocks += 1;
        // Dense forest: do not read two coarse cells on every occupied site.
        let mut region = None;
        if skip && fine.batch == 0 {
            for l in scene.levels[1..].iter().rev() {
                let b = scene.blocks[l.index(c.as_uvec3() / UVec3::splat(l.scale))];
                stats.blocks += 1;
                if b.batch == 0 {
                    region = Some(empty_region(c, *l, b.distance, dims));
                    break;
                }
            }
            if region.is_none() {
                region = Some(empty_region(c, scene.levels[0], fine.distance, dims));
            }
        }
        let (lo, hi) = region.unwrap_or((c, c + IVec3::ONE));
        let (next_t, next) = advance(o, d, t, lo, hi, dims, CELL);
        if region.is_some() && (hi - lo).max_element() > 1 {
            stats.jumps += 1;
        }
        if fine.batch != 0 {
            let local_o = (o - c.as_vec3() * CELL) + d * t;
            let limit = next_t.min(end) - t;
            let canopy = trace_tree(
                &scene.geometry,
                (fine.batch & 65535) - 1,
                c,
                local_o,
                d,
                limit,
                false,
                skip,
                stats,
            );
            let understory = trace_tree(
                &scene.geometry,
                (fine.batch >> 16) - 1,
                c,
                local_o,
                d,
                canopy.map_or(limit, |hit| hit.t),
                true,
                skip,
                stats,
            );
            if let Some(mut hit) = understory.or(canopy) {
                hit.t += t;
                return Some(hit);
            }
        }
        c = next;
        t = next_t;
    }
    ground
}

fn rotate(p: Vec3, turn: u32) -> Vec3 {
    match turn {
        1 => Vec3::new(p.z, p.y, -p.x),
        2 => Vec3::new(-p.x, p.y, -p.z),
        3 => Vec3::new(-p.z, p.y, p.x),
        _ => p,
    }
}

fn trace_tree(
    geometry: &TreeGeometry,
    instance: u32,
    world_cell: IVec3,
    origin: Vec3,
    direction: Vec3,
    limit: f32,
    understory: bool,
    skip: bool,
    stats: &mut Stats,
) -> Option<Hit> {
    let tree = (instance / 4) as usize;
    let turn = instance & 3;
    let batch = &geometry.batches[tree];
    let lo = Vec3::from(batch.lo);
    let hi = Vec3::from(batch.hi);
    let key = world_cell.x as u32 + world_cell.z as u32 * 65537;
    let center = Vec3::new(CELL * 0.5, 0.0, CELL * 0.5);
    let (o, scale) = if understory {
        let axis = |seed| {
            let j = random(key ^ seed);
            if j < 0.5 {
                4.0 + j * 2.0
            } else {
                11.0 + (j - 0.5) * 2.0
            }
        };
        let anchor = Vec3::new(axis(0x715ed0b1), 0.0, axis(0x48f52175));
        let scale = 0.34 + 0.12 * random(key ^ 0x57d05d9b);
        (rotate((origin - anchor) / scale, turn) + center, scale)
    } else {
        let jitter = |lo: f32, hi: f32, seed| {
            let left = lo.min(2.3);
            let right = (CELL - hi).min(2.3);
            random(key ^ seed) * (left + right) - left
        };
        let shift = Vec3::new(
            jitter(lo.x, hi.x, 0x57a318d1),
            0.0,
            jitter(lo.z, hi.z, 0x6a814b21),
        );
        (rotate(origin - center, turn) + center - shift, 1.0)
    };
    let d = rotate(direction, turn);
    let mut hit = trace_template(geometry, tree, o, d, limit / scale, skip, stats)?;
    hit.t *= scale;
    hit.normal = rotate(hit.normal, (4 - turn) & 3);
    Some(hit)
}

fn trace_template(geometry: &TreeGeometry, tree: usize, o: Vec3, d: Vec3, limit: f32, skip: bool, stats: &mut Stats) -> Option<Hit> {
    let batch = &geometry.batches[tree];
    let (enter, leave) = slab(o, d, Vec3::from(batch.lo), Vec3::from(batch.hi))?;
    let mut t = enter.max(0.0);
    let end = leave.min(limit);
    if t >= end {
        return None;
    }

    let dims = UVec3::splat(BIN_SIDE);
    let mut c = cell_at(o + d * t, d, dims, BIN_WIDTH);
    for _ in 0..BIN_SIDE * 3 {
        if c.min_element() < 0 || c.max_element() >= BIN_SIDE as i32 || t >= end {
            break;
        }
        let local_bin = bin_index(c.as_uvec3());
        let bin = tree * BINS_PER_TREE + local_bin;
        let start = geometry.bin_offsets[bin] as usize;
        let finish = geometry.bin_offsets[bin + 1] as usize;
        if skip && start == finish {
            let r = geometry.acceleration[tree * ACCEL_STRIDE + local_bin].saturating_sub(1) as i32;
            let lo = (c - IVec3::splat(r)).max(IVec3::ZERO);
            let hi = (c + IVec3::splat(r+1)).min(dims.as_ivec3());
            let (next_t, next) = advance(o, d, t, lo, hi, dims, BIN_WIDTH);
            if r>0 {stats.jumps+=1;}
            c=next; t=next_t; continue;
        }
        let (next_t, next) = advance(o, d, t, c, c + IVec3::ONE, dims, BIN_WIDTH);
        let mut best = next_t.min(end);
        let mut intersection = None;
        let mut last_patch = u32::MAX;
        for &index in &geometry.bin_indices[start..finish] {
            stats.surfels += 1;
            let s = &geometry.surfels[index as usize];
            if s.patch == last_patch {
                continue;
            }
            let (p, radius) = s.center_radius();
            let delta = p - o;
            let along = delta.dot(d);
            if along + radius < t
                || along - radius >= best
                || (delta - d * along).length_squared() > radius * radius
            {
                continue;
            }
            let patch = &geometry.patches[s.patch as usize];
            let n = Vec3::from_slice(&patch.plane);
            let denom = d.dot(n);
            if denom.abs() < 1e-7 {
                continue;
            }
            let u = (patch.plane[3] - o.dot(n)) / denom;
            if u < t || u >= best {
                continue;
            }
            let pos = o + d * u;
            if (pos - p).length_squared() > radius * radius {
                continue;
            }
            last_patch = s.patch;
            let b = patch.coordinates(pos);
            if b.min_element() < -0.00005
                || b.element_sum() > 1.00005
                || !geometry.alpha_visible(patch, b)
            {
                continue;
            }
            best = u;
            intersection = Some((patch, b, denom > 0.0));
        }
        if let Some((patch, b, backface)) = intersection {
            let (color, mut normal) = geometry.surface(patch, b);
            if backface {
                normal = -normal;
            }
            return Some(Hit {
                t: best,
                color,
                normal,
            });
        }
        c = next;
        t = next_t;
    }
    None
}

// Bake six local orthographic depth/color/normal maps using the ORIGINAL
// alpha-tested surfel geometry. This is an optional, approximate GPU far LOD,
// not part of the exact CPU reference. It does not store or render cube faces.
pub(crate) fn bake_tree_proxies(geometry: &mut TreeGeometry) {
    for tree in 0..geometry.batches.len() {
        for face in 0..6 {
            let axis = face / 2;
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            let positive = face % 2 == 0;
            let mut d = Vec3::ZERO;
            d[axis] = if positive {1.0} else {-1.0};
            for y in 0..PROXY_SIDE { for x in 0..PROXY_SIDE {
                let mut o = Vec3::ZERO;
                o[axis] = if positive {0.0} else {CELL};
                o[a] = (x as f32 + 0.5)*CELL/PROXY_SIDE as f32;
                o[b] = (y as f32 + 0.5)*CELL/PROXY_SIDE as f32;
                if let Some(hit) = trace_template(geometry, tree, o, d, CELL, true, &mut Stats::default()) {
                    let i = tree * ACCEL_STRIDE + BIN_COUNT + (face * PROXY_SIDE * PROXY_SIDE + y * PROXY_SIDE + x) * 3;
                    let rgb = hit.color.clamp(Vec3::ZERO,Vec3::ONE).to_array().map(|v| (v*255.0+0.5) as u32);
                    geometry.acceleration[i] = (o[axis]+d[axis]*hit.t).to_bits();
                    geometry.acceleration[i+1] = 0xff000000 | rgb[0] | (rgb[1]<<8) | (rgb[2]<<16);
                    geometry.acceleration[i+2] = trees::pack_normal(hit.normal);
                }
            }}
        }
    }
}

pub fn shade(hit: Option<Hit>, d: Vec3) -> Vec3 {
    match hit {
        Some(h) => {
            let sun = Vec3::new(-0.5, 0.8, -0.4).normalize();
            let light = 0.36 + 0.64 * h.normal.dot(sun).max(0.0);
            let fog = 1.0 - (-h.t * 0.00045).exp();
            (h.color * light).lerp(Vec3::new(0.51, 0.66, 0.78), fog)
        }
        None => Vec3::new(0.65, 0.76, 0.87).lerp(Vec3::new(0.23, 0.47, 0.75), d.y.max(0.0)),
    }
}

// Physical slots for a future camera-relative ring cache. Coordinates must
// first be checked against the active logical window and a generation tag.
pub fn ring_slot(world: IVec3, dims: IVec3) -> usize {
    let x = world.x.rem_euclid(dims.x);
    let y = world.y.rem_euclid(dims.y);
    let z = world.z.rem_euclid(dims.z);
    (x + dims.x * (y + dims.y * z)) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    fn forest(side: u32, density: u32) -> Scene {
        static GEOMETRY: OnceLock<Arc<TreeGeometry>> = OnceLock::new();
        let geometry =
            GEOMETRY.get_or_init(|| Arc::new(TreeGeometry::load(Path::new(TREE_ASSETS)).unwrap()));
        Scene::with_geometry(side, density, Arc::clone(geometry))
    }

    #[test]
    fn direct_address_and_distance() {
        let s = forest(9, 100);
        let l = s.levels[0];
        for i in 0..l.len() {
            let c = l.cell(i);
            assert_eq!(l.index(c), i);
            assert_eq!(s.blocks[i].distance, c.y);
        }
    }
    #[test]
    fn sparse_distance_matches_brute_force() {
        let s = forest(11, 19);
        for l in s.levels {
            let occupied: Vec<_> = (0..l.len())
                .filter(|&i| s.blocks[l.offset + i].batch != 0)
                .map(|i| l.cell(i).as_ivec3())
                .collect();
            if occupied.is_empty() {
                continue;
            }
            for i in 0..l.len() {
                let p = l.cell(i).as_ivec3();
                let expected = occupied
                    .iter()
                    .map(|q| (*q - p).abs().max_element() as u32)
                    .min()
                    .unwrap();
                assert_eq!(s.blocks[l.offset + i].distance, expected);
            }
        }
    }
    #[test]
    fn skip_matches_fine_dda() {
        let s = forest(24, 43);
        for i in 0..4000u32 {
            let o = Vec3::new(
                random(i * 7) * 500.0 - 60.0,
                1.0 + random(i * 7 + 1) * 95.0,
                random(i * 7 + 2) * 500.0 - 60.0,
            );
            let d = Vec3::new(
                random(i * 7 + 3) * 2.0 - 1.0,
                random(i * 7 + 4) * 2.0 - 1.0,
                random(i * 7 + 5) * 2.0 - 1.0,
            )
            .normalize();
            let a = trace(&s, o, d, 0.002, false, &mut Stats::default());
            let b = trace(&s, o, d, 0.002, true, &mut Stats::default());
            match (a, b) {
                (Some(a), Some(b)) => assert!((a.t - b.t).abs() < 0.002, "ray {i}: {a:?} {b:?}"),
                (None, None) => {}
                _ => panic!("different visibility at ray {i}: {a:?} {b:?}"),
            }
        }
    }
    #[test]
    fn empty_forest_and_axis_rays() {
        let s = forest(17, 0);
        let o = Vec3::new(20.0, 25.0, 20.0);
        for d in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z, Vec3::Y] {
            assert!(trace(&s, o, d, 0.001, true, &mut Stats::default()).is_none());
        }
        assert_eq!(
            trace(&s, o, Vec3::NEG_Y, 0.001, true, &mut Stats::default())
                .unwrap()
                .t,
            25.0
        );
    }
    #[test]
    fn ring_wraps_negative_coordinates() {
        let dims = IVec3::new(8, 4, 8);
        assert_eq!(
            ring_slot(IVec3::new(-1, 0, -1), dims),
            ring_slot(IVec3::new(7, 0, 7), dims)
        );
    }
}
