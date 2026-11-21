use bevy::{
    diagnostic::DiagnosticsPlugin,
    gizmos::GizmoPlugin,
    gltf::GltfPlugin,
    input::{
        keyboard::{Key, KeyboardInput},
        ButtonState, InputPlugin,
    },
    render::{primitives::Aabb, render_resource::Shader, texture::ImagePlugin},
    scene::ScenePlugin,
    window::PrimaryWindow,
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
        ScenePlugin,
        ImagePlugin::default(),
        GltfPlugin::default(),
        InputPlugin,
        DiagnosticsPlugin,
    ))
    .init_asset::<Mesh>()
    .init_asset::<StandardMaterial>()
    .init_asset::<Shader>()
    // GLB scene cloning also runs in this headless app.
    .register_type::<Parent>()
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
    let (camera, parent) = world.query::<(&WorldCamera, &Parent)>().single(world);
    assert_eq!((*camera, parent.get()), (WorldCamera::Map, root));
    let parent = world
        .query_filtered::<&Parent, With<Explorer>>()
        .single(world);
    assert_eq!(parent.get(), root);

    let spawn = terrain_aligned_direction(STARTER_REGION.spawn.direction);
    let ground = world.resource::<SurfaceHeights>().sample(spawn).unwrap();
    assert!(ground > WATER_SURFACE_ELEVATION);
    let (cell, transform) = {
        let mut explorers = world.query_filtered::<GridTransform, With<Explorer>>();
        let explorer = explorers.single_mut(world);
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
        world.query::<&Name>().iter(world)
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
