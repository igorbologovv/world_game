//! Load the standalone tree meshes into the shared forest model and portrait tables.

use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

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
    impostor, library, textures, FoliageMaterial, ForestModels, ModelLod, Species, VariantModels,
    Wind, WoodLod, BROADLEAF_TRANSMISSION,
};

const LODS: usize = 3;
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
    queued: VecDeque<(Prefab, [String; LODS])>,
    pending: Vec<(Prefab, [Handle<Gltf>; LODS])>,
    mipmaps: HashMap<AssetId<Image>, Handle<Image>>,
    portrait_images: HashMap<AssetId<Image>, Arc<Image>>,
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
            let paths: [String; LODS] = std::array::from_fn(|level| {
                format!("models/vegetation/{}_lod{level}.glb", prefab.path)
            });
            paths
                .iter()
                .all(|path| assets.directory().join(path).is_file())
                .then_some((prefab, paths))
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
        imports -= 1;
        let state = &mut *loading;
        if let Some((model, portrait)) = build_variant(
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
            &mut state.portrait_images,
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
    let available = MAX_PENDING_PREFABS - loading.pending.len();
    let state = &mut *loading;
    let requests = available.min(state.queued.len());
    state.pending.extend(
        state
            .queued
            .drain(..requests)
            .map(|(prefab, paths)| (prefab, paths.map(|path| asset_server.load(path)))),
    );
    if !state.is_pending() {
        loading.mipmaps.clear();
        loading.portrait_images.clear();
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
    portrait_images: &mut HashMap<AssetId<Image>, Arc<Image>>,
) -> Option<(VariantModels, impostor::Source)> {
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
    if prefab.species.broadleaf() {
        leaves.diffuse_transmission = BROADLEAF_TRANSMISSION;
    }
    let AlphaMode::Mask(cutoff) = leaves.alpha_mode else {
        return None;
    };
    let bark_image = bark.base_color_texture.as_ref()?;
    let leaf_image = leaves.base_color_texture.as_ref()?;
    let mut shared = |handle: &Handle<Image>| {
        let image = images.get(handle)?;
        Some(Arc::clone(
            portrait_images
                .entry(handle.id())
                .or_insert_with(|| Arc::new(image.clone())),
        ))
    };
    let leaf_normal = leaves
        .normal_map_texture
        .as_ref()
        .filter(|_| leaves.normal_map_channel == bevy::mesh::UvChannel::Uv0)
        .filter(|_| prefab.species.broadleaf())
        .and_then(&mut shared);
    let (wood, foliage_mesh) = geometry.first()?;
    let portrait = impostor::Source::new([
        impostor::Part::from_shared(
            meshes.get(wood)?,
            shared(bark_image)?,
            &bark,
            prefab.repeat_bark,
            None,
        )?,
        impostor::Part::from_shared(
            meshes.get(foliage_mesh)?,
            shared(leaf_image)?,
            &leaves,
            prefab.repeat_leaves,
            leaf_normal,
        )?,
    ]);

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
        Wind::for_crown(portrait.frame())
    } else {
        Wind::default()
    };
    let leaf_material = foliage.add(FoliageMaterial {
        base: leaves,
        extension,
    });

    let mut lods = Vec::with_capacity(LODS);
    for (wood, leaves) in geometry {
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

    fn load_shipped_assets() -> App {
        use crate::forest::hero_oak::{self, HeroOakAssets};
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
        .add_systems(
            Startup,
            (
                super::super::build_models,
                request_assets,
                hero_oak::request_assets,
            ),
        )
        .add_systems(Update, (finish_loading, hero_oak::finish_loading));
        app.finish();
        app.cleanup();

        let timeout = Instant::now() + Duration::from_secs(30);
        loop {
            app.update();
            if !app.world().resource::<TreeAssets>().is_pending()
                && !app.world().resource::<HeroOakAssets>().is_pending()
            {
                break;
            }
            assert!(Instant::now() < timeout, "tree assets timed out");
            std::thread::sleep(Duration::from_millis(5));
        }

        app
    }

    #[test]
    #[ignore = "regenerates the shipped tree impostor pack"]
    fn bake_tree_impostors() {
        let app = load_shipped_assets();
        let world = app.world();
        let assets = world.resource::<WorldAssets>();
        let forest = world.resource::<ForestModels>();
        assert!(forest.portraits.contains(Species::Oak, 0));
        for prefab in PREFABS {
            assert!(forest.portraits.contains(prefab.species, prefab.variant));
        }
        for (index, tree) in library::ASSETS.iter().enumerate() {
            assert!(
                forest
                    .portraits
                    .contains(tree.species, library::variant(index)),
                "{}",
                tree.path
            );
        }
        impostor::bake_to(
            &assets.directory().join(impostor::ATLAS_PATH),
            std::path::Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/forest/tree_coverage.rs"
            )),
            &forest.portraits,
        )
        .unwrap();
    }

    #[test]
    fn shipped_prefabs_load_into_three_shared_lods_and_the_forest_atlas() {
        let app = load_shipped_assets();
        let world = app.world();
        let forest = world.resource::<ForestModels>();
        let assets = world.resource::<WorldAssets>();
        impostor::load(
            &assets.directory().join(impostor::ATLAS_PATH),
            &forest.portraits,
        )
        .expect("the shipped impostors must match the canonical LOD0s");
        let meshes = world.resource::<Assets<Mesh>>();
        let images = world.resource::<Assets<Image>>();
        let materials = world.resource::<Assets<FoliageMaterial>>();
        assert_eq!(library::ASSETS.len(), 60);
        for (index, asset) in library::ASSETS.iter().enumerate() {
            let crown_envelope = std::f32::consts::PI * (asset.width * asset.height).powi(2);
            let zenith_area = library::ZENITH_AREAS[index];
            assert!(
                zenith_area > 0.0 && zenith_area <= crown_envelope,
                "{}",
                asset.path
            );
            let variant = library::variant(index);
            assert!(
                forest.portraits.contains(asset.species, variant),
                "{}",
                asset.path
            );
            let model = &forest.variants[asset.species as usize][variant];
            assert_eq!(model.lods.len(), LODS);
            let mut previous = 6_001;
            for lod in &model.lods {
                assert_eq!(lod.foliage_material, model.lods[0].foliage_material);
                let wood = meshes.get(&lod.wood.as_ref().unwrap().mesh).unwrap();
                let leaves = meshes.get(&lod.foliage).unwrap();
                let triangles =
                    (wood.indices().unwrap().len() + leaves.indices().unwrap().len()) / 3;
                assert!(
                    triangles < previous,
                    "{} has {triangles} triangles",
                    asset.path
                );
                previous = triangles;
            }
        }
        for prefab in PREFABS {
            let variant = &forest.variants[prefab.species as usize][prefab.variant];
            assert_eq!(variant.lods.len(), LODS);
            let foliage = &variant.lods[0].foliage_material;
            let mut previous = 6_000;
            for (level, lod) in variant.lods.iter().enumerate() {
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
                if level != 0 {
                    let (mean, worst) = forest.portraits.silhouette_score(
                        prefab.species,
                        prefab.variant,
                        [wood, leaves],
                    );
                    println!(
                        "{} LOD{level}: silhouette IoU {mean:.3} mean, {worst:.3} minimum",
                        prefab.path
                    );
                    assert!(
                        mean >= 0.88 && worst >= 0.80,
                        "{} LOD{level}: silhouette overlap {mean:.3} / {worst:.3}",
                        prefab.path
                    );
                }
                previous = triangles;
            }
            let leaves = &materials.get(foliage).unwrap().base;
            let leaf_image = images
                .get(leaves.base_color_texture.as_ref().unwrap())
                .unwrap();
            assert!(leaf_image.texture_descriptor.mip_level_count > 1);
            assert!(forest.portraits.contains(prefab.species, prefab.variant));
            if prefab.species == Species::Oak {
                assert!(
                    forest
                        .portraits
                        .has_leaf_normal_map(prefab.species, prefab.variant),
                    "{} must carry its leaf normal map into the impostor bake",
                    prefab.path
                );
            }
        }
    }
}
