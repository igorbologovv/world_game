//! Indexed heightfield meshes with bounded quadtree refinement on the CPU.
use std::collections::{HashMap, HashSet};

use bevy::{
    math::{DVec2, DVec3},
    pbr::{NotShadowCaster, FogFalloff},
    prelude::*,
    asset::RenderAssetUsages,
    mesh::{Indices, PrimitiveTopology},
};
use world_core::{
    coordinates::cube_direction,
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrame, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use crate::TerrainMaterial;

const GRID: usize = 20;
const BASE_LEVEL: u8 = 1;
const MAX_LEVEL: u8 = 11;
const MAX_CHUNKS: usize = 640;
const SPLITS_PER_FRAME: usize = 3;
const MERGES_PER_FRAME: usize = 5;
const SPLIT_ERROR: f64 = 0.78;
const MERGE_ERROR: f64 = 0.49;

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

    fn error(self, eye: DVec3, anchor: DVec3) -> f64 {
        let width = RADIUS * 1.65 * self.unit();
        width / (eye.distance(anchor) - width * 0.42).max(1.0)
    }
}

struct Chunk {
    entity: Entity,
    anchor: DVec3,
}

#[derive(Resource)]
pub(super) struct TerrainSurface {
    material: Handle<TerrainMaterial>,
    leaves: HashMap<Tile, Chunk>,
}

impl TerrainSurface {
    pub(super) fn new(material: Handle<TerrainMaterial>) -> Self {
        Self {
            material,
            leaves: HashMap::new(),
        }
    }

    fn retire(&mut self, tile: Tile, commands: &mut Commands) {
        if let Some(chunk) = self.leaves.remove(&tile) {
            commands.entity(chunk.entity).despawn();
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

    let material = terrain.material.clone();
    let context = TileContext {
        root: root.entity(),
        frame,
        heights: &heights,
        material: &material,
    };
    if terrain.leaves.is_empty() {
        for face in 0..6 {
            for tile in Tile::root(face).children() {
                terrain
                    .leaves
                    .insert(tile, context.spawn(tile, &mut commands, &mut meshes));
            }
        }
    }

    let parents: HashSet<_> = terrain
        .leaves
        .keys()
        .filter_map(|tile| tile.parent())
        .filter(|parent| parent.level >= BASE_LEVEL)
        .collect();
    let mut merges: Vec<_> = parents
        .into_iter()
        .filter_map(|parent| {
            if !parent
                .children()
                .iter()
                .all(|child| terrain.leaves.contains_key(child))
            {
                return None;
            }
            let error = parent.error(eye, parent.anchor(&heights));
            (error < MERGE_ERROR).then_some((error, parent))
        })
        .collect();
    merges.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    for (_, parent) in merges.into_iter().take(MERGES_PER_FRAME) {
        let replacement = context.spawn(parent, &mut commands, &mut meshes);
        for child in parent.children() {
            terrain.retire(child, &mut commands);
        }
        terrain.leaves.insert(parent, replacement);
    }

    let available = (MAX_CHUNKS.saturating_sub(terrain.leaves.len()) / 3).min(SPLITS_PER_FRAME);
    if available == 0 {
        return;
    }
    let mut splits: Vec<_> = terrain
        .leaves
        .iter()
        .filter_map(|(&tile, chunk)| {
            let error = tile.error(eye, chunk.anchor);
            (tile.level < MAX_LEVEL && error > SPLIT_ERROR).then_some((error, tile))
        })
        .collect();
    splits.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    for (_, tile) in splits.into_iter().take(available) {
        // Queue all four replacements together so extraction sees a complete surface.
        for child in tile.children() {
            let replacement = context.spawn(child, &mut commands, &mut meshes);
            terrain.leaves.insert(child, replacement);
        }
        terrain.retire(tile, &mut commands);
    }
}

struct TileContext<'a> {
    root: Entity,
    frame: &'a ReferenceFrame,
    heights: &'a SurfaceHeights,
    material: &'a Handle<TerrainMaterial>,
}

impl TileContext<'_> {
    fn spawn(&self, tile: Tile, commands: &mut Commands, meshes: &mut Assets<Mesh>) -> Chunk {
        let anchor = tile.anchor(self.heights);
        let (cell, translation) = self.frame.translation_to_grid(anchor);
        let mesh = build_mesh(tile, anchor, self.heights);
        let bounds = mesh
            .get_aabb()
            .expect("terrain chunks contain a full grid");
        let entity = commands
            .spawn((
                cell,
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(self.material.clone()),
                Transform::from_translation(translation),
                bounds,
                NotShadowCaster,
            ))
            .id();
        commands.entity(self.root).add_child(entity);
        Chunk { entity, anchor }
    }
}

fn build_mesh(tile: Tile, anchor: DVec3, heights: &SurfaceHeights) -> Mesh {
    let stride = GRID + 1;
    let mut positions = Vec::with_capacity(stride * stride + 4 * GRID);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut locations = Vec::with_capacity(positions.capacity());
    let mut indices = Vec::with_capacity((GRID * GRID + 4 * GRID) * 6);
    for y in 0..=GRID {
        for x in 0..=GRID {
            let up = tile.direction(DVec2::new(x as f64, y as f64) / GRID as f64);
            let height = heights.sample(up).unwrap_or(0.0);
            let position = up * (RADIUS + height);
            positions.push((position - anchor).as_vec3().to_array());
            normals.push(ground_normal(up, height, heights).as_vec3().to_array());
            let planet = position.as_vec3();
            locations.push([planet.x, planet.y, planet.z, height as f32]);
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
    let depth = (RADIUS * tile.unit() / GRID as f64 * 0.46).max(0.5);
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

fn ground_normal(up: DVec3, height: f64, heights: &SurfaceHeights) -> DVec3 {
    let reference = if up.y.abs() < 0.93 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let east = up.cross(reference).normalize();
    let north = east.cross(up);
    let step = 8.0 / RADIUS;
    let sample = |tangent: DVec3| {
        let displaced = (up + tangent * step).normalize();
        heights.sample(displaced).unwrap_or(height)
    };
    let slope_x = (sample(east) - sample(-east)) / 16.0;
    let slope_y = (sample(north) - sample(-north)) / 16.0;
    (up - east * slope_x - north * slope_y).normalize()
}

pub(super) fn atmosphere(
    mut cameras: Query<(Entity, GridTransform, &mut Camera, &mut DistanceFog), With<WorldCamera>>,
    frames: ReferenceFrames,
) {
    let Ok((entity, transform, mut camera, mut fog)) = cameras.single_mut() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&transform.cell, &transform.transform);
    let altitude = (eye.length() - RADIUS).max(0.0) as f32;
    let space = ((altitude - 3_000.0) / 40_000.0).clamp(0.0, 1.0);
    let sky = Vec3::new(0.57, 0.72, 0.82).lerp(Vec3::new(0.026, 0.035, 0.066), space);
    camera.clear_color = ClearColorConfig::Custom(Color::srgb(sky.x, sky.y, sky.z));
    let haze = 1.0 - ((altitude - 1_400.0) / 10_000.0).clamp(0.0, 1.0);
    fog.color = Color::srgba(0.59, 0.72, 0.81, 0.65 * haze);
    fog.falloff = FogFalloff::Linear {
        start: 100.0 + altitude * 0.7,
        end: 1150.0 + altitude * 4.0,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert!(root.error(center * 400_000.0, center * RADIUS) < SPLIT_ERROR);
            assert!(root.error(center * (RADIUS + 2.2), center * RADIUS) > SPLIT_ERROR);
        }
    }
}
