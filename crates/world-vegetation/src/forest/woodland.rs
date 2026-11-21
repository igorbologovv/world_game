//! Optional textured woodland tree variants extracted from the source glTF.
//!
//! The source scene is never spawned. Once the standalone prefabs have been
//! exported with tools/extract_woodland_prefabs.py, they replace two existing
//! oak variants at their deterministic world sites. The canonical placement,
//! streaming, and existing distant-forest representation remain unchanged.

use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState},
    gltf::{Gltf, GltfMesh, GltfMaterial},
    pbr::gltf::standard_material_from_gltf_material,
    render::alpha::AlphaMode,
    prelude::*,
    mesh::VertexAttributeValues,
};

use world_core::runtime::WorldAssets;

use super::{
    textures, FoliageMaterial, ForestModels, ModelLod, Species, VariantModels, Wind, WoodLod,
};

const PREFIX: &str = "models/vegetation/woodland";
const ALPHA_CUTOFF: f32 = 0.42;
const VARIANTS: usize = 2; // Preserve the existing Hero Oak at variant zero.
const LODS: usize = 3;

#[derive(Resource, Default)]
pub(super) struct WoodlandAssets {
    pending: Vec<(usize, [Handle<Gltf>; LODS])>,
}

pub(super) fn request_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    assets: Res<WorldAssets>,
) {
    let mut pending = Vec::new();
    for idx in 0..VARIANTS {
        let directory = format!("{PREFIX}/woodland_tree_{idx:02}");
        let exists = (0..LODS).all(|lod| {
            assets.directory()
                .join(format!("{directory}/tree_lod{lod}.glb"))
                .is_file()
        });
        if !exists {
            continue;
        }
        let lods = std::array::from_fn(|lod| {
            asset_server.load(format!("{directory}/tree_lod{lod}.glb"))
        });
        pending.push((idx + 1, lods));
    }
    if pending.is_empty() {
        info!("No standalone woodland tree prefabs found; procedural oak variants remain enabled");
    }
    commands.insert_resource(WoodlandAssets { pending });
}

pub(super) fn finish_loading(
    asset_server: Res<AssetServer>,
    mut loading: ResMut<WoodlandAssets>,
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
    while index < loading.pending.len() {
        let (variant, lods) = &loading.pending[index];
        let failed = lods.iter().any(|file| {
            matches!(asset_server.load_state(file.id()), LoadState::Failed(_))
                || matches!(
                    asset_server.get_recursive_dependency_load_state(file.id()),
                    Some(RecursiveDependencyLoadState::Failed(_))
                )
        });
        if failed {
            warn!("Woodland tree variant {} could not load; keeping procedural tree", variant);
            loading.pending.swap_remove(index);
            continue;
        }
        if !lods.iter().all(|file| asset_server.is_loaded_with_dependencies(file.id())) {
            index += 1;
            continue;
        }

        let converted = build_variant(lods, &gltfs, &gltf_meshes, &gltf_materials, &mut meshes,
                                      &mut images, &mut materials, &mut foliage);
        if let Some(variant_models) = converted {
            models.variants[Species::Oak as usize][*variant] = variant_models;
            info!("Enabled standalone woodland tree for oak variant {}", variant);
        } else {
            warn!("Woodland tree variant {} has incompatible meshes/materials", variant);
        }
        loading.pending.swap_remove(index);
    }
}

fn build_variant(
    files: &[Handle<Gltf>; LODS],
    gltfs: &Assets<Gltf>,
    gltf_meshes: &Assets<GltfMesh>,
    gltf_materials: &Assets<GltfMaterial>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    foliage: &mut Assets<FoliageMaterial>,
) -> Option<VariantModels> {
    let mut levels = Vec::with_capacity(LODS);
    let mut bark_material = None;

    for (level, file) in files.iter().enumerate() {
        let gltf = gltfs.get(file)?;
        let find_part = |name: &str| {
            let material = gltf.named_materials.get(name)?;
            let primitive = gltf.meshes.iter()
                .filter_map(|handle| gltf_meshes.get(handle))
                .flat_map(|mesh| &mesh.primitives)
                .find(|primitive| primitive.material.as_ref() == Some(material))?;
            Some((primitive.mesh.clone(), material.clone()))
        };

        let (wood, source_bark) = find_part("Bark")?;
        let (leaves, source_leaves) = find_part("Leaves")?;
        if level == 0 {
            let original_bark = standard_material_from_gltf_material(gltf_materials.get(&source_bark)?);
            bark_material = Some(materials.add(original_bark));
        }
        let source_leaf_material = standard_material_from_gltf_material(gltf_materials.get(&source_leaves)?);
        let source_leaf_image = source_leaf_material.base_color_texture.as_ref()?;
        let image = textures::import_mipmapped(images, source_leaf_image, Some(ALPHA_CUTOFF), false);
        let leaf_material = foliage.add(FoliageMaterial {
            base: StandardMaterial {
                base_color_texture: Some(image),
                alpha_mode: AlphaMode::Mask(ALPHA_CUTOFF),
                double_sided: true,
                cull_mode: None,
                metallic: 0.0,
                perceptual_roughness: 0.87,
                ..default()
            },
            extension: Wind::default(),
        });

        // The foliage shader expects wind weights in UV1, encoded once per
        // shared mesh. Per-tree transforms remain owned by the forest streamer.
        let mesh = meshes.get_mut(&leaves)?;
        let bounds = mesh.get_aabb()?;
        let Some(VertexAttributeValues::Float32x3(points)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            return None;
        };
        let weights: Vec<[f32; 2]> = points.iter()
            .map(|p| [((p[1] - 1.5) * 0.025).clamp(0.0, 0.18),
                       p[0] * 0.21 + p[2] * 0.17 + p[1] * 0.09])
            .collect();
        let count = weights.len();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; count]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, weights);
        let wood = meshes.get(&wood).and_then(|mesh| mesh.get_aabb().map(|bounds| (wood, bounds)));
        let wood = wood.map(|(mesh, bounds)| WoodLod { mesh, bounds });
        levels.push(ModelLod::from_parts(wood, leaves, leaf_material, bounds));
    }
    Some(VariantModels { lods: levels, bark: bark_material })
}
