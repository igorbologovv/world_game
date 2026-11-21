use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState},
    gltf::{Gltf, GltfMesh},
    prelude::*,
    render::{mesh::VertexAttributeValues, render_resource::Face},
};

use super::{textures, FoliageMaterial, ForestModels, ModelLod, Wind, WoodLod};

pub(super) const ROOT_SINK: f64 = 0.16;
const ALPHA_CUTOFF: f32 = 0.46;

pub(super) struct HeroOak {
    pub lods: [ModelLod; 4],
    pub bark: Handle<StandardMaterial>,
    pub impostor_image: Handle<Image>,
}

#[derive(Resource)]
pub(super) struct HeroOakAssets(Option<[Handle<Gltf>; 4]>);

impl HeroOakAssets {
    pub fn is_pending(&self) -> bool {
        self.0.is_some()
    }
}

struct OakPrimitives {
    leaves: [Handle<Mesh>; 4],
    wood: [Handle<Mesh>; 3],
    bark_image: Handle<Image>,
    leaf_image: Handle<Image>,
    impostor_image: Handle<Image>,
}

impl OakPrimitives {
    fn resolve(
        files: &[Handle<Gltf>; 4],
        gltfs: &Assets<Gltf>,
        gltf_meshes: &Assets<GltfMesh>,
        materials: &Assets<StandardMaterial>,
    ) -> Option<Self> {
        let mut leaves = Vec::with_capacity(4);
        let mut wood = Vec::with_capacity(3);
        let mut bark_image = None;
        let mut leaf_image = None;
        let mut impostor_image = None;
        for (level, file) in files.iter().enumerate() {
            let gltf = gltfs.get(file)?;
            let part = |name: &str| -> Option<(Handle<Mesh>, Handle<Image>)> {
                // Use the exported material names. Blender and the standalone
                // exporter can assign different mesh and texture indices.
                let material = gltf.named_materials.get(name)?;
                let primitive = gltf
                    .meshes
                    .iter()
                    .filter_map(|handle| gltf_meshes.get(handle))
                    .flat_map(|mesh| &mesh.primitives)
                    .find(|primitive| primitive.material.as_ref() == Some(material))?;
                let image = materials.get(material)?.base_color_texture.clone()?;
                Some((primitive.mesh.clone(), image))
            };
            if level == 3 {
                let (mesh, image) = part("Oak_Distant_Silhouette")?;
                leaves.push(mesh);
                impostor_image = Some(image);
            } else {
                let (mesh, image) = part("Sunlit_Oak_Foliage")?;
                leaves.push(mesh);
                let (trunk, bark) = part("Aged_Oak_Bark")?;
                wood.push(trunk);
                if level == 0 {
                    bark_image = Some(bark);
                    leaf_image = Some(image);
                }
            }
        }
        Some(Self {
            leaves: leaves.try_into().ok()?,
            wood: wood.try_into().ok()?,
            bark_image: bark_image?,
            leaf_image: leaf_image?,
            impostor_image: impostor_image?,
        })
    }

    fn build(
        &self,
        meshes: &mut Assets<Mesh>,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
        foliage_materials: &mut Assets<FoliageMaterial>,
    ) -> Option<HeroOak> {
        for handle in self.leaves.iter().chain(self.wood.iter()) {
            meshes.get(handle)?;
        }
        for handle in [&self.bark_image, &self.leaf_image, &self.impostor_image] {
            images.get(handle)?;
        }

        let bark = materials.add(StandardMaterial {
            base_color_texture: Some(textures::import_mipmapped(
                images,
                &self.bark_image,
                None,
                true,
            )),
            perceptual_roughness: 0.98,
            double_sided: true,
            cull_mode: None,
            ..default()
        });
        let canopy = foliage_materials.add(foliage(
            textures::import_mipmapped(images, &self.leaf_image, Some(ALPHA_CUTOFF), false),
            false,
        ));
        let impostor = foliage_materials.add(foliage(
            textures::import_mipmapped(images, &self.impostor_image, Some(ALPHA_CUTOFF), false),
            true,
        ));
        let mut lods = Vec::with_capacity(4);
        for (level, leaves) in self.leaves.iter().enumerate() {
            let mesh = meshes.get_mut(leaves)?;
            let foliage_bounds = mesh.compute_aabb()?;
            // The existing foliage shader uses a color and a second UV set.
            // Prepare the imported mesh once so every oak can share it.
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                return None;
            };
            let wind: Vec<_> = positions
                .iter()
                .map(|&[x, y, z]| {
                    let weight = if level < 2 {
                        ((y - 3.0) * 0.008).clamp(0.0, 0.20)
                    } else {
                        0.0
                    };
                    [weight, x * 0.21 + z * 0.17 + y * 0.09]
                })
                .collect();
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0; 4]; wind.len()]);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, wind);

            let wood = self.wood.get(level).and_then(|handle| {
                Some(WoodLod {
                    mesh: handle.clone(),
                    bounds: meshes.get(handle)?.compute_aabb()?,
                })
            });
            let material = if level == 3 {
                impostor.clone()
            } else {
                canopy.clone()
            };
            lods.push(ModelLod::from_parts(
                wood,
                leaves.clone(),
                material,
                foliage_bounds,
            ));
        }
        Some(HeroOak {
            lods: lods.try_into().ok()?,
            bark,
            impostor_image: self.impostor_image.clone(),
        })
    }
}

