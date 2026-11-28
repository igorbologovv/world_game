//! A depth-writing canopy surface built from the resident crown relief texture.

use std::ops::RangeInclusive;

use bevy::{
    asset::RenderAssetUsages,
    camera::primitives::{Aabb, MeshAabb},
    math::{DVec2, DVec3},
    mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology},
    pbr::{MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    render::render_resource::{
        AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::{ShaderDefVal, ShaderRef},
};
use world_core::{
    canopy::{CanopyLayout, ForestCanopy},
    planet::RADIUS,
    runtime::{PlanetRoot, ReferenceFrame},
};

use super::{crowns, AtlasStream, Frame};

const CELLS: usize = crowns::SIDE / 8;
const STRIDE: usize = CELLS + 1;
const FADE_OFFSETS: Vec2 = Vec2::new(400.0, 1450.0);

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct CanopyMaterial {
    #[uniform(7)]
    planet_radius: f32,
    #[texture(8, dimension = "2d_array")]
    #[sampler(9)]
    atlas: Handle<Image>,
    #[uniform(10)]
    layout: CanopyLayout,
    #[texture(11)]
    #[sampler(12)]
    crowns: Handle<Image>,
    #[texture(13, dimension = "cube")]
    #[sampler(14)]
    globe: Handle<Image>,
    #[uniform(18)]
    far_fade: Vec2,
}

impl From<&ForestCanopy> for CanopyMaterial {
    fn from(canopy: &ForestCanopy) -> Self {
        Self {
            planet_radius: RADIUS as f32,
            atlas: canopy.atlas.clone(),
            layout: canopy.layout,
            crowns: canopy.crowns.clone(),
            globe: canopy.globe.clone(),
            far_fade: FADE_OFFSETS + Vec2::splat(canopy.layout.fade.y),
        }
    }
}

impl Material for CanopyMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/distant_canopy.wesl".into()
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(fragment) = &mut descriptor.fragment {
            fragment.shader_defs.push(ShaderDefVal::UInt(
                "CANOPY_SAMPLES".into(),
                key.mesh_key.msaa_samples(),
            ));
        }
        Ok(())
    }
}

pub(super) struct PreparedMesh {
    mesh: Mesh,
    bounds: Aabb,
    anchor: DVec3,
    radii: RangeInclusive<f64>,
}

