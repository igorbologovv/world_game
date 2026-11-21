//! Load the standalone tree meshes into the shared forest model and portrait tables.

use std::collections::HashMap;

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
    impostor, textures, FoliageMaterial, ForestModels, ModelLod, Species, VariantModels, Wind,
    WoodLod,
};

const LODS: usize = 3;

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
    pending: Vec<(&'static Prefab, [Handle<Gltf>; LODS])>,
    mipmaps: HashMap<AssetId<Image>, Handle<Image>>,
}

pub(super) fn request_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    assets: Res<WorldAssets>,
) {
    let pending = PREFABS
        .iter()
        .filter_map(|prefab| {
            let paths: [String; LODS] = std::array::from_fn(|level| {
                format!("models/vegetation/{}_lod{level}.glb", prefab.path)
            });
            paths
                .iter()
                .all(|path| assets.directory().join(path).is_file())
                .then(|| (prefab, paths.map(|path| asset_server.load(path))))
        })
        .collect();
    commands.insert_resource(TreeAssets {
        pending,
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
    while index < loading.pending.len() {
        let (prefab, files) = &loading.pending[index];
        if files.iter().any(|file| {
            matches!(asset_server.load_state(file.id()), LoadState::Failed(_))
                || matches!(
                    asset_server.get_recursive_dependency_load_state(file.id()),
                    Some(RecursiveDependencyLoadState::Failed(_))
                )
        }) {
            warn!("Tree prefab {} failed to load", prefab.path);
            loading.pending.swap_remove(index);
            continue;
        }
        if !files
            .iter()
            .all(|file| asset_server.is_loaded_with_dependencies(file.id()))
        {
            index += 1;
            continue;
        }

        let (prefab, files) = loading.pending.swap_remove(index);
        if let Some((model, portrait)) = build_variant(
            prefab,
            &files,
            &gltfs,
            &gltf_meshes,
            &gltf_materials,
            &mut meshes,
            &mut images,
            &mut materials,
            &mut foliage,
            &mut loading.mipmaps,
        ) {
            models.variants[prefab.species as usize][prefab.variant] = model;
            models
                .portraits
                .insert(prefab.species, prefab.variant, portrait);
            info!("Loaded tree prefab {}", prefab.path);
        } else {
            warn!(
                "Tree prefab {} has incompatible geometry or materials",
                prefab.path
            );
        }
    }
    if loading.pending.is_empty() {
        loading.mipmaps.clear();
    }
}

fn build_variant(
    prefab: &Prefab,
    files: &[Handle<Gltf>; LODS],
    gltfs: &Assets<Gltf>,
    gltf_meshes: &Assets<GltfMesh>,
    gltf_materials: &Assets<GltfMaterial>,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    foliage: &mut Assets<FoliageMaterial>,
    mipmaps: &mut HashMap<AssetId<Image>, Handle<Image>>,
) -> Option<(VariantModels, Vec<[u8; 4]>)> {
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
    let mut geometry = Vec::with_capacity(LODS);
    for file in files {
        let (wood, _) = parts(file, "Bark")?;
        let (leaves, _) = parts(file, "Leaves")?;
        geometry.push((wood, leaves));
    }

    let mut bark = standard_material_from_gltf_material(parts(&files[0], "Bark")?.1);
    let mut leaves = standard_material_from_gltf_material(parts(&files[0], "Leaves")?.1);
    let AlphaMode::Mask(cutoff) = leaves.alpha_mode else {
        return None;
    };
    let bark_image = bark.base_color_texture.as_ref()?;
    let leaf_image = leaves.base_color_texture.as_ref()?;
    let (far_wood, far_leaves) = geometry.last()?;
    let portrait = impostor::render_authored(
        prefab.species,
        prefab.variant,
        [
            (
                meshes.get(far_wood)?,
                images.get(bark_image)?,
                0.0,
                prefab.repeat_bark,
            ),
            (
                meshes.get(far_leaves)?,
                images.get(leaf_image)?,
                cutoff,
                prefab.repeat_leaves,
            ),
        ],
    )?;

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
    let leaf_material = foliage.add(FoliageMaterial {
        base: leaves,
        extension: Wind::default(),
    });

    let mut lods = Vec::with_capacity(LODS);
    for (level, (wood, leaves)) in geometry.into_iter().enumerate() {
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
                let weight = if level < 2 {
                    ((y - 1.5) * 0.025).clamp(0.0, 0.18)
                } else {
                    0.0
                };
                [weight, x * 0.21 + z * 0.17 + y * 0.09]
            })
            .collect();
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; weights.len()]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, weights);
        lods.push(ModelLod::from_parts(
            Some(wood),
            leaves,
            leaf_material.clone(),
            bounds,
        ));
    }
    Some((
        VariantModels {
            lods,
            bark: Some(bark),
        },
        portrait,
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        time::{Duration, Instant},
    };

    use bevy::{
        asset::AssetPlugin,
        gltf::GltfPlugin,
        image::{CompressedImageFormats, ImageLoader, ImagePlugin},
        scene::ScenePlugin,
        world_serialization::WorldSerializationPlugin,
    };

    use super::*;

    #[test]
    fn shipped_prefabs_load_into_three_shared_lods_and_the_forest_atlas() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: path.into(),
                ..default()
            },
            ImagePlugin::default(),
            WorldSerializationPlugin,
            ScenePlugin,
            GltfPlugin::default(),
        ))
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<FoliageMaterial>()
        .insert_resource(WorldAssets::new(Path::new(path).to_path_buf()))
        .add_systems(Startup, (super::super::build_models, request_assets))
        .add_systems(Update, finish_loading);
        app.finish();
        app.cleanup();

        let timeout = Instant::now() + Duration::from_secs(10);
        loop {
            app.update();
            if app.world().resource::<TreeAssets>().pending.is_empty() {
                break;
            }
            assert!(Instant::now() < timeout, "tree assets timed out");
            std::thread::sleep(Duration::from_millis(5));
        }

        let world = app.world();
        let forest = world.resource::<ForestModels>();
        let meshes = world.resource::<Assets<Mesh>>();
        let images = world.resource::<Assets<Image>>();
        let materials = world.resource::<Assets<FoliageMaterial>>();
        assert_eq!(forest.portraits.revision, PREFABS.len());
        let atlas = impostor::bake(None, &forest.portraits);
        let fallback = impostor::bake(None, &impostor::Portraits::default());
        for prefab in PREFABS {
            let variant = &forest.variants[prefab.species as usize][prefab.variant];
            assert_eq!(variant.lods.len(), LODS);
            let foliage = &variant.lods[0].foliage_material;
            let mut previous = 6_000;
            for lod in &variant.lods {
                assert_eq!(&lod.foliage_material, foliage);
                let wood = meshes.get(&lod.wood.as_ref().unwrap().mesh).unwrap();
                let leaves = meshes.get(&lod.foliage).unwrap();
                let triangles =
                    (wood.indices().unwrap().len() + leaves.indices().unwrap().len()) / 3;
                assert!(
                    triangles < previous,
                    "{} LOD has {triangles} triangles",
                    prefab.path
                );
                previous = triangles;
            }
            let leaves = &materials.get(foliage).unwrap().base;
            let leaf_image = images
                .get(leaves.base_color_texture.as_ref().unwrap())
                .unwrap();
            assert!(leaf_image.texture_descriptor.mip_level_count > 1);
            let width = atlas.texture_descriptor.size.width as usize;
            let tile_x = (impostor::uv(prefab.species, prefab.variant, Vec2::ZERO).x * width as f32)
                as usize;
            let side = atlas.texture_descriptor.size.height as usize;
            let tile_x = tile_x - tile_x % side;
            let bytes = |image: &Image| {
                image
                    .data
                    .as_ref()
                    .unwrap()
                    .chunks_exact(width * 4)
                    .take(side)
                    .flat_map(|row| row[tile_x * 4..(tile_x + side) * 4].iter().copied())
                    .collect::<Vec<_>>()
            };
            let portrait = bytes(&atlas);
            assert_ne!(portrait, bytes(&fallback));
            assert!(
                portrait
                    .chunks_exact(4)
                    .filter(|pixel| pixel[3] > 120)
                    .count()
                    > 100,
                "{} portrait is missing its cutout",
                prefab.path
            );
        }
    }
}
