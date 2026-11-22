#![warn(missing_docs)]
//! Adaptive spherical terrain rendering and its lighting and atmosphere.

mod terrain_surface;

use bevy::{
    asset::LoadState,
    pbr::MaterialPlugin,
    light::{atmosphere::ScatteringMedium, light_consts::lux, Atmosphere, SunDisk},
    prelude::*,
    reflect::TypePath,
    shader::ShaderRef,
    image::{ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    render::{
        render_resource::{AsBindGroup, TextureFormat, TextureViewDimension},

    },
};
use world_core::{planet::RADIUS, runtime::{PlanetRoot, WorldMovement, WorldReady}};

/// Register terrain materials, chunk streaming, lighting, and atmospheric fog.
///
/// Install with the shared world space plugin. Streaming reads a camera marked
/// with [`WorldCamera`](world_core::runtime::WorldCamera).
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default())
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(Startup, setup.after(WorldReady))
        .add_systems(Update, load_detail.run_if(resource_exists::<TerrainDetail>))
        .add_systems(Update, toggle_canopy_debug)
        .add_systems(Update, terrain_surface::stream.after(WorldMovement));
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
    // F9 cycles normal terrain, forest density and canopy color.
    #[uniform(6)]
    terrain_debug_mode: u32,
    #[uniform(7)]
    planet_radius: f32,
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
            terrain_debug_mode: 0,
            planet_radius: RADIUS as f32,
        }
    }
}
/// Cycle diagnostic overlays for global forest coverage and its canopy colors.
fn toggle_canopy_debug(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    terrain: Option<Res<terrain_surface::TerrainSurface>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    if !keyboard.is_some_and(|input| input.just_pressed(KeyCode::F9)) {
        return;
    }
    let Some(terrain) = terrain else {
        return;
    };
    if let Some(mut material) = materials.get_mut(&terrain.material) {
        material.terrain_debug_mode = (material.terrain_debug_mode + 1) % 3;
        info!(
            "Planet forest diagnostic: {} (F9 to cycle)",
            match material.terrain_debug_mode {
                1 => "forest coverage (grayscale)",
                2 => "canopy colors",
                _ => "normal terrain",
            }
        );
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
        "shaders/spherical.wesl".into()
    }
}

fn setup(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
    mut mediums: ResMut<Assets<ScatteringMedium>>,
    assets: Res<AssetServer>,
) {
    let material = materials.add(TerrainMaterial::load(&assets));
    commands.insert_resource(TerrainDetail::request(material.clone(), &assets));
    commands.insert_resource(terrain_surface::TerrainSurface::new(material));

    // One physically based atmosphere centered on the actual spherical planet,
    // not on a ground-level flat plane. RADIUS is in meters, as required by
    // Bevy's scattering equations. The planet root is transformed by big_space
    // relative to the floating origin, so the atmosphere follows that origin.
    // Keep the Earth medium's 60 km density profile to preserve scale heights.
    let scattering = mediums.add(ScatteringMedium::earth(256, 256));
    commands.entity(root.entity()).insert(Atmosphere {
        inner_radius: RADIUS as f32,
        outer_radius: RADIUS as f32 + 60_000.0,
        ground_albedo: Vec3::splat(0.27),
        medium: scattering,
    });

    // Sun direction in the planet reference frame: afternoon/daylight at the
    // starter region, with an oblique angle to show terrain and tree relief.
    let sun_direction = Vec3::new(-0.78, 0.60, -0.18).normalize();
    commands.spawn((
        DirectionalLight {
            // Raw sunlight is attenuated by the atmosphere during rendering.
            illuminance: lux::RAW_SUNLIGHT,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_translation(sun_direction * 1_000.0)
            .looking_at(Vec3::ZERO, Vec3::Y),
        SunDisk::EARTH,
    ));

    // The atmosphere's generated environment lighting supplies the blue sky
    // fill. A large uniform ambient term would wash out shadow contrast.
    commands.insert_resource(GlobalAmbientLight::NONE);
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bevy::{asset::AssetPlugin, image::{CompressedImageFormats, ImageLoader, ImagePlugin}};

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
        .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
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
