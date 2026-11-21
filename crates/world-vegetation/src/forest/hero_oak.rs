use bevy::{
    asset::{LoadState, RecursiveDependencyLoadState},
    gltf::{Gltf, GltfMesh, GltfMaterial},
    prelude::*,
    camera::primitives::MeshAabb,
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    render::{
        mesh::VertexAttributeValues,
        render_resource::{Face, TextureDimension, TextureFormat, TextureViewDimension},
    },
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
pub(super) struct HeroOakAssets(Option<OakFiles>);

struct OakFiles {
    lods: [Handle<Gltf>; 4],
    bark: [Handle<Image>; 3],
}

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
        materials: &Assets<GltfMaterial>,
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
        bark_maps: Option<&[Handle<Image>; 3]>,
    ) -> Option<HeroOak> {
        for handle in self.leaves.iter().chain(self.wood.iter()) {
            meshes.get(handle)?;
        }
        for handle in [&self.bark_image, &self.leaf_image, &self.impostor_image] {
            images.get(handle)?;
        }

        let mut bark = StandardMaterial {
            double_sided: true,
            cull_mode: None,
            metallic: 0.0,
            ..default()
        };
        if let Some([color, normal, roughness]) = bark_maps {
            bark.base_color_texture = Some(color.clone());
            bark.metallic_roughness_texture = Some(roughness.clone());
            bark.perceptual_roughness = 1.0;
            if self.wood.iter().all(|handle| {
                meshes.get_mut(handle).is_some_and(|mut mesh| {
                    mesh.contains_attribute(Mesh::ATTRIBUTE_TANGENT)
                        || mesh.generate_tangents().is_ok()
                })
            }) {
                bark.normal_map_texture = Some(normal.clone());
            }
        } else {
            bark.base_color_texture = Some(textures::import_mipmapped(
                images,
                &self.bark_image,
                None,
                true,
            ));
            bark.perceptual_roughness = 0.98;
        }
        let bark = materials.add(bark);
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
            let mut mesh = meshes.get_mut(leaves)?;
            let foliage_bounds = mesh.get_aabb()?;
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
            drop(mesh); // Release the AssetMut guard before reading other meshes.

            let wood = self.wood.get(level).and_then(|handle| {
                Some(WoodLod {
                    mesh: handle.clone(),
                    bounds: meshes.get(handle)?.get_aabb()?,
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
    let lods = std::array::from_fn(|lod| assets.load(format!("models/hero_oak/oak_lod{lod}.glb")));
    let bark = [
        ("models/hero_oak/bark/albedo.ktx2", true),
        ("models/hero_oak/bark/normal.ktx2", false),
        ("models/hero_oak/bark/metallic-roughness.ktx2", false),
    ]
    .map(|(path, is_srgb)| {
        assets.load_builder().with_settings(move |settings: &mut ImageLoaderSettings| {
            settings.is_srgb = is_srgb;
            settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                mag_filter: ImageFilterMode::Linear,
                min_filter: ImageFilterMode::Linear,
                mipmap_filter: ImageFilterMode::Linear,
                ..default()
            });
        }).load(path)
    });
    commands.insert_resource(HeroOakAssets(Some(OakFiles { lods, bark })));
}

pub(super) fn finish_loading(
    assets: Res<AssetServer>,
    mut pending: ResMut<HeroOakAssets>,
    mut models: ResMut<ForestModels>,
    gltfs: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<GltfMesh>>,
    gltf_materials: Res<Assets<GltfMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut foliage_materials: ResMut<Assets<FoliageMaterial>>,
) {
    let Some(files) = pending.0.as_ref() else {
        return;
    };
    if files.lods.iter().any(|file| {
        matches!(assets.load_state(file.id()), LoadState::Failed(_))
            || matches!(
                assets.get_recursive_dependency_load_state(file.id()),
                Some(RecursiveDependencyLoadState::Failed(_))
            )
    }) {
        warn!("Hero Oak failed to load. Keeping the procedural oak meshes.");
        pending.0 = None;
        return;
    }
    if !files
        .lods
        .iter()
        .all(|file| assets.is_loaded_with_dependencies(file.id()))
    {
        return;
    }
    let bark_maps = if let [Some(albedo), Some(normal), Some(roughness)] =
        files.bark.each_ref().map(|handle| images.get(handle))
    {
        if valid_bark_maps(albedo, normal, roughness) {
            Some(&files.bark)
        } else {
            warn!("Hero Oak bark maps have an incompatible layout. Using the bark embedded in the GLBs.");
            None
        }
    } else if files
        .bark
        .iter()
        .any(|image| matches!(assets.load_state(image.id()), LoadState::Failed(_)))
    {
        warn!("Hero Oak bark maps failed to load. Using the bark embedded in the GLBs.");
        None
    } else {
        return;
    };
    models.hero_oak = OakPrimitives::resolve(&files.lods, &gltfs, &gltf_meshes, &gltf_materials)
        .and_then(|parts| {
            parts.build(
                &mut meshes,
                &mut images,
                &mut materials,
                &mut foliage_materials,
                bark_maps,
            )
        });
    if models.hero_oak.is_none() {
        warn!("Hero Oak meshes or textures are unavailable. Keeping the procedural oak meshes.");
    }
    pending.0 = None;
}

fn valid_bark_maps(albedo: &Image, normal: &Image, roughness: &Image) -> bool {
    [
        (albedo, TextureFormat::Rgba8UnormSrgb),
        (normal, TextureFormat::Rgba8Unorm),
        (roughness, TextureFormat::Rgba8Unorm),
    ]
    .into_iter()
    .all(|(image, format)| {
        let descriptor = &image.texture_descriptor;
        let size = descriptor.size;
        let view = image
            .texture_view_descriptor
            .as_ref()
            .and_then(|view| view.dimension);
        descriptor.dimension == TextureDimension::D2
            && matches!(view, None | Some(TextureViewDimension::D2))
            && descriptor.format == format
            && size.depth_or_array_layers == 1
            && size == albedo.texture_descriptor.size
            && descriptor.mip_level_count == albedo.texture_descriptor.mip_level_count
            && descriptor.mip_level_count > 1
    })
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
                let materials = world.resource::<Assets<StandardMaterial>>();
                let bark = materials.get(&oak.bark).unwrap();
                assert_eq!(bark.metallic, 0.0);
                for (texture, format) in [
                    (&bark.base_color_texture, TextureFormat::Rgba8UnormSrgb),
                    (&bark.normal_map_texture, TextureFormat::Rgba8Unorm),
                    (&bark.metallic_roughness_texture, TextureFormat::Rgba8Unorm),
                ] {
                    let image = images.get(texture.as_ref().unwrap()).unwrap();
                    assert_eq!(image.texture_descriptor.format, format);
                    assert!(image.texture_descriptor.mip_level_count > 1);
                }
                for (level, (lod, count)) in
                    oak.lods.iter().zip([33_524, 4_384, 626, 8]).enumerate()
                {
                    let leaves = meshes.get(&lod.foliage).unwrap();
                    let wood_indices = lod.wood.as_ref().map_or(0, |wood| {
                        let mesh = meshes.get(&wood.mesh).unwrap();
                        assert_trunk_material_basis(mesh);
                        mesh.indices().unwrap().len()
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

    fn assert_trunk_material_basis(mesh: &Mesh) {
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("wood should have UV0");
        };
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("wood should have positions");
        };
        let Some(VertexAttributeValues::Float32x3(normals)) =
            mesh.attribute(Mesh::ATTRIBUTE_NORMAL)
        else {
            panic!("wood should have normals");
        };
        let Some(VertexAttributeValues::Float32x4(tangents)) =
            mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            panic!("wood should have tangents");
        };
        assert!(tangents.iter().all(|&t| Vec4::from_array(t).is_finite()));

        // The first ring of the main trunk has a duplicated seam vertex.
        let seam = uvs.iter().position(|&uv| uv == [4.0, 1.0]).unwrap();
        assert_eq!(uvs[0], [0.0, 1.0]);
        assert!(Vec3::from_array(positions[0]).abs_diff_eq(Vec3::from_array(positions[seam]), 1e-5));
        assert_eq!(normals[0], normals[seam]);
        let center = positions[..seam]
            .iter()
            .map(|&p| Vec3::from_array(p))
            .sum::<Vec3>()
            / seam as f32;
        for (&position, &normal) in positions[..seam].iter().zip(&normals[..seam]) {
            assert!((Vec3::from_array(position) - center).dot(Vec3::from_array(normal)) > 0.0);
        }
        let mut indices = mesh.indices().unwrap().iter();
        let [a, b, c] =
            std::array::from_fn(|_| Vec3::from_array(positions[indices.next().unwrap()]));
        assert!((b - a).cross(c - a).dot(a - center) > 0.0);
    }
}
