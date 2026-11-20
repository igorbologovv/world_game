mod planet;
mod river_surface;
mod player;
mod surface_heights;
mod trip;

use bevy::{
    gizmos::config::GizmoConfig,
    math::DVec3,
    pbr::{MaterialPlugin, NotShadowCaster},
    prelude::*,
    reflect::TypePath,
    render::{
        render_resource::*,
        texture::{
            ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
            ImageSamplerDescriptor,
        },
    },
    transform::TransformSystem,
};
use bevy_terrain::prelude::*;

const PATH: &str = "terrains/spherical";

use planet::RADIUS;
const STARTING_RIVER: DVec3 = DVec3::new(-0.35203830, 0.82923209, 0.43410042);

const MIN_HEIGHT: f32 = -12_000.0;
const MAX_HEIGHT: f32 = 9_000.0;

const TEXTURE_SIZE: u32 = 512;
const TERRAIN_BORDER_SIZE: u32 = 2;
const LOD_COUNT: u32 = 5;

//
// Terrain material
//

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct TerrainMaterial {
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

//
// Ocean material
//

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub struct OceanMaterial {
    #[texture(0)]
    ocean_mask: Handle<Image>,
}

impl Material for OceanMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/ocean.wgsl".into()
    }
}

fn main() {
    App::new()
        .add_plugins((
            DefaultPlugins
                .build()
                .disable::<TransformPlugin>(),
            TerrainPlugin,
            TerrainMaterialPlugin::<TerrainMaterial>::default(),
            TerrainDebugPlugin,
            MaterialPlugin::<OceanMaterial>::default(),
        ))
        .insert_gizmo_config(river_surface::RiverMapLines, GizmoConfig {
            line_width: 2.25,
            ..default()
        })
        .add_systems(PostUpdate, river_surface::draw_map.after(TransformSystem::TransformPropagate))
        .init_resource::<trip::TravelSpeed>()
        .add_systems(Startup, (setup, trip::spawn))
        .add_systems(Update, (trip::change_speed, player::walk, player::animate, trip::update).chain())
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut ocean_materials: ResMut<Assets<OceanMaterial>>,
    mut tile_trees: ResMut<TerrainViewComponents<TileTree>>,
    asset_server: Res<AssetServer>,
) {
    //
    // Terrain material
    //

    let terrain_material = terrain_materials.add(TerrainMaterial::load(&asset_server));

    //
    // Planet
    //

    let config = TerrainConfig {
        lod_count: LOD_COUNT,

        model: TerrainModel::sphere(
            DVec3::ZERO,
            RADIUS,
            MIN_HEIGHT,
            MAX_HEIGHT,
        ),

        path: PATH.to_string(),

        ..default()
    }
    .add_attachment(AttachmentConfig {
        name: "height".to_string(),
        texture_size: TEXTURE_SIZE,
        border_size: TERRAIN_BORDER_SIZE,
        mip_level_count: 4,
        format: AttachmentFormat::R16,
    });

    let view_config = TerrainViewConfig {
        grid_size: 64,
        load_distance: 3.5,
        blend_distance: 2.8,
        morph_distance: 20.0,
        precision_threshold_distance: 0.00003,
        ..default()
    };

    let tile_atlas =
        TileAtlas::new(&config);

    let tile_tree =
        TileTree::new(
            &tile_atlas,
            &view_config,
        );

    //
    // Ocean mask
    //

    let ocean_mask =
        asset_server.load(
            "world/ocean_atlas.png"
        );

    //
    // World
    //

    commands.insert_resource(surface_heights::SurfaceHeights::load());

    commands.spawn(TextBundle::from_section(
        "Click land to move     Scroll to zoom     Right-drag to turn     V: Toggle POV     Esc: Back or clear     T: Free camera",
        TextStyle { font_size: 17.0, color: Color::WHITE, ..default() },
    ).with_style(Style {
        position_type: PositionType::Absolute,
        left: Val::Px(18.0),
        bottom: Val::Px(16.0),
        ..default()
    }));

    let river = river_surface::load().map(|(surface, centerlines)| {
        commands.insert_resource(centerlines);
        surface
    });
    commands.spawn_big_space(
        ReferenceFrame::default(),
        |root| {
            let frame =
                root.frame().clone();

            //
            // Terrain
            //

            let terrain = root
                .spawn_spatial((
                    TerrainBundle::new(
                        tile_atlas,
                        &frame,
                    ),
                    terrain_material,
                ))
                .id();

            //
            // Camera
            //

            let player = player::Player::default();
            let map = player::MapView::default();
            let (camera_position, rotation) = player.camera_pose(map.altitude(1.0), &map);
            let mut camera = DebugCameraBundle::new(camera_position, 300.0, &frame);
            camera.camera.transform.rotation = rotation;
            if let Projection::Perspective(perspective) = &mut camera.camera.projection {
                perspective.near = 0.05;
                perspective.far = RADIUS as f32 * 20.0;
            }
            let view = root.spawn_spatial(camera).id();
            root.spawn_spatial((player, SpatialBundle::HIDDEN_IDENTITY))
                .with_children(|children| player::spawn_model(children, &mut meshes, &mut materials));

            root.spawn_spatial((player::Destination, PbrBundle {
                mesh: meshes.add(Torus::new(0.60, 0.84)),
                material: materials.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.84, 0.25),
                    unlit: true,
                    ..default()
                }),
                visibility: Visibility::Hidden,
                ..default()
            }));

            tile_trees.insert(
                (terrain, view),
                tile_tree,
            );

            if let Some(surface) = river {
                root.spawn_spatial((
                    PbrBundle {
                        mesh: meshes.add(surface),
                        material: materials.add(StandardMaterial {
                            base_color: Color::srgb(0.12, 0.59, 0.72),
                            unlit: true,
                            ..default()
                        }),
                        ..default()
                    },
                    NotShadowCaster,
                    river_surface::RiverWater,
                ));
            }

            let ocean_mesh = meshes.add(Sphere::new(RADIUS as f32 + 1.1).mesh().uv(1024, 512));

            let ocean_material =
                ocean_materials.add(
                    OceanMaterial {
                        ocean_mask,
                    },
                );

            root.spawn_spatial(
                MaterialMeshBundle::<OceanMaterial> {
                    mesh: ocean_mesh,
                    material:
                        ocean_material,
                    ..default()
                },
            );
        },
    );
}
