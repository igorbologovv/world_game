//! The transparent planet-centered cloud volume.

mod fields;
mod volumetric;

use bevy::{
    light::NotShadowCaster,
    material::AlphaMode,
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    },
    shader::ShaderRef,
};
use world_core::{
    planet::RADIUS,
    runtime::{PlanetRoot, WorldReady},
};

const BASE: f32 = 1_450.0;
const TOP: f32 = 7_900.0;
const SHADER: &str = "shaders/planet_clouds.wesl";

pub(super) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<CloudMaterial>::default())
        .init_resource::<CloudMode>()
        .add_systems(Startup, spawn.after(WorldReady))
        .add_systems(Update, select_renderer);
    volumetric::install(app);
}

#[derive(Default, Resource, Clone, Copy, PartialEq, Eq)]
enum CloudMode {
    #[default]
    Volumetric,
    Legacy,
    Clear,
}

#[derive(Component)]
struct CloudVisibility;

#[derive(Component)]
struct LegacyProxy;

#[derive(Resource)]
struct CloudFields {
    shape: Handle<Image>,
    weather: Handle<Image>,
}

fn select_renderer(
    keyboard: Option<Res<ButtonInput<KeyCode>>>,
    mut mode: ResMut<CloudMode>,
    mut proxy: Query<&mut Visibility, With<LegacyProxy>>,
) {
    if keyboard.is_some_and(|keys| keys.just_pressed(KeyCode::F6)) {
        *mode = match *mode {
            CloudMode::Volumetric => CloudMode::Legacy,
            CloudMode::Legacy => CloudMode::Clear,
            CloudMode::Clear => CloudMode::Volumetric,
        };
        info!(
            "Planetary clouds (F6): {}",
            match *mode {
                CloudMode::Volumetric => "volumetric",
                CloudMode::Legacy => "legacy",
                CloudMode::Clear => "clear",
            }
        );
    }
    if mode.is_changed() {
        if let Ok(mut visibility) = proxy.single_mut() {
            *visibility = if *mode == CloudMode::Legacy {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
    }
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct CloudMaterial {
    #[uniform(0)]
    radii: Vec4,
    #[texture(1, dimension = "3d")]
    #[sampler(2)]
    billows: Handle<Image>,
    #[texture(3, dimension = "cube")]
    #[sampler(4)]
    weather: Handle<Image>,
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
        // Composite over the transparent ocean as well as opaque terrain.
        (2.0 * RADIUS) as f32
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // The enclosing sphere covers every ray once, including from inside.
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
    let radius = RADIUS as f32;
    // The margin encloses the analytic shell between the proxy's vertices.
    let proxy = Sphere::new(radius + TOP + 500.0).mesh().ico(24).unwrap();
    let weather = images.add(fields::weather());
    commands.insert_resource(CloudFields {
        shape: images.add(fields::shape()),
        weather: weather.clone(),
    });
    root.spawn(
        &mut commands,
        (Name::new("Planetary clouds"), CloudVisibility),
    )
    .with_children(|clouds| {
        clouds.spawn((
            LegacyProxy,
            Visibility::Hidden,
            Mesh3d(meshes.add(proxy)),
            MeshMaterial3d(materials.add(CloudMaterial {
                radii: Vec4::new(radius, radius + BASE, radius + TOP, 0.0),
                billows: images.add(fields::billows()),
                weather,
            })),
            NotShadowCaster,
        ));
    });
}
