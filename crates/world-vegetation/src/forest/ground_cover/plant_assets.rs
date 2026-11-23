//! CPU geometry for the compact and detailed plant batches.

use std::sync::Arc;

use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState, RenderAssetUsages},
    gltf::{Gltf, GltfLoaderSettings, GltfMesh},
    math::DVec3,
    mesh::VertexAttributeValues,
    prelude::*,
};

use super::ecology::{Family, Site, SPECIES, VARIANTS};
use crate::forest::MeshBuilder;

const DIRECTORY: &str = "models/vegetation/procedural_plants/runtime";
pub(super) const COMPACT_TRIANGLES: usize = 128;
const SOURCE_TRIANGLES: usize = 2400;

#[derive(Default)]
pub(super) struct Library(Vec<[Option<Model>; VARIANTS]>);

impl Library {
    pub(super) fn get(&self, site: Site) -> Option<&Model> {
        self.0.get(site.species)?[site.variant].as_ref()
    }
}

#[derive(Resource)]
pub(super) enum PlantAssets {
    Loading {
        files: [Option<Handle<Gltf>>; Family::ALL.len()],
        library: Library,
    },
    Ready(Arc<Library>),
}

pub(super) struct Model {
    pub(super) compact: Geometry,
    pub(super) detail: Geometry,
}

pub(super) struct Geometry {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl Geometry {
    pub(super) fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub(super) fn append(
        &self,
        target: &mut MeshBuilder,
        site: Site,
        origin: DVec3,
        inverse: Quat,
    ) {
        let base = target.positions.len() as u32;
        let rotation = inverse * site.rotation;
        let root = inverse * (site.position - site.position.normalize() * 0.045 - origin).as_vec3();
        for ((&position, &normal), &color) in
            self.positions.iter().zip(&self.normals).zip(&self.colors)
        {
            let position = Vec3::from_array(position) * site.scale;
            let normal = rotation * (Vec3::from_array(normal) / site.scale);
            target
                .wind
                .push([(position.y * 0.095).clamp(0.0, 0.17), site.phase]);
            target.vertex(root + rotation * position, normal, Vec2::ZERO, color);
        }
        target
            .indices
            .extend(self.indices.iter().map(|index| base + index));
    }
}

pub(super) fn request(mut commands: Commands, server: Res<AssetServer>) {
    let files = Family::ALL.map(|family| {
        Some(
            server
                .load_builder()
                .with_settings(|settings: &mut GltfLoaderSettings| {
                    settings.load_meshes = RenderAssetUsages::MAIN_WORLD;
                    settings.load_materials = RenderAssetUsages::MAIN_WORLD;
                })
                .load(format!("{DIRECTORY}/{}.glb", family.directory())),
        )
    });
    commands.insert_resource(PlantAssets::Loading {
        files,
        library: Library(
            (0..SPECIES.len())
                .map(|_| std::array::from_fn(|_| None))
                .collect(),
        ),
    });
}

pub(super) fn finish(
    server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    meshes: Res<Assets<Mesh>>,
    mut assets: ResMut<PlantAssets>,
) {
    let PlantAssets::Loading { files, library } = &mut *assets else {
        return;
    };
    // Import one pack per update. Once settled, workers share an immutable catalog
    // and can keep the same placements through uploads and detail handoffs.
    for (family, file) in Family::ALL.into_iter().zip(files.iter_mut()) {
        let Some(handle) = file else {
            continue;
        };
        if server.is_loaded_with_dependencies(handle.id()) {
            if let Some(gltf) = gltfs.get(&*handle) {
                for (index, species) in SPECIES
                    .iter()
                    .enumerate()
                    .filter(|(_, species)| species.family == family)
                {
                    for variant in 0..VARIANTS {
                        let stem = format!("{}_{variant}", species.name);
                        let read = |lod, budget| {
                            read_mesh(
                                gltf,
                                &gltf_meshes,
                                &meshes,
                                &format!("{stem}_{lod}"),
                                budget,
                            )
                        };
                        match (
                            read("detail", SOURCE_TRIANGLES),
                            read("compact", COMPACT_TRIANGLES),
                        ) {
                            (Some(detail), Some(compact)) => {
                                library.0[index][variant] = Some(Model { detail, compact })
                            }
                            _ => warn!(
                                "Missing or invalid ground plant mesh {stem} in {}",
                                family.directory()
                            ),
                        }
                    }
                }
            }
        } else if matches!(server.load_state(handle.id()), LoadState::Failed(_))
            || matches!(
                server.get_recursive_dependency_load_state(handle.id()),
                Some(RecursiveDependencyLoadState::Failed(_))
            )
        {
            warn!("Ground plant pack {} failed to load", family.directory());
        } else {
            continue;
        }
        *file = None;
        break;
    }
    if files.iter().all(Option::is_none) {
        *assets = PlantAssets::Ready(Arc::new(std::mem::take(library)));
    }
}

fn read_mesh(
    gltf: &Gltf,
    gltf_meshes: &Assets<GltfMesh>,
    meshes: &Assets<Mesh>,
    name: &str,
    budget: usize,
) -> Option<Geometry> {
    let source = gltf_meshes.get(gltf.named_meshes.get(name)?)?;
    let [primitive] = source.primitives.as_slice() else {
        return None;
    };
    let mesh = meshes.get(&primitive.mesh)?;
    let VertexAttributeValues::Float32x3(positions) = mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
    else {
        return None;
    };
    let VertexAttributeValues::Float32x3(normals) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL)? else {
        return None;
    };
    let VertexAttributeValues::Float32x4(colors) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)? else {
        return None;
    };
    let indices = mesh.indices()?;
    if indices.is_empty()
        || indices.len() % 3 != 0
        || indices.len() / 3 > budget
        || indices.iter().any(|index| index >= positions.len())
        || positions.len() != normals.len()
        || positions.len() != colors.len()
    {
        return None;
    }
    Some(Geometry {
        positions: positions.clone(),
        normals: normals.clone(),
        colors: colors.clone(),
        indices: indices.iter().map(|index| index as u32).collect(),
    })
}
