//! Load the standalone tree meshes into the shared forest model and portrait tables.

use std::collections::{hash_map::Entry, HashMap};

use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState},
    camera::primitives::MeshAabb,
    gltf::{Gltf, GltfMaterial, GltfMesh},
    material::AlphaMode,
    mesh::VertexAttributeValues,
    pbr::gltf::standard_material_from_gltf_material,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
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
    processing: Option<(&'static Prefab, Task<Option<PreparedPrefab>>)>,
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
    if let Some((prefab, task)) = &mut loading.processing {
        let Some(ready) = block_on(poll_once(task)) else {
            return;
        };
        let prefab = *prefab;
        loading.processing = None;
        if let Some(mut ready) = ready {
            for (source, image) in ready.mipmaps.drain(..) {
                loading.mipmaps.insert(source, images.add(image));
            }
            apply_texture_handles(&mut ready.bark, &loading.mipmaps);
            apply_texture_handles(&mut ready.leaves, &loading.mipmaps);
            if let Some(model) = install_variant(
                ready.geometry,
                ready.bark,
                ready.leaves,
                &mut meshes,
                &mut materials,
                &mut foliage,
            ) {
                models.variants[prefab.species as usize][prefab.variant] = model;
                models
                    .portraits
                    .insert(prefab.species, prefab.variant, ready.portrait);
                info!("Loaded tree prefab {}", prefab.path);
            } else {
                warn!("Tree prefab {} has incompatible geometry", prefab.path);
            }
        } else {
            warn!(
                "Tree prefab {} has incompatible portrait textures",
                prefab.path
            );
        }
    }

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
        if let Some(task) = prepare_variant(
            prefab,
            &files,
            &gltfs,
            &gltf_meshes,
            &gltf_materials,
            &meshes,
            &images,
            &loading.mipmaps,
        ) {
            // One worker owns texture snapshots at a time. A shared source
            // mip chain (e.g. woodland bark) is reused after installation.
            loading.processing = Some((prefab, task));
            return;
        }
        warn!(
            "Tree prefab {} has incompatible geometry or materials",
            prefab.path
        );
    }
    if loading.pending.is_empty() {
        loading.mipmaps.clear();
    }
}

type Geometry = Vec<(Handle<Mesh>, Handle<Mesh>)>;

struct PreparedPrefab {
    geometry: Geometry,
    bark: StandardMaterial,
    leaves: StandardMaterial,
    portrait: Vec<[u8; 4]>,
    mipmaps: Vec<(AssetId<Image>, Image)>,
}

fn prepare_variant(
    prefab: &'static Prefab,
    files: &[Handle<Gltf>; LODS],
    gltfs: &Assets<Gltf>,
    gltf_meshes: &Assets<GltfMesh>,
    gltf_materials: &Assets<GltfMaterial>,
    meshes: &Assets<Mesh>,
    images: &Assets<Image>,
    mipmaps: &HashMap<AssetId<Image>, Handle<Image>>,
) -> Option<Task<Option<PreparedPrefab>>> {
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

    let bark = standard_material_from_gltf_material(parts(&files[0], "Bark")?.1);
    let leaves = standard_material_from_gltf_material(parts(&files[0], "Leaves")?.1);
    let AlphaMode::Mask(cutoff) = leaves.alpha_mode else {
        return None;
    };
    let (far_wood, far_leaves) = geometry.last()?;
    let (wood, canopy) = (
        meshes.get(far_wood)?.clone(),
        meshes.get(far_leaves)?.clone(),
    );
    let bark_id = bark.base_color_texture.as_ref()?.id();
    let leaf_id = leaves.base_color_texture.as_ref()?.id();
    let mut inputs = HashMap::new();
    for (material, cutoff, repeat) in [
        (&bark, None, prefab.repeat_bark),
        (&leaves, Some(cutoff), prefab.repeat_leaves),
    ] {
        for (handle, cutoff) in [
            (&material.base_color_texture, cutoff),
            (&material.normal_map_texture, None),
            (&material.metallic_roughness_texture, None),
        ] {
            if let Some(handle) = handle {
                let id = handle.id();
                let needs_mipmaps = !mipmaps.contains_key(&id);
                if needs_mipmaps || id == bark_id || id == leaf_id {
                    if let Entry::Vacant(entry) = inputs.entry(id) {
                        entry.insert((images.get(handle)?.clone(), cutoff, repeat, needs_mipmaps));
                    }
                }
            }
        }
    }
    Some(AsyncComputeTaskPool::get().spawn(async move {
        let portrait = impostor::render_authored(
            prefab.species,
            prefab.variant,
            [
                (&wood, &inputs.get(&bark_id)?.0, 0.0, prefab.repeat_bark),
                (
                    &canopy,
                    &inputs.get(&leaf_id)?.0,
                    cutoff,
                    prefab.repeat_leaves,
                ),
            ],
        )?;
        let mipmaps = inputs
            .into_iter()
            .filter_map(|(id, (image, cutoff, repeat, needed))| {
                if needed {
                    textures::prepare_mipmaps(&image, cutoff, repeat).map(|mips| (id, mips))
                } else {
                    None
                }
            })
            .collect();
        Some(PreparedPrefab {
            geometry,
            bark,
            leaves,
            portrait,
            mipmaps,
        })
    }))
}

fn apply_texture_handles(
    material: &mut StandardMaterial,
    mipmaps: &HashMap<AssetId<Image>, Handle<Image>>,
) {
    for handle in [
        &mut material.base_color_texture,
        &mut material.normal_map_texture,
        &mut material.metallic_roughness_texture,
    ]
    .into_iter()
    .flatten()
    {
        if let Some(mipmapped) = mipmaps.get(&handle.id()) {
            *handle = mipmapped.clone();
        }
    }
}

fn install_variant(
    geometry: Geometry,
    bark: StandardMaterial,
    leaves: StandardMaterial,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    foliage: &mut Assets<FoliageMaterial>,
) -> Option<VariantModels> {
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
    Some(VariantModels {
        lods,
        bark: Some(bark),
    })
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
            let loading = app.world().resource::<TreeAssets>();
            if loading.pending.is_empty() && loading.processing.is_none() {
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
