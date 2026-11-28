//! Builds nested index prefixes and matching vertex positions from each LOD0 mesh.
use glam::{IVec3, Vec2, Vec3};
use std::collections::{BTreeMap, HashMap};

/// Vertex positions at each simplified endpoint and the start of foliage retirement.
#[derive(Clone, Copy)]
pub struct LodVertex {
    pub middle: [f32; 3],
    pub far: [f32; 3],
    pub retire_at: f32,
}

/// Source vertices plus a single index ordering shared by the three detail levels.
pub struct MeshLod {
    pub vertices: Vec<LodVertex>,
    pub indices: Vec<u32>,
    /// Number of indices in the full, middle and far prefixes.
    pub counts: [usize; 3],
}

/// Build the three nested detail prefixes for one material of an indexed tree.
pub fn build(
    positions: &[[f32; 3]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    bounds: ([f32; 3], [f32; 3]),
    alpha_visible: Option<&dyn Fn([f32; 2]) -> bool>,
) -> MeshLod {
    let positions: Vec<_> = positions.iter().copied().map(Vec3::from).collect();
    let faces: Vec<[u32; 3]> = indices
        .chunks_exact(3)
        .map(|f| [f[0], f[1], f[2]])
        .collect();
    let cutoffs = vec![alpha_visible.map(|_| 0.0); faces.len()];
    let (lo, hi) = (Vec3::from(bounds.0), Vec3::from(bounds.1));
    let mut tiers = [Vec::new(), Vec::new(), Vec::new()];
    let mut vertices: Vec<_> = positions
        .iter()
        .map(|v| LodVertex {
            middle: v.to_array(),
            far: v.to_array(),
            retire_at: 3.0,
        })
        .collect();

    let mut wood = Wood::new(&positions, &faces, &cutoffs);
    let size = (hi - lo).max_element();
    let original = wood.faces.len();
    let middle = wood.simplify(size * 0.004, original * 42 / 100);
    let far = wood.simplify(size * 0.017, original * 19 / 100);
    for (i, &point) in wood.welded.iter().enumerate() {
        if point != usize::MAX {
            vertices[i].middle = middle[point].to_array();
            vertices[i].far = far[point].to_array();
        }
    }

    let foliage = alpha_visible.map(|sample| {
        foliage_tiers(
            &positions,
            uvs,
            lo,
            hi,
            &faces,
            &cutoffs,
            &mut vertices,
            size,
            sample,
        )
    });
    for (i, face) in faces.into_iter().enumerate() {
        let tier = if let Some(foliage) = &foliage {
            foliage[i]
        } else if degenerate(face, &vertices, |v| v.middle) {
            0
        } else if degenerate(face, &vertices, |v| v.far) {
            1
        } else {
            2
        };
        tiers[tier].extend(face);
    }
    let far = tiers[2].len();
    let middle = tiers[1].len() + far;
    let full = tiers[0].len() + middle;
    MeshLod {
        vertices,
        indices: tiers.into_iter().rev().flatten().collect(),
        counts: [full, middle, far],
    }
}

fn degenerate(
    f: [u32; 3],
    vertices: &[LodVertex],
    position: impl Fn(LodVertex) -> [f32; 3],
) -> bool {
    let [a, b, c] = f.map(|i| position(vertices[i as usize]));
    a == b || b == c || c == a
}

fn root(parent: &[usize], mut i: usize) -> usize {
    while i != parent[i] {
        i = parent[i];
    }
    i
}

fn join(parent: &mut [usize], a: usize, b: usize) {
    let a = root(parent, a);
    let b = root(parent, b);
    parent[a.max(b)] = a.min(b);
}

struct Foliage {
    faces: Vec<usize>,
    cluster: IVec3,
    hits: Vec<Hit>,
    importance: f32,
    tier: usize,
}

struct Hit {
    pixel: usize,
    depth: f32,
}

const GRID: usize = 64;
const VIEW_PIXELS: usize = GRID * GRID;
const VIEWS: usize = 17;

fn projections() -> [(Vec3, Vec3); VIEWS] {
    std::array::from_fn(|i| {
        if i == 16 {
            return (Vec3::X, Vec3::Z);
        }
        let yaw = i as f32 * std::f32::consts::TAU / 8.0;
        let pitch = if i < 8 { 0.0f32 } else { -0.83f32 };
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
        (right, forward.cross(right))
    })
}

fn foliage_tiers(
    positions: &[Vec3],
    uvs: &[[f32; 2]],
    lo: Vec3,
    hi: Vec3,
    faces: &[[u32; 3]],
    cutoffs: &[Option<f32>],
    vertices: &mut [LodVertex],
    size: f32,
    alpha_visible: &dyn Fn([f32; 2]) -> bool,
) -> Vec<usize> {
    // Keep connected foliage together, including the triangles of each source leaf card.
    let mut parent: Vec<_> = (0..vertices.len()).collect();
    for (f, cutoff) in faces.iter().zip(cutoffs) {
        if cutoff.is_some() {
            join(&mut parent, f[0] as usize, f[1] as usize);
            join(&mut parent, f[0] as usize, f[2] as usize);
        }
    }
    let mut components = BTreeMap::<usize, Vec<usize>>::new();
    for (i, f) in faces
        .iter()
        .enumerate()
        .filter(|(i, _)| cutoffs[*i].is_some())
    {
        components
            .entry(root(&parent, f[0] as usize))
            .or_default()
            .push(i);
    }

    let center = (lo + hi) * 0.5;
    let scale = (GRID as f32 - 2.0) / (hi - lo).length().max(0.001);
    let axes = projections();
    let model_lo = lo;
    let mut counts = vec![0u32; VIEWS * VIEW_PIXELS];
    let mut front = vec![f32::INFINITY; counts.len()];
    let mut clusters = HashMap::<IVec3, usize>::new();
    let mut foliage = Vec::with_capacity(components.len());
    for face_ids in components.into_values() {
        let mut hits = Vec::new();
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for &i in &face_ids {
            let p = faces[i].map(|j| positions[j as usize]);
            for point in p {
                lo = lo.min(point);
                hi = hi.max(point);
            }
            let uv = faces[i].map(|j| Vec2::from(uvs[j as usize]));
            for (view, (right, up)) in axes.iter().enumerate() {
                let forward = right.cross(*up);
                let depth = p.map(|p| (p - center).dot(forward));
                let screen = p.map(|p| {
                    let delta = p - center;
                    Vec2::new(delta.dot(*right), delta.dot(*up)) * scale
                        + Vec2::splat(GRID as f32 * 0.5)
                });
                rasterize(screen, |pixel, b, c| {
                    let uv = uv[0] + b * (uv[1] - uv[0]) + c * (uv[2] - uv[0]);
                    if alpha_visible(uv.to_array()) {
                        hits.push(Hit {
                            pixel: view * VIEW_PIXELS + pixel,
                            depth: depth[0] + b * (depth[1] - depth[0]) + c * (depth[2] - depth[0]),
                        });
                    }
                });
            }
        }
        hits.sort_unstable_by(|a, b| {
            a.pixel
                .cmp(&b.pixel)
                .then_with(|| a.depth.total_cmp(&b.depth))
        });
        hits.dedup_by_key(|hit| hit.pixel);
        for hit in &hits {
            counts[hit.pixel] += 1;
            front[hit.pixel] = front[hit.pixel].min(hit.depth);
        }
        let cluster = (((lo + hi) * 0.5 - model_lo) * (6.0 / size.max(0.001)))
            .floor()
            .as_ivec3();
        *clusters.entry(cluster).or_default() += 1;
        foliage.push(Foliage {
            faces: face_ids,
            cluster,
            hits,
            importance: 0.0,
            tier: 2,
        });
    }

    let mut original = [0u32; VIEWS];
    let weights: Vec<_> = counts
        .iter()
        .enumerate()
        .map(|(p, &n)| {
            if n == 0 {
                return 0;
            }
            let pixel = p % VIEW_PIXELS;
            let x = pixel % GRID;
            let y = pixel / GRID;
            let edge = x == 0
                || x == GRID - 1
                || y == 0
                || y == GRID - 1
                || [p - 1, p + 1, p - GRID, p + GRID]
                    .iter()
                    .any(|&q| counts[q] == 0);
            let weight = if edge { 3 } else { 1 };
            original[p / VIEW_PIXELS] += weight;
            weight
        })
        .collect();
    for card in &mut foliage {
        // Exposed, hard-to-replace cards contribute more to the projected silhouette.
        card.importance = card
            .hits
            .iter()
            .map(|hit| {
                let p = hit.pixel;
                let exterior = hit.depth - front[p] < size * 0.02;
                weights[p] as f32 * (1.0 / counts[p] as f32 + if exterior { 0.25 } else { 0.0 })
            })
            .sum::<f32>()
            / card.hits.len().max(1) as f32;
    }
    foliage.sort_by(|a, b| a.importance.total_cmp(&b.importance));

    let mut lost = [0u32; VIEWS];
    for (stage, retained) in [98, 95].into_iter().enumerate() {
        let mut retired = Vec::new();
        for (id, card) in foliage.iter_mut().enumerate() {
            if card.tier != 2 || clusters[&card.cluster] <= 1 {
                continue;
            }
            let mut loss = lost;
            for hit in &card.hits {
                let p = hit.pixel;
                if counts[p] == 1 {
                    loss[p / VIEW_PIXELS] += weights[p];
                }
            }
            if (0..VIEWS).any(|i| loss[i] * 100 > original[i] * (100 - retained)) {
                continue;
            }
            lost = loss;
            for hit in &card.hits {
                counts[hit.pixel] -= 1;
            }
            *clusters.get_mut(&card.cluster).unwrap() -= 1;
            card.tier = stage;
            retired.push(id);
        }
        // Retire the least significant detail first. All representatives are
        // already present, so their depth/alpha coverage is shared during handoff.
        let scale = 0.6 / retired.len().max(1) as f32;
        for (rank, id) in retired.into_iter().enumerate() {
            for &face in &foliage[id].faces {
                for vertex in faces[face] {
                    vertices[vertex as usize].retire_at = stage as f32 + scale * rank as f32;
                }
            }
        }
    }
    let mut tiers = vec![2; faces.len()];
    for card in foliage {
        for face in card.faces {
            tiers[face] = card.tier;
        }
    }
    tiers
}

fn rasterize([a, b, c]: [Vec2; 3], mut sample: impl FnMut(usize, f32, f32)) {
    let ab = b - a;
    let ac = c - a;
    let area = ab.perp_dot(ac);
    if area.abs() < 1e-6 {
        return;
    }
    let lo = a.min(b).min(c).floor().max(Vec2::ZERO).as_uvec2();
    let hi = a
        .max(b)
        .max(c)
        .ceil()
        .min(Vec2::splat(GRID as f32))
        .as_uvec2();
    for y in lo.y..hi.y {
        for x in lo.x..hi.x {
            let d = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) - a;
            let u = d.perp_dot(ac) / area;
            let v = ab.perp_dot(d) / area;
            if u >= 0.0 && v >= 0.0 && u + v <= 1.0 {
                sample(y as usize * GRID + x as usize, u, v);
            }
        }
    }
}

