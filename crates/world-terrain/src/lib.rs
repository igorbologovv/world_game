#![warn(missing_docs)]
//! Adaptive spherical terrain rendering and its lighting and atmosphere.

mod celestial;
mod clouds;
mod surface_materials;
mod surface_water;
mod terrain_surface;

use bevy::{
    image::{
        ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
        ImageSamplerDescriptor,
    },
    light::{atmosphere::ScatteringMedium, Atmosphere},
    pbr::MaterialPlugin,
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};
use surface_materials::SurfaceTextures;
use world_core::{
    canopy::{CanopyLayout, ForestCanopy},
    planet::RADIUS,
    runtime::{PlanetRoot, WorldMovement, WorldReady},
};

/// Register terrain materials, chunk streaming, lighting, atmosphere, and clouds.
///
/// Install with the shared world space plugin. Streaming reads a camera marked
/// with [`WorldCamera`](world_core::runtime::WorldCamera).
pub struct TerrainPlugin;

impl Plugin for TerrainPlugin {
    fn build(&self, app: &mut App) {
        celestial::install(app);
        clouds::install(app);
        app.add_plugins(MaterialPlugin::<TerrainMaterial>::default())
            .insert_resource(ClearColor(Color::BLACK))
            .add_systems(Startup, setup.after(WorldReady))
            .add_systems(
                Update,
                surface_materials::load.run_if(resource_exists::<SurfaceTextures>),
            )
            .add_systems(Update, toggle_canopy_debug)
            .add_systems(Update, update_canopy)
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
    #[texture(8, dimension = "2d_array")]
    #[sampler(9)]
    canopy_atlas: Option<Handle<Image>>,
    #[uniform(10)]
    canopy_layout: CanopyLayout,
    #[texture(11)]
    #[sampler(12)]
    crown_detail: Option<Handle<Image>>,
    #[texture(13, dimension = "cube")]
    #[sampler(14)]
    globe_canopy: Option<Handle<Image>>,
    #[texture(15, dimension = "2d_array")]
    biome_albedo_height: Handle<Image>,
    #[texture(16, dimension = "2d_array")]
    biome_normal_roughness: Handle<Image>,
    #[uniform(17)]
    biome_enabled: u32,
}

impl TerrainMaterial {
    fn load(asset_server: &AssetServer) -> Self {
        // Base layers use the order sand, grass, dirt, rock, snow.
        let albedo = asset_server
            .load_builder()
            .with_settings(|settings: &mut ImageLoaderSettings| {
                settings.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
                    address_mode_u: ImageAddressMode::Repeat,
                    address_mode_v: ImageAddressMode::Repeat,
                    mag_filter: ImageFilterMode::Linear,
                    min_filter: ImageFilterMode::Linear,
                    mipmap_filter: ImageFilterMode::Linear,
                    ..default()
                });
            })
            .load("textures/terrain/albedo.ktx2");
        let normal_roughness = asset_server
            .load_builder()
            .with_settings(|settings: &mut ImageLoaderSettings| settings.is_srgb = false)
            .load("textures/terrain/normal-roughness.ktx2");
        Self {
            // Valid array bindings let the base terrain render during detail loading.
            detail_albedo: albedo.clone(),
            detail_normal_roughness: normal_roughness.clone(),
            biome_albedo_height: albedo.clone(),
            biome_normal_roughness: normal_roughness.clone(),
            biome_enabled: 0,
            albedo,
            normal_roughness,
            detail_enabled: 0,
            terrain_debug_mode: 0,
            planet_radius: RADIUS as f32,
            canopy_atlas: None,
            canopy_layout: CanopyLayout::default(),
            crown_detail: None,
            globe_canopy: None,
        }
    }
}

fn update_canopy(
    canopy: Option<Res<ForestCanopy>>,
    terrain: Option<Res<terrain_surface::TerrainSurface>>,
    mut materials: ResMut<Assets<TerrainMaterial>>,
) {
    let (Some(canopy), Some(terrain)) = (canopy, terrain) else {
        return;
    };
    if canopy.is_changed() || terrain.is_added() {
        if let Some(mut material) = materials.get_mut(&terrain.material) {
            material.canopy_atlas = Some(canopy.atlas.clone());
            material.crown_detail = Some(canopy.crowns.clone());
            material.globe_canopy = Some(canopy.globe.clone());
            material.canopy_layout = canopy.layout;
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
        material.terrain_debug_mode = (material.terrain_debug_mode + 1) % 4;
        info!(
            "Planet forest diagnostic: {} (F9 to cycle)",
            match material.terrain_debug_mode {
                1 => "forest coverage (grayscale)",
                2 => "canopy colors",
                3 => "canopy cascade contributions",
                _ => "normal terrain",
            }
        );
    }
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
    let biome_materials = std::env::var_os("WORLD_TERRAIN_LEGACY_DETAIL").is_none();
    commands.insert_resource(SurfaceTextures::request(
        material.clone(),
        &assets,
        biome_materials,
    ));
    commands.insert_resource(terrain_surface::TerrainSurface::new(
        material,
        biome_materials,
    ));

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

    // The atmosphere's generated environment lighting supplies the blue sky
    // fill. A large uniform ambient term would wash out shadow contrast.
    commands.insert_resource(GlobalAmbientLight::NONE);
}
