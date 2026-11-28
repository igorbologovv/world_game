//! Load the standalone tree meshes into the forest LOD0 source table.

use std::collections::{HashMap, VecDeque};

use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState},
    camera::primitives::MeshAabb,
    gltf::{Gltf, GltfMaterial, GltfMesh},
    material::AlphaMode,
    mesh::VertexAttributeValues,
    pbr::gltf::standard_material_from_gltf_material,
    prelude::*,
};
use world_core::runtime::WorldAssets;

use super::{
    library, textures, FoliageMaterial, ForestModels, ModelLod, Species, VariantModels, Wind,
    WoodLod, BROADLEAF_TRANSMISSION,
};

const MAX_PENDING_PREFABS: usize = 8;

#[derive(Clone, Copy)]
struct Prefab {
    path: &'static str,
    species: Species,
    variant: usize,
    repeat_leaves: bool,
    repeat_bark: bool,
}

const PREFABS: [Prefab; 4] = [
    Prefab {
        path: "coconut_palm/tree",
        species: Species::Palm,
        variant: 0,
        repeat_leaves: true,
        repeat_bark: false,
    },
    Prefab {
        path: "tree_part9/tree",
        species: Species::Jungle,
        variant: 1,
        repeat_leaves: false,
        repeat_bark: true,
    },
    Prefab {
        path: "woodland/woodland_tree_00",
        species: Species::Oak,
        variant: 1,
        repeat_leaves: true,
        repeat_bark: true,
    },
    Prefab {
        path: "woodland/woodland_tree_01",
        species: Species::Oak,
        variant: 2,
        repeat_leaves: true,
        repeat_bark: true,
    },
];

#[derive(Resource, Default)]
pub(super) struct TreeAssets {
    queued: VecDeque<(Prefab, String)>,
    pending: Vec<(Prefab, Handle<Gltf>)>,
    mipmaps: HashMap<AssetId<Image>, Handle<Image>>,
}

impl TreeAssets {
    pub fn is_pending(&self) -> bool {
        !self.queued.is_empty() || !self.pending.is_empty()
    }
}

pub(super) fn request_assets(mut commands: Commands, assets: Res<WorldAssets>) {
    let queued = PREFABS
        .into_iter()
        .chain(library::ASSETS.iter().enumerate().map(|(i, asset)| Prefab {
            path: asset.path,
            species: asset.species,
            variant: library::variant(i),
            repeat_leaves: false,
            repeat_bark: true,
        }))
        .filter_map(|prefab| {
            let path = format!("models/vegetation/{}_lod0.glb", prefab.path);
            assets
                .directory()
                .join(&path)
                .is_file()
                .then_some((prefab, path))
        })
        .collect();
    commands.insert_resource(TreeAssets {
        queued,
        ..default()
    });
}

pub(super) fn finish_loading(
    asset_server: Res<AssetServer>,
    mut loading: ResMut<TreeAssets>,
    mut models: ResMut<ForestModels>,
    gltfs: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    gltf_materials: Res<Assets<GltfMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut foliage: ResMut<Assets<FoliageMaterial>>,
) {
    let mut index = 0;
    let mut imports = 4;
    while index < loading.pending.len() && imports > 0 {
        let (prefab, file) = &loading.pending[index];
        if matches!(asset_server.load_state(file.id()), LoadState::Failed(_))
            || matches!(
                asset_server.get_recursive_dependency_load_state(file.id()),
                Some(RecursiveDependencyLoadState::Failed(_))
            )
        {
            warn!("Tree prefab {} failed to load", prefab.path);
            loading.pending.swap_remove(index);
            continue;
        }
        if !asset_server.is_loaded_with_dependencies(file.id()) {
            index += 1;
            continue;
        }

        let (prefab, files) = loading.pending.swap_remove(index);
        imports -= 1;
        let state = &mut *loading;
        if let Some(model) = build_variant(
            &prefab,
            &files,
            &gltfs,
            &gltf_meshes,
            &gltf_materials,
            &mut meshes,
            &mut images,
            &mut materials,
            &mut foliage,
            &mut state.mipmaps,
        ) {
            models.variants[prefab.species as usize][prefab.variant] = model;
            info!("Loaded tree prefab {}", prefab.path);
        } else {
            warn!(
                "Tree prefab {} has incompatible geometry or materials",
                prefab.path
            );
        }
    }
    let available = MAX_PENDING_PREFABS - loading.pending.len();
    let state = &mut *loading;
    let requests = available.min(state.queued.len());
    state.pending.extend(
        state
            .queued
            .drain(..requests)
            .map(|(prefab, paths)| (prefab, asset_server.load(paths))),
    );
    if !state.is_pending() {
        loading.mipmaps.clear();
    }
}

