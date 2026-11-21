use bevy::{
    diagnostic::DiagnosticsPlugin,
    gizmos::GizmoPlugin,
    gltf::GltfPlugin,
    input::{
        keyboard::{Key, KeyboardInput},
        ButtonState, InputPlugin,
    },
    render::{render_resource::Shader, texture::ImagePlugin},
    scene::ScenePlugin,
    window::PrimaryWindow,
};
use world_core::{
    coordinates::terrain_aligned_direction,
    runtime::{Explorer, PlanetRoot, ReferenceFrame, WorldCamera},
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
        ScenePlugin,
        ImagePlugin::default(),
        GltfPlugin::default(),
        InputPlugin,
        DiagnosticsPlugin,
    ))
    .init_asset::<Mesh>()
    .init_asset::<StandardMaterial>()
    .init_asset::<Shader>()
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
    let (camera, parent) = world.query::<(&WorldCamera, &Parent)>().single(world);
    assert_eq!((*camera, parent.get()), (WorldCamera::Map, root));
    let parent = world
        .query_filtered::<&Parent, With<Explorer>>()
        .single(world);
    assert_eq!(parent.get(), root);

    let spawn = terrain_aligned_direction(STARTER_REGION.spawn.direction);
    let ground = world.resource::<SurfaceHeights>().sample(spawn).unwrap();
    assert!((ground - STARTER_REGION.spawn.elevation).abs() < 10.0);

    for (key_code, logical_key, expected) in [
        (
            KeyCode::KeyV,
            Key::Character("v".into()),
            WorldCamera::FirstPerson,
        ),
        (KeyCode::Escape, Key::Escape, WorldCamera::Map),
        (KeyCode::KeyT, Key::Character("t".into()), WorldCamera::Free),
    ] {
        app.world_mut().send_event(KeyboardInput {
            key_code,
            logical_key,
            state: ButtonState::Pressed,
            window,
        });
        app.update();
        let world = app.world_mut();
        assert_eq!(*world.query::<&WorldCamera>().single(world), expected);
    }
}