fn foliage(texture: Handle<Image>, impostor: bool) -> FoliageMaterial {
    FoliageMaterial {
        base: StandardMaterial {
            base_color_texture: Some(texture),
            alpha_mode: AlphaMode::Mask(ALPHA_CUTOFF),
            perceptual_roughness: if impostor { 0.9 } else { 0.79 },
            reflectance: 0.16,
            double_sided: !impostor,
            // The impostor already contains both windings for each card.
            cull_mode: impostor.then_some(Face::Back),
            ..default()
        },
        extension: Wind::default(),
    }
}

pub(super) fn request_assets(mut commands: Commands, assets: Res<AssetServer>) {
    let files = std::array::from_fn(|lod| assets.load(format!("models/hero_oak/oak_lod{lod}.glb")));
    commands.insert_resource(HeroOakAssets(Some(files)));
}

pub(super) fn finish_loading(
    assets: Res<AssetServer>,
    mut pending: ResMut<HeroOakAssets>,
    mut models: ResMut<ForestModels>,
    gltfs: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut foliage_materials: ResMut<Assets<FoliageMaterial>>,
) {
    let Some(files) = pending.0.as_ref() else {
        return;
    };
    if files.iter().any(|file| {
        matches!(assets.load_state(file.id()), LoadState::Failed(_))
            || matches!(
                assets.get_recursive_dependency_load_state(file.id()),
                Some(RecursiveDependencyLoadState::Failed)
            )
    }) {
        warn!("Hero Oak failed to load. Keeping the procedural oak meshes.");
        pending.0 = None;
        return;
    }
    if files
        .iter()
        .all(|file| assets.is_loaded_with_dependencies(file.id()))
    {
        models.hero_oak =
            OakPrimitives::resolve(files, &gltfs, &gltf_meshes, &materials).and_then(|parts| {
                parts.build(
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    &mut foliage_materials,
                )
            });
        if models.hero_oak.is_none() {
            warn!(
                "Hero Oak meshes or textures are unavailable. Keeping the procedural oak meshes."
            );
        }
        pending.0 = None;
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bevy::{
        asset::AssetPlugin, gltf::GltfPlugin, render::texture::ImagePlugin, scene::ScenePlugin,
    };

    use super::*;

    #[test]
    fn shipped_oak_assets_load_with_the_exported_lods_and_shared_cutout_materials() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                ..default()
            },
            ScenePlugin,
            ImagePlugin::default(),
            GltfPlugin::default(),
        ))
        .init_asset::<Mesh>()
        .init_asset::<StandardMaterial>()
        .init_asset::<FoliageMaterial>()
        .insert_resource(ForestModels {
            variants: Vec::new(),
            bark_material: Handle::default(),
            hero_oak: None,
        })
        .add_systems(Startup, request_assets)
        .add_systems(Update, finish_loading);
        app.finish();
        app.cleanup();

        let start = Instant::now();
        loop {
            app.update();
            let world = app.world();
            if world.resource::<HeroOakAssets>().0.is_none() {
                let oak =
                    world.resource::<ForestModels>().hero_oak.as_ref().expect(
                        "the shipped Hero Oak GLBs should resolve into shared render assets",
                    );
                let meshes = world.resource::<Assets<Mesh>>();
                let foliage_materials = world.resource::<Assets<FoliageMaterial>>();
                let images = world.resource::<Assets<Image>>();
                for (level, (lod, count)) in
                    oak.lods.iter().zip([33_524, 4_384, 626, 8]).enumerate()
                {
                    let leaves = meshes.get(&lod.foliage).unwrap();
                    let wood_indices = lod.wood.as_ref().map_or(0, |wood| {
                        meshes.get(&wood.mesh).unwrap().indices().unwrap().len()
                    });
                    assert_eq!((leaves.indices().unwrap().len() + wood_indices) / 3, count);
                    assert_eq!(lod.wood.is_some(), level < 3);
                    assert!(leaves.contains_attribute(Mesh::ATTRIBUTE_UV_1));
                    assert!(leaves.contains_attribute(Mesh::ATTRIBUTE_COLOR));

                    let material = foliage_materials.get(&lod.foliage_material).unwrap();
                    assert_eq!(material.base.alpha_mode, AlphaMode::Mask(ALPHA_CUTOFF));
                    assert_eq!(material.base.cull_mode, (level == 3).then_some(Face::Back));
                    let texture = images
                        .get(material.base.base_color_texture.as_ref().unwrap())
                        .unwrap();
                    assert!(texture.texture_descriptor.mip_level_count > 1);
                }
                assert_eq!(oak.lods[0].foliage_material, oak.lods[1].foliage_material);
                assert_eq!(oak.lods[1].foliage_material, oak.lods[2].foliage_material);
                assert_ne!(oak.lods[2].foliage_material, oak.lods[3].foliage_material);
                break;
            }
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "timed out loading Hero Oak"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