impl PreparedMesh {
    pub(super) fn build(
        frame: Frame,
        relief: &[u8],
        mut ground: impl FnMut(DVec3) -> Option<f64>,
    ) -> Option<Self> {
        // Use the existing 128² mip to shape the mesh. The fragment shader
        // samples the full mip chain to retain narrow gaps inside each quad.
        let mip_start = (crowns::SIDE.pow(2) - CELLS.pow(2)) * 4 / 3;
        let samples = &relief[mip_start * 4..][..CELLS * CELLS * 4];
        let mut relief = vec![None; STRIDE * STRIDE];
        for y in 0..STRIDE {
            for x in 0..STRIDE {
                let mut coverage = 0.0;
                let mut height = 0.0;
                for j in y.saturating_sub(1)..(y + 1).min(CELLS) {
                    for i in x.saturating_sub(1)..(x + 1).min(CELLS) {
                        let pixel = &samples[(j * CELLS + i) * 4..];
                        coverage += f64::from(pixel[0]);
                        height += f64::from(pixel[1]);
                    }
                }
                if coverage > 0.0 {
                    relief[y * STRIDE + x] =
                        Some(height / coverage * f64::from(crowns::HEIGHT_RANGE));
                }
            }
        }

        let anchor = frame.up * (RADIUS + ground(frame.up).unwrap_or(0.0));
        let mut positions = Vec::new();
        let mut planet_positions = Vec::new();
        let mut uvs = Vec::new();
        let mut indices = Vec::new();
        let mut vertices = vec![u16::MAX; STRIDE * STRIDE];
        let mut min_radius = f64::INFINITY;
        let mut max_radius = f64::NEG_INFINITY;
        for y in 0..CELLS {
            for x in 0..CELLS {
                let start = y * STRIDE + x;
                let corners = [start, start + 1, start + STRIDE, start + STRIDE + 1];
                if corners.iter().all(|&corner| relief[corner].is_none()) {
                    continue;
                }
                let [a, b, c, d] = corners.map(|corner| {
                    let index = &mut vertices[corner];
                    if *index == u16::MAX {
                        let uv = DVec2::new((corner % STRIDE) as f64, (corner / STRIDE) as f64)
                            / CELLS as f64;
                        let up = frame.direction(uv * 2.0 - 1.0);
                        let elevation = ground(up).unwrap_or(0.0);
                        let radius = RADIUS + elevation + relief[corner].unwrap_or(0.0);
                        let position = up * radius;
                        min_radius = min_radius.min(radius);
                        max_radius = max_radius.max(radius);
                        *index = positions.len() as u16;
                        positions.push((position - anchor).as_vec3().to_array());
                        planet_positions.push(position.as_vec3().extend(0.0).to_array());
                        uvs.push(uv.as_vec2().to_array());
                    }
                    *index
                });
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        if indices.is_empty() {
            return None;
        }
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, planet_positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_indices(Indices::U16(indices));
        let bounds = mesh.get_aabb()?;
        Some(Self {
            mesh,
            bounds,
            anchor,
            radii: min_radius..=max_radius,
        })
    }
}

struct ResidentMesh {
    entity: Entity,
    material: Handle<CanopyMaterial>,
    crowns: AssetId<Image>,
    radii: RangeInclusive<f64>,
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CanopyMaterial>::default())
        .add_systems(Update, publish.after(super::stream));
}

fn publish(
    mut commands: Commands,
    mut stream: ResMut<AtlasStream>,
    canopy: Res<ForestCanopy>,
    root: Res<PlanetRoot>,
    grids: Query<&ReferenceFrame>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<CanopyMaterial>>,
    mut visibility: Query<&mut Visibility>,
    mut resident: Local<Option<ResidentMesh>>,
) {
    let crown_id = canopy.crowns.id();
    if resident
        .as_ref()
        .is_some_and(|current| current.crowns != crown_id || stream.centers[0].is_none())
    {
        let current = resident.take().unwrap();
        commands.entity(current.entity).despawn();
    }
    let far = f64::from(canopy.layout.fade.y + FADE_OFFSETS.y);
    let eye_radius = stream.eye_radius;
    let visibility_for = |radii: &RangeInclusive<f64>| {
        if eye_radius
            .is_some_and(|eye| eye >= radii.start() - far - 1.0 && eye <= radii.end() + far + 1.0)
        {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    // Geometry and its crown image are published together. Atlas recentering
    // or construction edits cannot leave an old mesh sampling a new chart.
    if let Some(prepared) = stream.mesh.take() {
        let (cell, translation) = grids
            .get(root.entity())
            .unwrap()
            .translation_to_grid(prepared.anchor);
        let material = materials.add(CanopyMaterial::from(&*canopy));
        let entity = root
            .spawn(
                &mut commands,
                (
                    Name::new("Distant forest canopy"),
                    cell,
                    Transform::from_translation(translation),
                    Mesh3d(meshes.add(prepared.mesh)),
                    MeshMaterial3d(material.clone()),
                    prepared.bounds,
                    visibility_for(&prepared.radii),
                ),
            )
            .id();
        *resident = Some(ResidentMesh {
            entity,
            material,
            crowns: crown_id,
            radii: prepared.radii,
        });
    } else if let Some(current) = resident.as_ref() {
        if canopy.is_changed() {
            if let Some(mut material) = materials.get_mut(&current.material) {
                *material = CanopyMaterial::from(&*canopy);
            }
        }
        if let Ok(mut visible) = visibility.get_mut(current.entity) {
            visible.set_if_neq(visibility_for(&current.radii));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;

    const MIP_START: usize = (crowns::SIDE.pow(2) - CELLS.pow(2)) * 4 / 3 * 4;

    #[test]
    fn sparse_canopy_keeps_gaps_and_tracks_radial_ground_and_crown_height() {
        let mut relief = vec![0; MIP_START + CELLS * CELLS * 4];
        let frame = Frame::new(DVec3::new(1.0, 0.2, -1.0).normalize(), 2_900.0);
        let ground = |up: DVec3| Some(150.0 + 90_000.0 * up.dot(frame.east));
        assert!(PreparedMesh::build(frame, &relief, ground).is_none());

        for (x, y, premultiplied_height) in [(20, 65, 32), (105, 65, 96)] {
            let start = MIP_START + (y * CELLS + x) * 4;
            relief[start..start + 2].copy_from_slice(&[128, premultiplied_height]);
        }
        let prepared = PreparedMesh::build(frame, &relief, ground).unwrap();
        let Some(VertexAttributeValues::Float32x3(positions)) =
            prepared.mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("expected 3D canopy positions");
        };
        let Some(Indices::U16(indices)) = prepared.mesh.indices() else {
            panic!("expected indexed canopy triangles");
        };
        assert_eq!(
            indices.len(),
            2 * 9 * 6,
            "only occupied cells and their border"
        );
        let mut crowns = [false; 2];
        for vertex in positions {
            let position = prepared.anchor + Vec3::from_array(*vertex).as_dvec3();
            let up = position.normalize();
            let square = frame.project(up);
            assert!(square.x.abs() > 0.55, "the central clearing has no mesh");
            let height = position.length() - RADIUS - ground(up).unwrap();
            if height > 0.1 {
                let side = usize::from(square.x > 0.0);
                let expected = [10.0, 30.0][side];
                assert!((height - expected).abs() < 0.001);
                crowns[side] = true;
            }
        }
        assert_eq!(crowns, [true, true]);
        let [a, b, c] = [0, 1, 2].map(|i| Vec3::from_array(positions[indices[i] as usize]));
        assert!((b - a).cross(c - a).dot(frame.up.as_vec3()) > 0.0);
    }
}