fn build_variant(
    prefab: &Prefab,
    files: &Handle<Gltf>,
    gltfs: &Assets<Gltf>,
    gltf_meshes: &Assets<GltfMesh>,
    gltf_materials: &Assets<GltfMaterial>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    foliage: &mut Assets<FoliageMaterial>,
    mipmaps: &mut HashMap<AssetId<Image>, Handle<Image>>,
) -> Option<VariantModels> {
    let parts = |file, name: &str| {
        let gltf = gltfs.get(file)?;
        let material = gltf.named_materials.get(name)?;
        let primitive = gltf
            .meshes
            .iter()
            .filter_map(|handle| gltf_meshes.get(handle))
            .flat_map(|mesh| &mesh.primitives)
            .find(|primitive| primitive.material.as_ref() == Some(material))?;
        Some((primitive.mesh.clone(), gltf_materials.get(material)?))
    };
    let (wood, bark_source) = parts(files, "Bark")?;
    let (leaves_mesh, leaves_source) = parts(files, "Leaves")?;
    let mut bark = standard_material_from_gltf_material(bark_source);
    let mut leaves = standard_material_from_gltf_material(leaves_source);
    if prefab.species.broadleaf() {
        leaves.diffuse_transmission = BROADLEAF_TRANSMISSION;
    }
    let AlphaMode::Mask(cutoff) = leaves.alpha_mode else {
        return None;
    };
    let bark_image = bark.base_color_texture.as_ref()?;
    let leaf_image = leaves.base_color_texture.as_ref()?;
    bark.base_color_texture = Some(
        mipmaps
            .entry(bark_image.id())
            .or_insert_with(|| {
                textures::import_mipmapped(images, bark_image, None, prefab.repeat_bark)
            })
            .clone(),
    );
    leaves.base_color_texture = Some(
        mipmaps
            .entry(leaf_image.id())
            .or_insert_with(|| {
                textures::import_mipmapped(images, leaf_image, Some(cutoff), prefab.repeat_leaves)
            })
            .clone(),
    );
    let bark = materials.add(bark);
    let extension = if prefab.species.broadleaf() {
        Wind::for_crown(meshes.get(&leaves_mesh)?.get_aabb()?)
    } else {
        Wind::default()
    };
    let leaf_material = foliage.add(FoliageMaterial {
        base: leaves,
        extension,
    });

    let lod = {
        let leaves = leaves_mesh;
        let bounds = meshes.get(&wood)?.get_aabb()?;
        let wood = WoodLod { mesh: wood, bounds };
        let mut mesh = meshes.get_mut(&leaves)?;
        let bounds = mesh.get_aabb()?;
        let VertexAttributeValues::Float32x3(positions) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)?
        else {
            return None;
        };
        let weights: Vec<_> = positions
            .iter()
            .map(|&[x, y, z]| {
                let weight = ((y - 1.5) * 0.025).clamp(0.0, 0.18);
                [weight, x * 0.21 + z * 0.17 + y * 0.09]
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; weights.len()]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, weights);
        ModelLod::from_parts(Some(wood), leaves, leaf_material.clone(), bounds)
    };
    Some(VariantModels {
        lods: vec![lod],
        bark: Some(bark),
    })
}
