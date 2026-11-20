use bevy::{
    math::DVec3,
    pbr::MaterialPlugin,
    prelude::*,
    reflect::TypePath,
    render::{
        render_resource::*,
        texture::{
            ImageAddressMode, ImageFilterMode, ImageLoaderSettings, ImageSampler,
            ImageSamplerDescriptor,
        },
    },
};
use bevy_terrain::prelude::*;

const PATH: &str = "terrains/spherical";

const RADIUS: f64 = 6_371_000.0;

const MIN_HEIGHT: f32 = -12_000.0;
const MAX_HEIGHT: f32 = 9_000.0;

const TEXTURE_SIZE: u32 = 512;
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
        .add_systems(Startup, setup)
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
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
        border_size: 2,
        mip_level_count: 4,
        format: AttachmentFormat::R16,
    });

    let view_config =
        TerrainViewConfig::default();

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

            let view = root
                .spawn_spatial(
                    DebugCameraBundle::new(
                        -DVec3::X
                            * RADIUS
                            * 3.0,
                        RADIUS,
                        &frame,
                    ),
                )
                .id();

            tile_trees.insert(
                (terrain, view),
                tile_tree,
            );

            //
            // Ocean
            //
            // Это всё ещё одна сфера уровня моря.
            //
            // НО shader делает discard везде,
            // где ocean_mask == 0.
            //

let ocean_mesh =
    meshes.add(
        Sphere::new(
            RADIUS as f32 + 100_000.0
        )
        .mesh()
        .ico(6)
        .expect(
            "failed to create ocean sphere"
        ),
    );

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