struct Wood {
    welded: Vec<usize>,
    positions: Vec<Vec3>,
    faces: Vec<[usize; 3]>,
    parent: Vec<usize>,
    radius: Vec<f32>,
}

impl Wood {
    fn new(positions: &[Vec3], faces: &[[u32; 3]], cutoffs: &[Option<f32>]) -> Self {
        let mut welded = vec![usize::MAX; positions.len()];
        let mut points = Vec::new();
        let mut lookup = HashMap::new();
        let mut triangles = Vec::new();
        for (face, cutoff) in faces.iter().zip(cutoffs) {
            if cutoff.is_some() {
                continue;
            }
            let f = face.map(|i| {
                let index = i as usize;
                if welded[index] == usize::MAX {
                    let p = positions[index];
                    // Weld only identical positions for topology. Source UV
                    // seams stay separate in the actual render vertex stream.
                    let key = p.to_array().map(|v| if v == 0.0 { 0 } else { v.to_bits() });
                    welded[index] = *lookup.entry(key).or_insert_with(|| {
                        points.push(p);
                        points.len() - 1
                    });
                }
                welded[index]
            });
            if f[0] != f[1] && f[1] != f[2] && f[2] != f[0] {
                triangles.push(f);
            }
        }
        let count = points.len();
        Self {
            welded,
            positions: points,
            faces: triangles,
            parent: (0..count).collect(),
            radius: vec![0.0; count],
        }
    }

