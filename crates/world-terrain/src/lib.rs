#![warn(missing_docs)]
//! Adaptive spherical terrain rendering and its lighting and atmosphere.

mod terrain_surface;

use bevy::{
    asset::LoadState,
    pbr::MaterialPlugin,
    prelude::*,
    reflect::TypePath,
    shader::ShaderRef,
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    render::{
        render_resource::{AsBindGroup, TextureFormat, TextureViewDimension},

    },
};
use world_core::runtime::{CameraMotion, WorldMovement};

/// Register terrain materials, chunk streaming, lighting, and atmospheric fog.
///
/// Install with the shared world space plugin. Streaming reads a camera marked
/// with [`WorldCamera`](world_core::runtime::WorldCamera).
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default())
        .add_systems(Startup, setup)
        .add_systems(Update, load_detail.run_if(resource_exists::<TerrainDetail>))
        .add_systems(Update, terrain_surface::stream.after(WorldMovement))
        .add_systems(PostUpdate, terrain_surface::atmosphere.after(CameraMotion));
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct TerrainMaterial {
    #[texture(0, dimension = "2d_array")]
    #[sampler(1)]
    albedo: Handle<Image>,
    #[texture(2, dimension = "2d_array")]
    normal_roughness: Handle<Image>,
    #[texture(3, dimension = "2d_array")]
    detail_albedo: Handle<Image>,
    #[texture(4, dimension = "2d_array")]
    detail_normal_roughness: Handle<Image>,
    #[uniform(5)]
    detail_enabled: u32,
}

impl TerrainMaterial {
    fn load(asset_server: &AssetServer) -> Self {
        // Base layers use the order sand, grass, dirt, rock, snow.
        let albedo = asset_server.load_builder().with_settings(
            |settings: &mut ImageLoaderSettings| {
                settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    address_mode_u: ImageAddressMode::Repeat,
                    address_mode_v: ImageAddressMode::Repeat,
                    mag_filter: ImageFilterMode::Linear,
                    min_filter: ImageFilterMode::Linear,
                    mipmap_filter: ImageFilterMode::Linear,
                    ..default()
                });
            },
        ).load("textures/terrain/albedo.ktx2");
        let normal_roughness = asset_server.load_builder()
            .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
            .load("textures/terrain/normal-roughness.ktx2");
        Self {
            // Valid array bindings let the base terrain render during detail loading.
            detail_albedo: albedo.clone(),
            detail_normal_roughness: normal_roughness.clone(),
            albedo,
            normal_roughness,
            detail_enabled: 0,
        }
    }
}

#[derive(Resource)]
struct TerrainDetail {
    material: Handle<TerrainMaterial>,
    albedo: Handle<Image>,
    normal_roughness: Handle<Image>,
}

impl TerrainDetail {
    fn request(material: Handle<TerrainMaterial>, assets: &AssetServer) -> Self {
        Self {
            material,
            albedo: assets.load("textures/terrain/detail-albedo.ktx2"),
            normal_roughness: assets.load_builder()
                .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
                .load("textures/terrain/detail-normal-roughness.ktx2"),
        }
    }
}

fn load_detail(
    mut commands: Commands,
    detail: Res<TerrainDetail>,
    assets: Res<AssetServer>,
    images: Res<Assets<Image>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    match (
        images.get(&detail.albedo),
        images.get(&detail.normal_roughness),
    ) {
        (Some(albedo), Some(normal_roughness)) => {
            if valid_detail_pair(albedo, normal_roughness) {
                if let Some(mut material) = materials.get_mut(&detail.material) {
                    material.detail_albedo = detail.albedo.clone();
                    material.detail_normal_roughness = detail.normal_roughness.clone();
                    material.detail_enabled = 1;
                }
            } else {
                warn!("Terrain detail arrays have an incompatible layout. Using the base terrain material.");
            }
        }
        _ if [&detail.albedo, &detail.normal_roughness]
            .iter()
            .any(|map| matches!(assets.load_state(map.id()), LoadState::Failed(_))) =>
        {
            warn!("Terrain detail arrays failed to load. Using the base terrain material.");
        }
        _ => return,
    }
    commands.remove_resource::<TerrainDetail>();
}

fn valid_detail_pair(albedo: &Image, normal_roughness: &Image) -> bool {
    [
        (albedo, TextureFormat::Rgba8UnormSrgb),
        (normal_roughness, TextureFormat::Rgba8Unorm),
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
            && size.depth_or_array_layers == 4
            && size.width == size.height
            && size.width.is_power_of_two()
            && descriptor.mip_level_count == size.width.ilog2() + 1
    }) && albedo.texture_descriptor.size == normal_roughness.texture_descriptor.size
}

impl Material for TerrainMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/spherical.wgsl".into()
    }
}

fn setup(
    mut commands: Commands,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    assets: Res<AssetServer>,
) {
    let material = materials.add(TerrainMaterial::load(&assets));
    commands.insert_resource(TerrainDetail::request(material.clone(), &assets));
    commands.insert_resource(terrain_surface::TerrainSurface::new(material));
    commands.spawn((
        DirectionalLight {
            illuminance: 5000.0,
            ..default()
        },
        Transform::from_xyz(-1.0, 1.0, -1.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.82, 0.90, 1.0),
        brightness: 295.0,
        ..default()
    });
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bevy::{asset::AssetPlugin, render::texture::ImagePlugin};

    use super::*;

    fn check_detail_loading(missing_albedo: bool) {
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
        .add_systems(Update, load_detail.run_if(resource_exists::<TerrainDetail>));
        app.finish();
        app.cleanup();

        let world = app.world_mut();
        let assets = world.resource::<AssetServer>().clone();
        let material = world
            .resource_mut::<Assets<TerrainMaterial>>()
            .add(TerrainMaterial::load(&assets));
        let mut detail = TerrainDetail::request(material.clone(), &assets);
        if missing_albedo {
            detail.albedo = assets.load("textures/terrain/absent-detail.ktx2");
        }
        let detail_albedo = detail.albedo.clone();
        let detail_normal = detail.normal_roughness.clone();
        world.insert_resource(detail);

        let start = Instant::now();
        while app.world().contains_resource::<TerrainDetail>() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "detail load stalled"
            );
            app.update();
            std::thread::sleep(Duration::from_millis(5));
        }
        let loaded = app
            .world()
            .resource::<Assets<TerrainMaterial>>()
            .get(&material)
            .unwrap();
        if missing_albedo {
            assert_eq!(loaded.detail_enabled, 0);
            assert_eq!(loaded.detail_albedo, loaded.albedo);
            assert_eq!(loaded.detail_normal_roughness, loaded.normal_roughness);
        } else {
            assert_eq!(loaded.detail_enabled, 1);
            assert_eq!(loaded.detail_albedo, detail_albedo);
            assert_eq!(loaded.detail_normal_roughness, detail_normal);
        }
    }

    #[test]
    fn shipped_detail_arrays_replace_the_base_bindings_together() {
        check_detail_loading(false);
    }

    #[test]
    fn missing_detail_map_keeps_the_base_terrain_renderable() {
        check_detail_loading(true);
    }
}
