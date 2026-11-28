//! Multi-species adaptation of canopy_forest: sampled surface nets and
//! finite-time progressive coverage. No point primitives are drawn.
use super::{Bounds, Instance, Placed, View};
use crate::trees::{MeshModel, TreeGeometry};
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3, Vec4};
use std::{collections::BTreeMap, error::Error};
const TEMPLATE_POINTS: usize = 32768;
fn setting(name: &str, default: f32, min: f32, max: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
        .clamp(min, max)
}
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b);
    x ^ (x >> 16)
}
fn random(x: u32) -> f32 {
    (hash(x) >> 8) as f32 / 16777216.0
}
#[derive(Clone, Copy)]
struct Sample {
    p: Vec3,
    color: Vec3,
    area: f32,
}

fn template(geo: &TreeGeometry, mesh: &MeshModel, foliage: bool) -> Vec<Sample> {
    let mut faces = Vec::new();
    let mut total = 0.0f64;
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = [
            mesh.vertices[t[0] as usize],
            mesh.vertices[t[1] as usize],
            mesh.vertices[t[2] as usize],
        ];
        let (_, leaf, _) = geo.sample_mesh_material(a.material, Vec2::from_array(a.uv));
        if leaf != foliage {
            continue;
        }
        let p = Vec3::from_array(a.position);
        let q = Vec3::from_array(b.position);
        let r = Vec3::from_array(c.position);
        let cross = (q - p).cross(r - p);
        let area = cross.length() * 0.5;
        if !area.is_finite() || area <= 1e-12 {
            continue;
        }
        total += f64::from(area);
        faces.push((total, [a, b, c]));
    }
    if faces.is_empty() {
        return Vec::new();
    }
    let count = if foliage { TEMPLATE_POINTS } else { 4096 };
    let mut result = Vec::with_capacity(count);
    let mut trials = 0u32;
    for j in 0..(count as u32 * 128) {
        trials = j + 1;
        let seed = j
            .wrapping_mul(4)
            .wrapping_add(if foliage { 0x9e3779b9 } else { 0 });
        let target = f64::from(random(seed)) * total;
        let i = faces
            .partition_point(|x| x.0 <= target)
            .min(faces.len() - 1);
        let (_, [a, b, c]) = faces[i];
        let u = random(seed.wrapping_add(1)).sqrt();
        let v = random(seed.wrapping_add(2));
        let (wa, wb, wc) = (1.0 - u, u * (1.0 - v), u * v);
        let uv =
            Vec2::from_array(a.uv) * wa + Vec2::from_array(b.uv) * wb + Vec2::from_array(c.uv) * wc;
        let (rgba, _, accepted) = geo.sample_mesh_material(a.material, uv);
        if !accepted {
            continue;
        }
        let p = Vec3::from_array(a.position) * wa
            + Vec3::from_array(b.position) * wb
            + Vec3::from_array(c.position) * wc;
        result.push(Sample {
            p,
            color: rgba.truncate(),
            area: 0.0,
        });
        if result.len() == count {
            break;
        }
    }
    let area = (total / f64::from(trials.max(1))) as f32;
    for s in &mut result {
        s.area = area;
    }
    result
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Vertex {
    position: [f32; 3],
    rgba: [u16; 4],
    grain: f32,
    normal: [i8; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
pub(super) struct Draw {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}
#[derive(Default)]
pub(super) struct Geometry {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}
impl Geometry {
    pub(super) fn bytes(&self) -> u64 {
        (self.vertices.len() * size_of::<Vertex>() + self.indices.len() * size_of::<u32>()) as u64
    }
}
const MAX_BYTES: u64 = 384 * 1024 * 1024;
const CORNERS: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [0, 1, 1],
    [1, 1, 1],
];
const EDGES: [[usize; 2]; 12] = [
    [0, 1],
    [2, 3],
    [4, 5],
    [6, 7],
    [0, 2],
    [1, 3],
    [4, 6],
    [5, 7],
    [0, 4],
    [1, 5],
    [2, 6],
    [3, 7],
];
struct Field {
    origin: Vec3,
    step: f32,
    n: [usize; 3],
    nodes: Vec<Vec4>,
    area: Vec<Vec2>,
}
impl Field {
    fn new(b: Bounds, step: f32) -> Result<Self, Box<dyn Error>> {
        let origin = b.lo - Vec3::splat(2.0 * step);
        let e = (b.hi - b.lo) / step;
        let n = [
            e.x.ceil() as usize + 5,
            e.y.ceil() as usize + 5,
            e.z.ceil() as usize + 5,
        ];
        let count = n[0]
            .checked_mul(n[1])
            .and_then(|v| v.checked_mul(n[2]))
            .ok_or("canopy field overflow")?;
        if count > 4_000_000 {
            return Err("canopy field too large; increase step or reduce cell size".into());
        }
        Ok(Self {
            origin,
            step,
            n,
            nodes: vec![Vec4::ZERO; count],
            area: vec![Vec2::ZERO; count],
        })
    }
    fn id(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.n[1] + y) * self.n[0] + x
    }
    fn add(&mut self, p: Vec3, c: Vec3, area: f32, leaf: bool) {
        let q = (p - self.origin) / self.step;
        let base = q.floor();
        let t = q - base;
        if base.min_element() < 0.0 {
            return;
        }
        let (x, y, z) = (base.x as usize, base.y as usize, base.z as usize);
        if x + 1 >= self.n[0] || y + 1 >= self.n[1] || z + 1 >= self.n[2] {
            return;
        }
        let weights = [Vec3::ONE - t, t];
        for [a, b, d] in CORNERS {
            let w = weights[a].x * weights[b].y * weights[d].z;
            let id = self.id(x + a, y + b, z + d);
            self.nodes[id] += c.extend(1.0) * w;
            self.area[id] += if leaf {
                Vec2::new(area * w, 0.0)
            } else {
                Vec2::new(0.0, area * w)
            };
        }
    }
    // Estimate local projected opacity from source surface area rather than
    // sample count. The 8 trilinear node supports overlap; the normalization
    // and opacity floor are deliberately heuristic, to be calibrated by A/B.
    fn opacity(&self, x: usize, y: usize, z: usize) -> f32 {
        let a = CORNERS
            .iter()
            .map(|&[i, j, k]| self.area[self.id(x + i, y + j, z + k)])
            .sum::<Vec2>();
        coverage(a, self.step)
    }
    // Surface nets: one shared vertex per sign-changing lattice cube,
    // then one quad for each sign-changing lattice edge. Empty spaces
    // remain empty; no canopy-wide plane, no billboards.
    fn emit(&self, threshold: f32, g: &mut Geometry) -> Result<Draw, Box<dyn Error>> {
        let first = u32::try_from(g.indices.len())?;
        let [nx, ny, nz] = self.n;
        let mut vertex_ids = vec![u32::MAX; self.nodes.len()];
        for z in 0..nz - 1 {
            for y in 0..ny - 1 {
                for x in 0..nx - 1 {
                    let values = CORNERS.map(|[a, b, c]| self.nodes[self.id(x + a, y + b, z + c)]);
                    let inside = values.map(|v| v.w >= threshold);
                    if inside.iter().all(|&v| v) || inside.iter().all(|&v| !v) {
                        continue;
                    }
                    let mut position = Vec3::ZERO;
                    let mut color = Vec3::ZERO;
                    let mut gradient = Vec3::ZERO;
                    let mut samples = 0.0;
                    for [a, b] in EDGES {
                        if inside[a] == inside[b] {
                            continue;
                        }
                        let va = values[a];
                        let vb = values[b];
                        let t = ((threshold - va.w) / (vb.w - va.w)).clamp(0.0, 1.0);
                        let corner = |i: usize| {
                            Vec3::new(
                                CORNERS[i][0] as f32,
                                CORNERS[i][1] as f32,
                                CORNERS[i][2] as f32,
                            )
                        };
                        position += corner(a).lerp(corner(b), t);
                        // Interpolate the accumulated albedo and weight together.
                        let q = va.lerp(vb, t);
                        color += q.truncate() / q.w.max(1e-6);
                        samples += 1.0;
                        gradient += (corner(b) - corner(a)) * (vb.w - va.w);
                    }
                    if samples == 0.0 {
                        continue;
                    }
                    let p = self.origin
                        + (Vec3::new(x as f32, y as f32, z as f32) + position / samples)
                            * self.step;
                    vertex_ids[self.id(x, y, z)] = u32::try_from(g.vertices.len())?;
                    let opacity = self.opacity(x, y, z);
                    let rgba = pack_rgba((color / samples).clamp(Vec3::ZERO, Vec3::ONE), opacity);
                    g.vertices.push(Vertex {
                        position: p.to_array(),
                        rgba,
                        grain: (self.step * 0.40).clamp(0.20, 1.4),
                        normal: (-gradient)
                            .normalize_or(Vec3::Y)
                            .extend(0.0)
                            .to_array()
                            .map(|axis| (axis * 127.0).round() as i8),
                    });
                }
            }
        }
        for z in 1..nz - 1 {
            for y in 1..ny - 1 {
                for x in 1..nx - 1 {
                    let q = [x, y, z];
                    let start = self.nodes[self.id(x, y, z)].w >= threshold;
                    for axis in 0..3 {
                        let mut end = q;
                        end[axis] += 1;
                        if start == (self.nodes[self.id(end[0], end[1], end[2])].w >= threshold) {
                            continue;
                        }
                        let a = (axis + 1) % 3;
                        let b = (axis + 2) % 3;
                        let mut p1 = q;
                        p1[a] -= 1;
                        let mut p2 = p1;
                        p2[b] -= 1;
                        let mut p3 = q;
                        p3[b] -= 1;
                        let ids = [q, p1, p2, p3].map(|p| vertex_ids[self.id(p[0], p[1], p[2])]);
                        if ids.contains(&u32::MAX) {
                            continue;
                        }
                        let [i, j, k, l] = ids;
                        if start {
                            g.indices.extend_from_slice(&[i, j, k, i, k, l]);
                        } else {
                            g.indices.extend_from_slice(&[i, k, j, i, l, k]);
                        }
                    }
                }
            }
        }
        if g.bytes() > MAX_BYTES {
            return Err("canopy geometry exceeds 384 MiB; increase IGOR_CANOPY_FAR_STEP".into());
        }
        Ok(Draw {
            index_count: u32::try_from(g.indices.len())? - first,
            instance_count: 1,
            first_index: first,
            base_vertex: 0,
            first_instance: 0,
        })
    }
}
fn coverage(area: Vec2, step: f32) -> f32 {
    let total = area.x + area.y;
    if total <= 1e-8 {
        return 1.0;
    }
    // Approximate random leaf projection and a shell's local path length.
    let tau = area.x / (8.0 * step * step);
    let foliage = 0.48 + 0.50 * (1.0 - (-tau).exp());
    // Wood-dominated surface must stay solid; mixed cells gradually retain it.
    let wood = (2.0 * area.y / total).clamp(0.0, 1.0);
    foliage + (1.0 - foliage) * wood
}
fn pack_rgba(color: Vec3, alpha: f32) -> [u16; 4] {
    color
        .extend(alpha)
        .clamp(Vec4::ZERO, Vec4::ONE)
        .to_array()
        .map(|v| (v * 65535.0).round() as u16)
}
fn identity() -> Instance {
    Instance {
        translation: [0.0, 0.0, 0.0, 1.0],
        rotation: [1.0, 0.0, 0.0, 0.0],
    }
}
fn smooth(p: f32, low: f32, high: f32) -> f32 {
    let t = ((p - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
// Select a stable discrete target with hysteresis, then converge to it
// in finite time. Never stop at an intermediate screen-space fraction.
#[derive(Clone, Copy, Debug)]
struct Blend {
    weight: f32,
    target: bool,
}
fn advance_rank(
    state: &mut Option<Blend>,
    fraction: f32,
    rank: f32,
    width: f32,
    dt: f32,
    seconds: f32,
) -> f32 {
    let initial = fraction >= 1.0 || (fraction > 0.0 && fraction > rank);
    let previous = state.unwrap_or(Blend {
        weight: if initial { 1.0 } else { 0.0 },
        target: initial,
    });
    let target = if fraction <= 0.0 {
        false
    } else if fraction >= 1.0 {
        true
    } else if previous.target {
        fraction >= (rank - width * 0.5).max(0.0)
    } else {
        fraction > (rank + width * 0.5).min(1.0)
    };
    let target = if state.is_none() || width <= 0.0 {
        initial
    } else {
        target
    };
    let goal = if target { 1.0 } else { 0.0 };
    let delta = if seconds > 0.0 { dt / seconds } else { 1.0 };
    let next = previous.weight + (goal - previous.weight).clamp(-delta, delta);
    let weight = if (next - goal).abs() < 1e-5 {
        goal
    } else {
        next
    };
    *state = Some(Blend { weight, target });
    weight
}
// Far fills throughout the fade; fine geometry thins with delayed overlap.
// Full-to-mid retains the two-phase overlap.
fn lod_ranges(coarse: f32, full: f32) -> [(f32, f32); 3] {
    let far_hi = coarse * coarse * (3.0 - 2.0 * coarse);
    let fine_lo = (far_hi - 0.24 * 4.0 * far_hi * (1.0 - far_hi)).max(0.0);
    let mid_hi = fine_lo + (1.0 - fine_lo) * (2.0 * (1.0 - full)).min(1.0);
    let full_lo = fine_lo + (1.0 - fine_lo) * (1.0 - 2.0 * full).max(0.0);
    [(0.0, far_hi), (fine_lo, mid_hi), (full_lo, 1.0)]
}
fn ranged(mut instance: Instance, lo: f32, hi: f32) -> Instance {
    instance.rotation[2] = lo;
    instance.rotation[3] = hi;
    instance
}

struct Cell {
    bounds: Bounds,
    trees: Vec<usize>,
    draw: Draw,
}

pub(super) struct Canopy {
    cells: Vec<Cell>,
    pub geometry: Geometry,
    pub padding: f32,
}

pub(super) struct Template {
    wood: Vec<Sample>,
    leaf: Vec<Sample>,
}

pub(super) struct Selector {
    cell_blends: Vec<Option<Blend>>,
    tree_blends: Vec<Option<Blend>>,
    mid: Vec<Draw>,
    mid_low: f32,
    mid_high: f32,
    far_low: f32,
    far_high: f32,
    fade_width: f32,
    fade_seconds: f32,
    far_seconds: f32,
}

pub(super) struct Selected {
    pub full: Vec<Vec<Instance>>,
    pub mid: Vec<Vec<Instance>>,
    pub far: Vec<(Draw, Instance)>,
    pub full_count: usize,
    pub mid_count: usize,
    pub far_count: usize,
    pub fades: [usize; 2],
}
impl Selected {
    fn new(n: usize) -> Self {
        Self {
            full: vec![Vec::new(); n],
            mid: vec![Vec::new(); n],
            far: Vec::new(),
            full_count: 0,
            mid_count: 0,
            far_count: 0,
            fades: [0; 2],
        }
    }
}
fn append(dst: &mut Geometry, local: Geometry, mut draw: Draw) -> Result<Draw, Box<dyn Error>> {
    let v = u32::try_from(dst.vertices.len())?;
    draw.first_index = u32::try_from(dst.indices.len())?;
    dst.vertices.extend(local.vertices);
    dst.indices.extend(local.indices.into_iter().map(|i| i + v));
    Ok(draw)
}

fn geometry_budget() -> u64 {
    (setting("IGOR_CANOPY_BUDGET_MIB", 128.0, 16.0, 384.0) * 1048576.0) as u64
}

pub(super) fn build_template(
    geo: &TreeGeometry,
    m: &MeshModel,
    models: usize,
    geometry: &mut Geometry,
) -> Result<(Draw, Template), Box<dyn Error>> {
    let resolution = setting("IGOR_CANOPY_MID_RES", 32.0, 12.0, 64.0);
    let mid_threshold = setting("IGOR_CANOPY_MID_FIELD", 0.65, 0.05, 16.0);
    let model_budget = (geometry_budget() / 3 / models.max(1) as u64).max(256);
    let b = Bounds { lo: m.lo, hi: m.hi };
    let wood = template(geo, m, false);
    let leaf = template(geo, m, true);
    let mut draw = Draw::default();
    let mut step = ((m.hi - m.lo).max_element() / resolution).max(0.01);

    for _ in 0..6 {
        let mut field = Field::new(b, step)?;
        for (samples, is_leaf) in [(&wood, false), (&leaf, true)] {
            for &s in samples {
                field.add(s.p, s.color, s.area, is_leaf);
            }
        }
        let mut local = Geometry::default();
        if let Ok(d) = field.emit(mid_threshold, &mut local) {
            if d.index_count > 0
                && local.bytes() <= model_budget
                && u64::from(d.index_count) < m.indices.len() as u64
            {
                draw = append(geometry, local, d)?;
                break;
            }
        }
        step *= 1.35;
    }

    Ok((draw, Template { wood, leaf }))
}

impl Canopy {
    pub(super) fn build(
        placed: &[Placed],
        templates: &[Template],
        mid_bytes: u64,
    ) -> Result<Self, Box<dyn Error>> {
        let cell_size = setting("IGOR_HYBRID_CELL_METERS", 48.0, 8.0, 128.0);
        let far_step = setting("IGOR_CANOPY_FAR_STEP", 3.0, 0.5, 16.0);
        let far_threshold = setting("IGOR_CANOPY_FAR_FIELD", 1.5, 0.05, 32.0);
        let budget = geometry_budget();
        let mut geometry = Geometry::default();
        let mut grid = BTreeMap::<(i32, i32), Vec<usize>>::new();
        for (i, p) in placed.iter().enumerate() {
            let center = (p.bounds.lo + p.bounds.hi) * 0.5;
            grid.entry((
                (center.x / cell_size).floor() as i32,
                (center.z / cell_size).floor() as i32,
            ))
            .or_default()
            .push(i);
        }
        let mut cells = Vec::with_capacity(grid.len());
        let mut padding: f32 = 0.0;
        let per_tree_budget =
            budget.saturating_sub(mid_bytes + geometry.bytes()) / placed.len().max(1) as u64;
        for trees in grid.into_values() {
            let mut b = Bounds::empty();
            for &i in &trees {
                b.merge(placed[i].bounds);
            }
            let cell_budget = (per_tree_budget * trees.len() as u64)
                .min(budget.saturating_sub(mid_bytes + geometry.bytes()));
            let mut draw = Draw::default();
            let mut step = far_step;
            for _ in 0..7 {
                if cell_budget < 256 {
                    break;
                }
                let mut field = match Field::new(b, step) {
                    Ok(f) => f,
                    Err(_) => {
                        step *= 1.4;
                        continue;
                    }
                };
                for &i in &trees {
                    let p = &placed[i];
                    let t = p.instance;
                    let Template { wood, leaf } = &templates[p.model];
                    for (all, is_leaf, limit) in [(wood, false, 512usize), (leaf, true, 4096usize)]
                    {
                        let samples = &all[..all.len().min(limit)];
                        if samples.is_empty() {
                            continue;
                        }
                        let weight = all.len() as f32 / samples.len() as f32
                            * t.translation[3]
                            * t.translation[3];
                        for &sample in samples {
                            field.add(
                                t.world(sample.p),
                                sample.color,
                                sample.area * weight,
                                is_leaf,
                            );
                        }
                    }
                }
                let mut local = Geometry::default();
                if let Ok(d) = field.emit(far_threshold * (step / 1.5).max(1.0), &mut local) {
                    if d.index_count > 0 && local.bytes() <= cell_budget {
                        draw = append(&mut geometry, local, d)?;
                        break;
                    }
                }
                step *= 1.4;
            }
            // Keep the cell bounds conservative for the generated shell.
            let bounds = if draw.index_count > 0 {
                Bounds {
                    lo: b.lo - Vec3::splat(step),
                    hi: b.hi + Vec3::splat(step),
                }
            } else {
                b
            };
            cells.push(Cell {
                bounds,
                trees,
                draw,
            });
            if draw.index_count > 0 {
                padding = padding.max(step);
            }
        }

        Ok(Self {
            cells,
            geometry,
            padding,
        })
    }
}

impl Selector {
    pub(super) fn new(canopy: &Canopy, trees: usize, mid: Vec<Draw>) -> Self {
        let mid_high = setting("IGOR_CANOPY_MID_HIGH", 130.0, 12.0, 2048.0);
        let far_high = setting("IGOR_CANOPY_FAR_HIGH", 160.0, 12.0, 512.0);
        Self {
            cell_blends: vec![None; canopy.cells.len()],
            tree_blends: vec![None; trees],
            mid,
            mid_high,
            far_high,
            mid_low: setting("IGOR_CANOPY_MID_LOW", 70.0, 4.0, mid_high * 0.99),
            far_low: setting("IGOR_CANOPY_FAR_LOW", 80.0, 4.0, far_high * 0.99),
            fade_width: setting("IGOR_CANOPY_FADE_WIDTH", 0.15, 0.0, 1.0),
            fade_seconds: setting("IGOR_CANOPY_FADE_SECONDS", 0.35, 0.0, 1.0),
            far_seconds: setting("IGOR_CANOPY_FAR_FADE_SECONDS", 0.9, 0.0, 4.0),
        }
    }

    pub(super) fn reset(&mut self) {
        self.cell_blends.fill(None);
        self.tree_blends.fill(None);
    }

    pub(super) fn select(
        &mut self,
        canopy: &Canopy,
        view: View,
        placed: &[Placed],
        enabled: bool,
        soft: bool,
        dt: f32,
    ) -> Selected {
        let mut out = Selected::new(self.mid.len());
        if !enabled {
            for p in placed {
                if view.visible(p.bounds) {
                    out.full[p.model].push(ranged(p.instance, 0.0, 1.0));
                    out.full_count += 1;
                }
            }
            return out;
        }
        let width = if soft { self.fade_width } else { 0.0 };
        let seconds = if soft { self.fade_seconds } else { 0.0 };
        for (id, cell) in canopy.cells.iter().enumerate() {
            let blend = &mut self.cell_blends[id];
            if !view.visible(cell.bounds) {
                *blend = None;
                for &i in &cell.trees {
                    self.tree_blends[i] = None;
                }
                continue;
            }
            let px = view.span(cell.bounds);
            let coarse = if cell.draw.index_count == 0 || !px.is_finite() {
                *blend = None;
                0.0
            } else {
                advance_rank(
                    blend,
                    1.0 - smooth(px, self.far_low, self.far_high),
                    1.0 - random((id as u32) ^ 0xe9c47031),
                    width,
                    dt,
                    if soft { self.far_seconds } else { 0.0 },
                )
            };
            let mut visible = 0;
            for &i in &cell.trees {
                let p = &placed[i];
                if !view.visible(p.bounds) {
                    self.tree_blends[i] = None;
                    continue;
                }
                visible += 1;
                if coarse >= 1.0 {
                    self.tree_blends[i] = None;
                    continue;
                }
                let px = view.span(p.bounds);
                let full = if self.mid[p.model].index_count == 0 || !px.is_finite() {
                    self.tree_blends[i] = None;
                    1.0
                } else {
                    advance_rank(
                        &mut self.tree_blends[i],
                        smooth(px, self.mid_low, self.mid_high),
                        random((i as u32) ^ 0x51a39d77),
                        width,
                        dt,
                        seconds,
                    )
                };
                let [_, m, f] = lod_ranges(coarse, full);
                if f.1 > f.0 {
                    out.full[p.model].push(ranged(p.instance, f.0, f.1));
                    out.full_count += 1;
                }
                if m.1 > m.0 {
                    out.mid[p.model].push(ranged(p.instance, m.0, m.1));
                    out.mid_count += 1;
                }
                if full > 0.0 && full < 1.0 {
                    out.fades[0] += 1;
                }
            }
            if coarse > 0.0 {
                // Draw if the cell is visible even when its source tree bounds
                // are just outside: the shell may extend beyond them.
                out.far.push((
                    cell.draw,
                    ranged(identity(), -1.0, lod_ranges(coarse, 0.0)[0].1),
                ));
                out.far_count += visible;
                if coarse < 1.0 {
                    out.fades[1] += 1;
                }
            }
        }
        out
    }
}
