//! Render a moving layer of volumetric cumulus in the planet's frame.

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    light::NotShadowCaster,
    material::AlphaMode,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, Extent3d, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError,
        TextureDimension, TextureFormat,
    },
    shader::ShaderRef,
};
use world_core::{
    planet::RADIUS,
    runtime::{PlanetRoot, WorldReady},
};

const BASE: f32 = RADIUS as f32 + 1_650.0;
const TOP: f32 = RADIUS as f32 + 3_850.0;
const NOISE_SIZE: u32 = 64;
const SHADER: &str = "shaders/planet_clouds.wesl";

pub(super) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CloudMaterial>::default())
        .add_systems(Startup, spawn.after(WorldReady));
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct CloudMaterial {
    #[uniform(0)]
    radii: Vec4,
    #[uniform(0)]
    sun: Vec4,
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    noise: Handle<Image>,
}

impl Material for CloudMaterial {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }

    fn depth_bias(&self) -> f32 {
        // Sort after the transparent water surface when looking down from orbit.
        (2.0 * RADIUS) as f32
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // The far face launches one ray per pixel from both inside and outside the shell.
        descriptor.primitive.cull_mode = Some(Face::Front);
        Ok(())
    }
}

fn spawn(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<CloudMaterial>>,
) {
    // Enclose the analytic cloud volume even between the proxy's vertices.
    let proxy = Sphere::new(TOP + 200.0).mesh().ico(24).unwrap();
    root.spawn(
        &mut commands,
        (
            Name::new("Planetary clouds"),
            Mesh3d(meshes.add(proxy)),
            MeshMaterial3d(materials.add(CloudMaterial {
                radii: Vec4::new(RADIUS as f32, BASE, TOP, 1.0 / (TOP - BASE)),
                sun: super::SUN_DIRECTION.normalize().extend(0.0),
                noise: images.add(noise_volume()),
            })),
            NotShadowCaster,
        ),
    );
}

fn noise_volume() -> Image {
    let mut data = Vec::with_capacity((NOISE_SIZE.pow(3) * 2) as usize);
    for z in 0..NOISE_SIZE {
        for y in 0..NOISE_SIZE {
            for x in 0..NOISE_SIZE {
                let p = (UVec3::new(x, y, z).as_vec3() + 0.5) / NOISE_SIZE as f32;
                let broad = 0.57 * value_noise(p, 4, 41)
                    + 0.27 * value_noise(p, 8, 113)
                    + 0.11 * value_noise(p, 16, 174)
                    + 0.05 * value_noise(p, 32, 237);
                let detail = 0.65 * value_noise(p, 8, 317) + 0.35 * value_noise(p, 16, 97);
                data.extend([broad, detail].map(|v| (255.0 * v).round() as u8));
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: NOISE_SIZE,
            height: NOISE_SIZE,
            depth_or_array_layers: NOISE_SIZE,
        },
        TextureDimension::D3,
        data,
        TextureFormat::Rg8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn value_noise(position: Vec3, period: u32, seed: u32) -> f32 {
    let p = position * period as f32;
    let cell = p.floor().as_uvec3();
    let f = p.fract();
    let weight = f * f * (3.0 - 2.0 * f);
    let lattice = |x, y, z| {
        let c = (cell + UVec3::new(x, y, z)) % period;
        let mut h = seed ^ (c.x + period * (c.y + period * c.z)).wrapping_mul(0x9e37_79b9);
        h = (h ^ (h >> 16)).wrapping_mul(0x7feb_352d);
        h = (h ^ (h >> 15)).wrapping_mul(0x846c_a68b);
        ((h ^ (h >> 16)) >> 8) as f32 / 16_777_215.0
    };
    let slice = |z| {
        let a = lattice(0, 0, z).lerp(lattice(1, 0, z), weight.x);
        let b = lattice(0, 1, z).lerp(lattice(1, 1, z), weight.x);
        a.lerp(b, weight.y)
    };
    slice(0).lerp(slice(1), weight.z)
}
