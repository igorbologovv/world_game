//! Activate each matched texture pair only after both arrays have loaded.

use bevy::{
    asset::LoadState,
    image::ImageLoaderSettings,
    prelude::*,
    render::render_resource::{TextureFormat, TextureViewDimension},
};

use crate::TerrainMaterial;

const BIOME_LAYERS: u32 = 34;

#[derive(Resource)]
pub(super) struct SurfaceTextures {
    material: Handle<TerrainMaterial>,
    legacy: Option<TexturePair>,
    biome: Option<TexturePair>,
}

struct TexturePair {
    color: Handle<Image>,
    finish: Handle<Image>,
    layers: u32,
}

impl TexturePair {
    fn request(
        assets: &AssetServer,
        color: &'static str,
        finish: &'static str,
        layers: u32,
    ) -> Self {
        Self {
            color: assets.load(color),
            finish: assets
                .load_builder()
                .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
                .load(finish),
            layers,
        }
    }

    fn poll(
        &self,
        assets: &AssetServer,
        images: &Assets<Image>,
    ) -> Option<Result<(), &'static str>> {
        if let (Some(color), Some(finish)) = (images.get(&self.color), images.get(&self.finish)) {
            return Some(
                valid_pair(color, finish, self.layers)
                    .then_some(())
                    .ok_or("incompatible array layout"),
            );
        }
        [&self.color, &self.finish]
            .iter()
            .any(|map| matches!(assets.load_state(map.id()), LoadState::Failed(_)))
            .then_some(Err("texture load failed"))
    }
}

impl SurfaceTextures {
    pub(super) fn request(
        material: Handle<TerrainMaterial>,
        assets: &AssetServer,
        biome: bool,
    ) -> Self {
        Self {
            material,
            legacy: Some(TexturePair::request(
                assets,
                "textures/terrain/detail-albedo.ktx2",
                "textures/terrain/detail-normal-roughness.ktx2",
                4,
            )),
            biome: biome.then(|| {
                TexturePair::request(
                    assets,
                    "textures/terrain/biome-albedo-height.ktx2",
                    "textures/terrain/biome-normal-roughness.ktx2",
                    BIOME_LAYERS,
                )
            }),
        }
    }
}

pub(super) fn load(
    mut commands: Commands,
    mut textures: ResMut<SurfaceTextures>,
    assets: Res<AssetServer>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let textures = &mut *textures;
    for (label, pending) in [
        ("legacy detail", &mut textures.legacy),
        ("biome surface", &mut textures.biome),
    ] {
        let Some(result) = pending
            .as_ref()
            .and_then(|pair| pair.poll(&assets, &images))
        else {
            continue;
        };
        let pair = pending.take().unwrap();
        match result {
            Ok(()) => {
                if let Some(mut material) = materials.get_mut(&textures.material) {
                    if pair.layers == BIOME_LAYERS {
                        material.biome_albedo_height = pair.color;
                        material.biome_normal_roughness = pair.finish;
                        material.biome_enabled = 1;
                    } else {
                        material.detail_albedo = pair.color;
                        material.detail_normal_roughness = pair.finish;
                        material.detail_enabled = 1;
                    }
                }
            }
            Err(reason) => warn!(
                "Terrain {label} is unavailable ({reason}). Using the remaining terrain layers."
            ),
        }
    }
    if textures.legacy.is_none() && textures.biome.is_none() {
        commands.remove_resource::<SurfaceTextures>();
    }
}

fn valid_pair(color: &Image, finish: &Image, layers: u32) -> bool {
    [
        (color, TextureFormat::Rgba8UnormSrgb),
        (finish, TextureFormat::Rgba8Unorm),
    ]
    .into_iter()
    .all(|(image, format)| {
        let descriptor = &image.texture_descriptor;
        let size = descriptor.size;
        image
            .texture_view_descriptor
            .as_ref()
            .and_then(|view| view.dimension)
            == Some(TextureViewDimension::D2Array)
            && descriptor.format == format
            && size.depth_or_array_layers == layers
            && size.width == size.height
            && size.width.is_power_of_two()
            && descriptor.mip_level_count == size.width.ilog2() + 1
    }) && color.texture_descriptor.size == finish.texture_descriptor.size
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bevy::{
        asset::AssetPlugin,
        image::{CompressedImageFormats, ImageLoader, ImagePlugin},
    };

    use super::*;

    fn check_texture_loading(missing_biome_color: bool) {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin {
                file_path: concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets").into(),
                ..default()
            },
            ImagePlugin::default(),
        ))
        .init_asset::<TerrainMaterial>()
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
        .add_systems(Update, load.run_if(resource_exists::<SurfaceTextures>));
        app.finish();
        app.cleanup();

        let world = app.world_mut();
        let assets = world.resource::<AssetServer>().clone();
        let material = world
            .resource_mut::<Assets<TerrainMaterial>>()
            .add(TerrainMaterial::load(&assets));
        let mut details = SurfaceTextures::request(material.clone(), &assets, true);
        let biome = details.biome.as_mut().unwrap();
        if missing_biome_color {
            biome.color = assets.load("textures/terrain/absent-biome.ktx2");
        }
        let biome_color = biome.color.clone();
        let biome_finish = biome.finish.clone();
        world.insert_resource(details);

        let start = Instant::now();
        while app.world().contains_resource::<SurfaceTextures>() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "texture load stalled"
            );
            app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        let loaded = app
            .world()
            .resource::<Assets<TerrainMaterial>>()
            .get(&material)
            .unwrap();
        assert_eq!(loaded.detail_enabled, 1);
        if missing_biome_color {
            assert_eq!(loaded.biome_enabled, 0);
            assert_eq!(loaded.biome_albedo_height, loaded.albedo);
            assert_eq!(loaded.biome_normal_roughness, loaded.normal_roughness);
        } else {
            assert_eq!(loaded.biome_enabled, 1);
            assert_eq!(loaded.biome_albedo_height, biome_color);
            assert_eq!(loaded.biome_normal_roughness, biome_finish);
        }
    }

    #[test]
    fn shipped_biome_arrays_activate_together() {
        check_texture_loading(false);
    }

    #[test]
    fn missing_biome_map_keeps_legacy_detail_renderable() {
        check_texture_loading(true);
    }
}
