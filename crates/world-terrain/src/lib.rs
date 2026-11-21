#![warn(missing_docs)]
//! Adaptive spherical terrain rendering and its lighting and atmosphere.

mod terrain_surface;

use bevy::{
    pbr::MaterialPlugin,
    prelude::*,
    reflect::TypePath,
    render::{
        render_resource::{AsBindGroup, ShaderRef},
        texture::{
            ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
            ImageSamplerDescriptor,
        },
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
        app.add_plugins(MaterialPlugin::<TerrainMaterial> {
            prepass_enabled: false,
            shadows_enabled: false,
            ..default()
        })
        .add_systems(Startup, setup)
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
}

impl TerrainMaterial {
    fn load(asset_server: &AssetServer) -> Self {
        // Each mipmapped array has the same layer order: sand, grass, dirt, rock, snow.
        Self {
            albedo: asset_server.load_with_settings(
                "textures/terrain/albedo.ktx2",
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
            ),
            // Linear data: tangent-space normal in RGB, perceptual roughness in alpha.
            normal_roughness: asset_server.load_with_settings(
                "textures/terrain/normal-roughness.ktx2",
                |settings: &mut ImageLoaderSettings| settings.is_srgb = false,
            ),
        }
    }
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
    commands.insert_resource(terrain_surface::TerrainSurface::new(material));
    commands.spawn(DirectionalLightBundle {
        directional_light: DirectionalLight {
            illuminance: 5000.0,
            ..default()
        },
        transform: Transform::from_xyz(-1.0, 1.0, -1.0).looking_at(Vec3::ZERO, Vec3::Y),
        ..default()
    });
    commands.insert_resource(AmbientLight {
        color: Color::srgb(0.82, 0.90, 1.0),
        brightness: 295.0,
    });
}