    fn simplify(&mut self, limit: f32, target: usize) -> Vec<Vec3> {
        while self.faces.len() > target && self.collapse_pass(limit, target) {}
        self.parent.iter().map(|&i| self.positions[i]).collect()
    }

    fn collapse_pass(&mut self, limit: f32, target: usize) -> bool {
        let mut adjacent = vec![Vec::new(); self.positions.len()];
        let mut incident = vec![Vec::new(); self.positions.len()];
        let mut edges = HashMap::<(usize, usize), [Option<usize>; 2]>::new();
        let mut non_manifold = vec![false; self.positions.len()];
        for (id, &[a, b, c]) in self.faces.iter().enumerate() {
            for (a, b, opposite) in [(a, b, c), (b, c, a), (c, a, b)] {
                incident[a].push(id);
                let edge = (a.min(b), a.max(b));
                let opposites = edges.entry(edge).or_default();
                match opposites {
                    [None, _] => {
                        opposites[0] = Some(opposite);
                        adjacent[a].push(b);
                        adjacent[b].push(a);
                    }
                    [_, None] => opposites[1] = Some(opposite),
                    _ => {
                        non_manifold[a] = true;
                        non_manifold[b] = true;
                    }
                }
            }
        }
        let mut locked = non_manifold;
        let mut candidates = Vec::new();
        for ((a, b), opposites) in edges {
            // Keep open ends, attachment seams and non-manifold junctions fixed.
            if let [Some(c), Some(d)] = opposites {
                let length = self.positions[a].distance(self.positions[b]);
                if c != d && length <= limit {
                    candidates.push((length, a, b, c, d));
                }
            } else {
                locked[a] = true;
                locked[b] = true;
            }
        }
        candidates.sort_unstable_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| (a.1, a.2).cmp(&(b.1, b.2)))
        });

        let mut replacements: Vec<_> = (0..self.positions.len()).collect();
        let mut remaining = self.faces.len();
        for (length, a, b, c, d) in candidates {
            if remaining <= target {
                break;
            }
            if locked[a]
                || locked[b]
                || adjacent[a]
                    .iter()
                    .any(|n| *n != c && *n != d && adjacent[b].contains(n))
                || (self.has_face(&incident[a], c, d) && self.has_face(&incident[b], c, d))
            {
                continue;
            }
            let (from, to) = [(a, b), (b, a)]
                .into_iter()
                .filter(|&(from, to)| {
                    self.radius[from] + length <= limit
                        && self.can_collapse(from, to, &incident[from])
                })
                .min_by(|&(fa, ta), &(fb, tb)| {
                    self.radius[ta]
                        .max(self.radius[fa] + length)
                        .total_cmp(&self.radius[tb].max(self.radius[fb] + length))
                })
                .unwrap_or((a, a));
            if from == to {
                continue;
            }
            replacements[from] = to;
            self.radius[to] = self.radius[to].max(self.radius[from] + length);
            locked[a] = true;
            locked[b] = true;
            // Affected faces have at most one edge contraction per pass.
            for &n in adjacent[a].iter().chain(&adjacent[b]) {
                locked[n] = true;
            }
            remaining -= 2;
        }
        if remaining == self.faces.len() {
            return false;
        }
        for i in &mut self.parent {
            *i = replacements[*i];
        }
        self.faces.retain_mut(|f| {
            *f = f.map(|i| replacements[i]);
            f[0] != f[1] && f[1] != f[2] && f[2] != f[0]
        });
        true
    }

    fn can_collapse(&self, from: usize, to: usize, faces: &[usize]) -> bool {
        faces.iter().all(|&i| {
            let f = self.faces[i];
            if f.contains(&to) {
                return true;
            }
            let [a, b, c] = f.map(|v| self.positions[v]);
            let [p, q, r] = f.map(|v| self.positions[if v == from { to } else { v }]);
            let old = (b - a).cross(c - a);
            let new = (q - p).cross(r - p);
            old.dot(new) > 0.5 * old.length() * new.length()
        })
    }

    fn has_face(&self, faces: &[usize], a: usize, b: usize) -> bool {
        faces
            .iter()
            .any(|&i| self.faces[i].contains(&a) && self.faces[i].contains(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::{rasterize, Wood};
    use glam::{Vec2, Vec3};

    #[test]
    fn coverage_raster_accepts_both_triangle_windings() {
        let [a, b, c] = [Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(0.0, 4.0)];
        let mut positive = Vec::new();
        let mut negative = Vec::new();
        rasterize([a, b, c], |pixel, _, _| positive.push(pixel));
        rasterize([a, c, b], |pixel, _, _| negative.push(pixel));
        assert!(!positive.is_empty());
        assert_eq!(positive, negative);
    }

    #[test]
    fn wood_simplification_keeps_a_closed_tip() {
        let positions = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
        let faces = [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]];
        let mut wood = Wood::new(&positions, &faces, &[None; 4]);
        assert_eq!(wood.simplify(2.0, 2), positions);
        assert_eq!(wood.faces.len(), 4);
    }
}
