use bevy::{
    camera::primitives::Aabb,
    diagnostic::DiagnosticsPlugin,
    gizmos::GizmoPlugin,
    gltf::GltfPlugin,
    image::{CompressedImageFormats, ImageLoader, ImagePlugin},
    input::{
        keyboard::{Key, KeyboardInput},
        ButtonState, InputPlugin,
    },
    light::atmosphere::ScatteringMedium,
    mesh::MeshPlugin,
    scene::ScenePlugin,
    shader::Shader,
    window::PrimaryWindow,
    world_serialization::WorldSerializationPlugin,
};
use world_core::{
    coordinates::terrain_aligned_direction,
    planet::{RADIUS, WATER_SURFACE_ELEVATION},
    runtime::{Explorer, GridTransform, PlanetRoot, ReferenceFrame, WorldCamera},
    SurfaceHeights,
};

use super::*;

#[test]
fn client_plugins_share_the_planet_and_starting_camera() {
    let assets = WorldAssets::new(Path::new(WORKSPACE_ROOT).join("assets"));
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        asset_plugin(&assets),
        WorldSerializationPlugin,
        ScenePlugin,
        ImagePlugin::default(),
        MeshPlugin,
        GltfPlugin::default(),
        InputPlugin,
        DiagnosticsPlugin,
    ))
    .init_asset::<StandardMaterial>()
    .init_asset::<Shader>()
    .init_asset::<ScatteringMedium>()
    .register_asset_loader(ImageLoader::new(CompressedImageFormats::empty()))
    // GLB scene cloning also runs in this headless app.
    .register_type::<ChildOf>()
    .register_type::<Children>()
    .register_type::<Aabb>()
    .register_type::<Visibility>()
    .register_type::<InheritedVisibility>()
    .register_type::<ViewVisibility>()
    .add_plugins(GizmoPlugin);
    install_world(&mut app, assets);
    let window = app
        .world_mut()
        .spawn((Window::default(), PrimaryWindow))
        .id();

    app.finish();
    app.cleanup();
    app.update();

    let world = app.world_mut();
    let root = world.resource::<PlanetRoot>().entity();
    assert!(world.get::<ReferenceFrame>(root).is_some());
    let (camera, parent) = world
        .query::<(&WorldCamera, &ChildOf)>()
        .single(world)
        .unwrap();
    assert_eq!((*camera, parent.parent()), (WorldCamera::Map, root));
    let parent = world
        .query_filtered::<&ChildOf, With<Explorer>>()
        .single(world)
        .unwrap();
    assert_eq!(parent.parent(), root);

    let spawn = terrain_aligned_direction(STARTER_REGION.spawn.direction);
    let ground = world.resource::<SurfaceHeights>().sample(spawn).unwrap();
    assert!(ground > WATER_SURFACE_ELEVATION);
    let (cell, transform) = {
        let mut explorers = world.query_filtered::<GridTransform, With<Explorer>>();
        let explorer = explorers.single_mut(world).unwrap();
        (*explorer.cell, *explorer.transform)
    };
    let position = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .grid_position_double(&cell, &transform);
    assert!(position.normalize().abs_diff_eq(spawn, 1e-6));
    assert!((position.length() - RADIUS - ground - 0.38).abs() < 0.02);

    // The imported demonstration is an asset source, not a second terrain tile.
    assert_eq!(
        world
            .query::<&Name>()
            .iter(world)
            .filter(|name| name.as_str() == "Low Poly Tree Scene Free")
            .count(),
        0,
    );

    for (key_code, logical_key, expected) in [
        (
            KeyCode::KeyV,
            Key::Character("v".into()),
            WorldCamera::FirstPerson,
        ),
        (KeyCode::Escape, Key::Escape, WorldCamera::Map),
        (KeyCode::KeyT, Key::Character("t".into()), WorldCamera::Free),
    ] {
        app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state: ButtonState::Pressed,
            window,
            text: None,
            repeat: false,
        });
        app.update();
        let world = app.world_mut();
        assert_eq!(
            *world.query::<&WorldCamera>().single(world).unwrap(),
            expected
        );
    }
}
