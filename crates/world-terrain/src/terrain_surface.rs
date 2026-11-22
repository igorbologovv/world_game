//! Indexed heightfield meshes with bounded quadtree refinement on the CPU.
use std::collections::{HashMap, HashSet};

use bevy::{
    asset::RenderAssetUsages,
    camera::primitives::{Aabb, MeshAabb},
    light::NotShadowCaster,
    math::{DVec2, DVec3},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use world_core::{
    coordinates::cube_direction,
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use crate::TerrainMaterial;

const GRID: usize = 20;
const BASE_LEVEL: u8 = 1;
// At the finest level, even the most stretched cube-face edges stay near
// one meter per vertex. The global chunk and job budgets still apply.
const MAX_LEVEL: u8 = 19;
const MAX_CHUNKS: usize = 960;
const MAX_JOBS: usize = 6;
const UPLOADS_PER_FRAME: usize = 8;
// Split when local geometric error becomes visible, even on a Mars-sized
// planet. Parent tiles remain visible until replacements are submitted.
const SPLIT_ERROR: f64 = 0.78;
const MERGE_ERROR: f64 = 0.49;
const DETAIL_ALTITUDE: f64 = 150.0;

#[derive(Clone, Copy)]
struct LodView {
    eye: DVec3,
    surface: DVec3,
    altitude: f64,
}

impl LodView {
    fn new(eye: DVec3, heights: &SurfaceHeights) -> Self {
        let up = eye.normalize_or_zero();
        let ground = heights.sample(up).unwrap_or(0.0);
        Self {
            eye,
            surface: up * RADIUS,
            altitude: (eye.length() - RADIUS - ground).max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Tile {
    face: usize,
    level: u8,
    x: u32,
    y: u32,
}

impl Tile {
    fn root(face: usize) -> Self {
        Self {
            face,
            level: 0,
            x: 0,
            y: 0,
        }
    }

    #[cfg(test)]
    fn parent(self) -> Option<Self> {
        (self.level > 0).then(|| Self {
            level: self.level - 1,
            x: self.x / 2,
            y: self.y / 2,
            ..self
        })
    }

    fn children(self) -> [Self; 4] {
        [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(x, y)| Self {
            level: self.level + 1,
            x: self.x * 2 + x,
            y: self.y * 2 + y,
            ..self
        })
    }

    fn unit(self) -> f64 {
        1.0 / f64::from(1_u32 << self.level)
    }

    fn direction(self, local: DVec2) -> DVec3 {
        let start = DVec2::new(f64::from(self.x), f64::from(self.y));
        cube_direction(self.face, (start + local) * self.unit())
    }

    fn anchor(self, heights: &SurfaceHeights) -> DVec3 {
        let up = self.direction(DVec2::splat(0.5));
        up * (RADIUS + heights.sample(up).unwrap_or(0.0))
    }

    fn error(self, view: LodView, anchor: DVec3) -> f64 {
        let width = RADIUS * 1.65 * self.unit();
        let footprint = width * 0.42;
        let visual = width / (view.eye.distance(anchor) - footprint).max(1.0);
        if self.level < 12 || view.altitude >= DETAIL_ALTITUDE {
            return visual;
        }
        // Reserve fine geometry around the surface position even when a
        // chunk center sits on the opposite slope of a hill or gully.
        let proximity = 1.0 - view.altitude / DETAIL_ALTITUDE;
        let distance = view.surface.distance(anchor.normalize() * RADIUS);
        let ground = width / (distance * 0.30 + view.altitude * 0.45 - footprint).max(1.0);
        visual.max(ground * proximity)
    }
}

struct Chunk {
    entity: Entity,
    anchor: DVec3,
    // A strong mesh handle must be explicitly released from Assets<Mesh>
    // when a tile is retired; despawning its entity alone is not enough.
    mesh: Handle<Mesh>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit {
    Roots,
    Split(Tile),
    Merge(Tile),
}

impl Edit {
    fn mesh_count(self) -> usize {
        match self {
            Self::Roots => 6,
            Self::Split(_) => 4,
            Self::Merge(_) => 1,
        }
    }

    fn reserve(self, reserved: &mut HashSet<Tile>) {
        match self {
            Self::Roots => {}
            Self::Split(tile) => {
                reserved.insert(tile);
            }
            Self::Merge(tile) => {
                reserved.extend(tile.children());
            }
        }
    }
}

struct PreparedTile {
    tile: Tile,
    anchor: DVec3,
    bounds: Aabb,
    mesh: Mesh,
}

impl PreparedTile {
    fn build(tile: Tile, heights: &SurfaceHeights) -> Self {
        let anchor = tile.anchor(heights);
        let mesh = build_mesh(tile, anchor, heights);
        let bounds = mesh.get_aabb().expect("terrain chunks contain a full grid");
        Self {
            tile,
            anchor,
            bounds,
            mesh,
        }
    }
}

struct TerrainJob {
    edit: Edit,
    task: Task<Vec<PreparedTile>>,
}

#[derive(Resource)]
pub(super) struct TerrainSurface {
    pub(super) material: Handle<TerrainMaterial>,
    leaves: HashMap<Tile, Chunk>,
    // Active internal nodes retain the exact sampled center without keeping
    // their mesh or resampling procedural elevation on every merge scan.
    parents: HashMap<Tile, DVec3>,
    pending: Vec<TerrainJob>,
}

impl TerrainSurface {
    pub(super) fn new(material: Handle<TerrainMaterial>) -> Self {
        Self {
            material,
            leaves: HashMap::new(),
            parents: HashMap::new(),
            pending: Vec::new(),
        }
    }

    fn request(&mut self, edit: Edit, heights: &SurfaceHeights) {
        let heights = heights.clone();
        let task = AsyncComputeTaskPool::get().spawn(async move {
            let tiles = match edit {
                Edit::Roots => (0..6).map(Tile::root).collect(),
                Edit::Split(tile) => tile.children().into(),
                Edit::Merge(tile) => vec![tile],
            };
            tiles
                .into_iter()
                .map(|tile| PreparedTile::build(tile, &heights))
                .collect()
        });
        self.pending.push(TerrainJob { edit, task });
    }

    fn retire(&mut self, tile: Tile, commands: &mut Commands, meshes: &mut Assets<Mesh>) -> Chunk {
        let chunk = self
            .leaves
            .remove(&tile)
            .expect("terrain edit owns its resident leaves");
        commands.entity(chunk.entity).despawn();
        meshes.remove(&chunk.mesh);
        chunk
    }

    fn eligible(&self, edit: Edit, view: LodView) -> bool {
        match edit {
            Edit::Roots => self.leaves.is_empty(),
            Edit::Split(tile) => self.leaves.get(&tile).is_some_and(|chunk| {
                tile.level < BASE_LEVEL || tile.error(view, chunk.anchor) > SPLIT_ERROR
            }),
            Edit::Merge(tile) => self.parents.get(&tile).is_some_and(|&anchor| {
                tile.error(view, anchor) < MERGE_ERROR
                    && tile
                        .children()
                        .iter()
                        .all(|child| self.leaves.contains_key(child))
            }),
        }
    }

    fn plan(&mut self, view: LodView, heights: &SurfaceHeights) {
        if self.leaves.is_empty() {
            if self.pending.is_empty() {
                self.request(Edit::Roots, heights);
            }
            return;
        }
        if self.pending.len() >= MAX_JOBS {
            return;
        }
        let mut reserved = HashSet::with_capacity(MAX_JOBS * 4);
        for job in &self.pending {
            job.edit.reserve(&mut reserved);
        }
        let mut merges: Vec<_> =
            self.parents
                .iter()
                .filter_map(|(&tile, &anchor)| {
                    let error = tile.error(view, anchor);
                    (tile.level >= BASE_LEVEL
                        && error < MERGE_ERROR
                        && tile.children().iter().all(|child| {
                            self.leaves.contains_key(child) && !reserved.contains(child)
                        }))
                    .then_some((error, tile))
                })
                .collect();
        merges.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        // Keep capacity for refinement near the camera while distant tiles coarsen.
        for (_, tile) in merges
            .into_iter()
            .take((MAX_JOBS - self.pending.len()).div_ceil(2))
        {
            let edit = Edit::Merge(tile);
            edit.reserve(&mut reserved);
            self.request(edit, heights);
        }

        let pending_splits = self
            .pending
            .iter()
            .filter(|job| matches!(job.edit, Edit::Split(_)))
            .count();
        let available = MAX_CHUNKS.saturating_sub(self.leaves.len() + 3 * pending_splits) / 3;
        let available = available.min(MAX_JOBS - self.pending.len());
        if available == 0 {
            return;
        }
        let mut splits: Vec<_> = self
            .leaves
            .iter()
            .filter_map(|(&tile, chunk)| {
                if reserved.contains(&tile) || tile.level >= MAX_LEVEL {
                    return None;
                }
                let error = tile.error(view, chunk.anchor);
                (tile.level < BASE_LEVEL || error > SPLIT_ERROR).then_some((error, tile))
            })
            .collect();
        splits.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (_, tile) in splits.into_iter().take(available) {
            self.request(Edit::Split(tile), heights);
        }
    }
}

pub(super) fn stream(
    mut commands: Commands,
    mut terrain: ResMut<TerrainSurface>,
    mut meshes: ResMut<Assets<Mesh>>,
    heights: Res<SurfaceHeights>,
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    root: Res<PlanetRoot>,
) {
    let Ok((camera_entity, camera)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(camera_entity) else {
        return;
    };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let view = LodView::new(eye, &heights);

    let mut uploads = UPLOADS_PER_FRAME;
    let mut index = 0;
    while index < terrain.pending.len() {
        let edit = terrain.pending[index].edit;
        let count = edit.mesh_count();
        if !terrain.eligible(edit, view) {
            terrain.pending.swap_remove(index);
            continue;
        }
        let Some(tiles) = (count <= uploads)
            .then(|| block_on(poll_once(&mut terrain.pending[index].task)))
            .flatten()
        else {
            index += 1;
            continue;
        };
        terrain.pending.swap_remove(index);
        uploads -= count;
        // Submit all replacements in one update. Retire old coverage only
        // after the complete replacement is available.
        for prepared in tiles {
            let PreparedTile {
                tile,
                anchor,
                bounds,
                mesh,
            } = prepared;
            let (cell, translation) = frame.translation_to_grid(anchor);
            let mesh = meshes.add(mesh);
            let entity = root
                .spawn(
                    &mut commands,
                    (
                        cell,
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(terrain.material.clone()),
                        Transform::from_translation(translation),
                        bounds,
                        NotShadowCaster,
                    ),
                )
                .id();
            terrain.leaves.insert(
                tile,
                Chunk {
                    entity,
                    anchor,
                    mesh,
                },
            );
        }
        match edit {
            Edit::Roots => {}
            Edit::Split(parent) => {
                let parent_chunk = terrain.retire(parent, &mut commands, &mut meshes);
                terrain.parents.insert(parent, parent_chunk.anchor);
            }
            Edit::Merge(parent) => {
                for child in parent.children() {
                    terrain.retire(child, &mut commands, &mut meshes);
                }
                terrain.parents.remove(&parent);
            }
        }
    }
    terrain.plan(view, &heights);
}

fn build_mesh(tile: Tile, anchor: DVec3, heights: &SurfaceHeights) -> Mesh {
    let stride = GRID + 1;
    let mut positions = Vec::with_capacity(stride * stride + 4 * GRID);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut locations = Vec::with_capacity(positions.capacity());
    let mut indices = Vec::with_capacity((GRID * GRID + 4 * GRID) * 6);
    let spacing = RADIUS * tile.unit() / GRID as f64;
    let normal_step = spacing.max(0.8);
    for y in 0..=GRID {
        for x in 0..=GRID {
            let up = tile.direction(DVec2::new(x as f64, y as f64) / GRID as f64);
            let height = heights.sample(up).unwrap_or(0.0);
            let position = up * (RADIUS + height);
            positions.push((position - anchor).as_vec3().to_array());
            let planet = position.as_vec3();
            locations.push([planet.x, planet.y, planet.z, height as f32]);
        }
    }

    for y in 0..=GRID {
        for x in 0..=GRID {
            let index = y * stride + x;
            let normal = if x > 0 && x < GRID && y > 0 && y < GRID {
                // Reuse the mesh samples, including geometry at meter scales.
                let position = |i| Vec3::from_array(positions[i]);
                let dx = position(index + 1) - position(index - 1);
                let dy = position(index + stride) - position(index - stride);
                dy.cross(dx).normalize()
            } else {
                // Edge normals remain identical between same-level tiles.
                let up = tile.direction(DVec2::new(x as f64, y as f64) / GRID as f64);
                ground_normal(up, f64::from(locations[index][3]), normal_step, heights).as_vec3()
            };
            normals.push(normal.to_array());
        }
    }

    for y in 0..GRID {
        for x in 0..GRID {
            let a = (y * stride + x) as u16;
            let b = a + 1;
            let c = a + stride as u16;
            indices.extend_from_slice(&[a, c, b, b, c, c + 1]);
        }
    }
    // Each perimeter has an inward skirt to cover mismatched edge samples
    // while adjacent chunks are loading at different levels of detail.
    let edge: Vec<_> = (0..GRID)
        .chain((0..GRID).map(|y| y * stride + GRID))
        .chain((0..GRID).map(|x| GRID * stride + GRID - x))
        .chain((0..GRID).map(|y| (GRID - y) * stride))
        .collect();
    let skirt_start = positions.len() as u16;
    let depth = spacing.max(1.25);
    for (i, &a) in edge.iter().enumerate() {
        let next = (i + 1) % edge.len();
        let b = edge[next] as u16;
        let low_a = skirt_start + i as u16;
        let low_b = skirt_start + next as u16;
        let position = Vec3::from_array(positions[a]).as_dvec3() + anchor;
        let lowered = position - position.normalize() * depth;
        positions.push((lowered - anchor).as_vec3().to_array());
        normals.push(normals[a]);
        locations.push(locations[a]);
        let a = a as u16;
        indices.extend_from_slice(&[a, b, low_b, a, low_b, low_a, a, low_b, b, a, low_a, low_b]);
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    // Preserve a planet-local position across floating-origin shifts.
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, locations);
    mesh.insert_indices(Indices::U16(indices));
    mesh
}

fn ground_normal(up: DVec3, height: f64, distance: f64, heights: &SurfaceHeights) -> DVec3 {
    let reference = if up.y.abs() < 0.93 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let east = up.cross(reference).normalize();
    let north = east.cross(up);
    let step = distance / RADIUS;
    let sample = |tangent: DVec3| {
        let displaced = (up + tangent * step).normalize();
        heights.sample(displaced).unwrap_or(height)
    };
    let slope_x = (sample(east) - sample(-east)) / (2.0 * distance);
    let slope_y = (sample(north) - sample(-north)) / (2.0 * distance);
    (up - east * slope_x - north * slope_y).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_tiles_share_exact_radial_border_directions() {
        for level in [4_u8, 11, MAX_LEVEL] {
            let left = Tile {
                face: 2,
                level,
                x: 0,
                y: 0,
            };
            let right = Tile { x: 1, ..left };
            for i in 0..=GRID {
                let t = i as f64 / GRID as f64;
                let a = left.direction(DVec2::new(1.0, t));
                let b = right.direction(DVec2::new(0.0, t));
                assert!(
                    a.distance(b) < 1e-12,
                    "mismatched tile border at level {level}"
                );
            }
        }
    }

    #[test]
    fn child_quadrants_cover_the_parent_and_share_their_edges() {
        for face in 0..6 {
            let root = Tile::root(face);
            let [top_left, top_right, bottom_left, bottom_right] = root.children();
            assert!(root.parent().is_none());
            assert!(root
                .children()
                .iter()
                .all(|tile| tile.parent() == Some(root)));
            let center = root.direction(DVec2::splat(0.5));
            assert_eq!(center, top_left.direction(DVec2::ONE));
            assert_eq!(center, top_right.direction(DVec2::Y));
            assert_eq!(center, bottom_left.direction(DVec2::X));
            assert_eq!(center, bottom_right.direction(DVec2::ZERO));
            let view = |altitude| LodView {
                eye: center * (RADIUS + altitude),
                surface: center * RADIUS,
                altitude,
            };
            assert!(root.error(view(3.0 * RADIUS), center * RADIUS) < SPLIT_ERROR);
            assert!(root.error(view(2.2), center * RADIUS) > SPLIT_ERROR);
        }
    }

    #[test]
    fn near_ground_tiles_refine_even_with_a_large_center_height_offset() {
        let tile = Tile {
            level: MAX_LEVEL - 1,
            ..Tile::root(2)
        };
        let up = tile.direction(DVec2::splat(0.5));
        let anchor = up * (RADIUS + 500.0);
        let view = LodView {
            eye: up * (RADIUS + 432.2),
            surface: up * RADIUS,
            altitude: 2.2,
        };
        assert!(tile.error(view, anchor) > SPLIT_ERROR);
        let orbit = LodView {
            eye: up * (RADIUS + 10_000.0),
            altitude: 9_600.0,
            ..view
        };
        assert!(tile.error(orbit, anchor) < MERGE_ERROR);
        let finest = tile.children()[0];
        let a = finest.direction(DVec2::ZERO);
        let b = finest.direction(DVec2::X / GRID as f64);
        assert!(RADIUS * a.distance(b) < 1.25);
    }

    #[test]
    fn finest_coastal_mesh_contains_shared_sub_texel_relief() {
        use bevy::mesh::VertexAttributeValues;
        use world_core::coordinates::{surface_uv, terrain_aligned_direction};

        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let heights = SurfaceHeights::load(&assets);
        let up = terrain_aligned_direction(DVec3::new(-0.97685, 0.21339, 0.01513));
        let (face, uv) = surface_uv(up);
        let cells = (uv * f64::from(1_u32 << MAX_LEVEL)).as_uvec2();
        let tile = Tile {
            face,
            level: MAX_LEVEL,
            x: cells.x,
            y: cells.y,
        };
        let mesh = PreparedTile::build(tile, &heights);
        let VertexAttributeValues::Float32x3(positions) =
            mesh.mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap()
        else {
            panic!("expected three-dimensional terrain vertices");
        };
        let mut relief: f64 = 0.0;
        let first = (mesh.anchor + Vec3::from_array(positions[0]).as_dvec3()).length();
        for y in 0..=GRID {
            for x in 0..=GRID {
                let local = DVec2::new(x as f64, y as f64) / GRID as f64;
                let up = tile.direction(local);
                let actual =
                    mesh.anchor + Vec3::from_array(positions[y * (GRID + 1) + x]).as_dvec3();
                let ground = heights.sample(up).unwrap();
                assert!(actual.distance(up * (RADIUS + ground)) < 1e-5);
                relief = relief.max((actual.length() - first).abs());
            }
        }
        assert!(relief > 0.10, "finest mesh relief is only {relief} m");
    }
}
