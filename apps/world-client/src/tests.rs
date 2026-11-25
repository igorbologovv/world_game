use bevy::{
    camera::{primitives::Aabb, CameraProjection, RenderTargetInfo},
    diagnostic::DiagnosticsPlugin,
    gizmos::GizmoPlugin,
    gltf::GltfPlugin,
    image::{CompressedImageFormats, ImageLoader, ImagePlugin},
    input::{
        keyboard::{Key, KeyboardInput},
        mouse::MouseButtonInput,
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
    let region = spawn_region(std::iter::empty()).unwrap();
    install_world(&mut app, assets, region);
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
    assert_eq!((*camera, parent.parent()), (WorldCamera::FirstPerson, root));
    let parent = world
        .query_filtered::<&ChildOf, With<Explorer>>()
        .single(world)
        .unwrap();
    assert_eq!(parent.parent(), root);

    let spawn = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    let ground = world.resource::<SurfaceHeights>().sample(spawn).unwrap();
    assert_eq!(RADIUS, 3_389_500.0);
    assert!(ground > WATER_SURFACE_ELEVATION);
    assert_explorer_at(world, root, spawn, ground);
    let local_grid = world_core::construction::ConstructionGrid {
        origin: spawn,
        origin_cell: 500_000,
    };
    let planet_position = local_grid.position_at(500_002.5, 500_002.5, ground);
    let frame = world.get::<ReferenceFrame>(root).unwrap();
    let (cell, translation) = frame.translation_to_grid(planet_position);
    let restored = frame.grid_position_double(&cell, &Transform::from_translation(translation));
    assert!(restored.distance(planet_position) < 0.001);
    app.world_mut().write_message(KeyboardInput {
        key_code: KeyCode::KeyB,
        logical_key: Key::Character("b".into()),
        state: ButtonState::Pressed,
        window,
        text: None,
        repeat: false,
    });
    app.update();
    let world = app.world_mut();
    assert_eq!(
        *world.query::<&WorldCamera>().single(world).unwrap(),
        WorldCamera::Settlement
    );
    app.world_mut().write_message(KeyboardInput {
        key_code: KeyCode::KeyB,
        logical_key: Key::Character("b".into()),
        state: ButtonState::Released,
        window,
        text: None,
        repeat: false,
    });
    app.update();
    app.world_mut().write_message(KeyboardInput {
        key_code: KeyCode::KeyB,
        logical_key: Key::Character("b".into()),
        state: ButtonState::Pressed,
        window,
        text: None,
        repeat: false,
    });
    app.update();
    let world = app.world_mut();
    assert_eq!(
        *world.query::<&WorldCamera>().single(world).unwrap(),
        WorldCamera::FirstPerson
    );
    let sun = world
        .query::<(&Transform, &bevy::light::SunDisk)>()
        .iter(world)
        .find(|(_, disk)| disk.intensity > 0.0)
        .unwrap()
        .0;
    assert!(spawn.dot((sun.rotation * Vec3::Z).as_dvec3()) > 0.60);
    let size = UVec2::new(800, 600);
    {
        let mut cameras = world.query_filtered::<&mut Camera, With<WorldCamera>>();
        let mut view = cameras.single_mut(world).unwrap();
        view.computed.target_info = Some(RenderTargetInfo {
            physical_size: size,
            scale_factor: 1.0,
        });
        view.computed.clip_from_view = PerspectiveProjection::default().get_clip_from_view();
    }
    app.update();
    let world = app.world_mut();
    assert!(world
        .query::<(&Text, &Visibility)>()
        .iter(world)
        .any(|(text, visibility)| {
            text.0 == "Coconut palm" && *visibility == Visibility::Hidden
        }));
    // A resident palm in the active view drives the guide, independent of
    // asset streaming timing in this headless composition test.
    let view = *world
        .query_filtered::<&GlobalTransform, With<WorldCamera>>()
        .single(world)
        .unwrap();
    let position = view.translation() + *view.forward() * 70.0 - Vec3::Y * 5.4;
    world.spawn((
        world_vegetation::CoconutPalm,
        GlobalTransform::from_translation(position),
    ));
    app.update();
    assert!(app
        .world_mut()
        .query::<(&Text, &Visibility)>()
        .iter(app.world())
        .any(|(text, visibility)| {
            text.0 == "Coconut palm" && *visibility == Visibility::Inherited
        }));

    let world = app.world_mut();
    // The imported demonstration is an asset source, not a second terrain tile.
    assert_eq!(
        world
            .query::<&Name>()
            .iter(world)
            .filter(|name| name.as_str() == "Low Poly Tree Scene Free")
            .count(),
        0,
    );

    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::Escape,
            logical_key: Key::Escape,
            state,
            window,
            text: None,
            repeat: false,
        });
        app.update();
    }
    let world = app.world_mut();
    assert_eq!(
        *world.query::<&WorldCamera>().single(world).unwrap(),
        WorldCamera::Map
    );

    // Aim a headless map view at the old starter region, then use the normal input path.
    let target = terrain_aligned_direction(STARTER_REGION.spawn.direction);
    let target_ground = world.resource::<SurfaceHeights>().sample(target).unwrap();
    world
        .get_mut::<Window>(window)
        .unwrap()
        .set_cursor_position(Some(size.as_vec2() * 0.5));
    let (cell, translation) = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .translation_to_grid(target * (RADIUS * 3.2));
    let pose = Transform::from_translation(translation).looking_to(-target.as_vec3(), Vec3::Y);
    let mut cameras = world
        .query_filtered::<(GridTransform, &mut GlobalTransform, &mut Camera), With<WorldCamera>>();
    let (mut grid, mut global, mut camera) = cameras.single_mut(world).unwrap();
    *grid.cell = cell;
    *grid.transform = pose;
    *global = GlobalTransform::from(pose);
    camera.computed.target_info = Some(RenderTargetInfo {
        physical_size: size,
        scale_factor: 1.0,
    });
    camera.computed.clip_from_view = PerspectiveProjection::default().get_clip_from_view();

    world.write_message(KeyboardInput {
        key_code: KeyCode::ControlLeft,
        logical_key: Key::Control,
        state: ButtonState::Pressed,
        window,
        text: None,
        repeat: false,
    });
    world.write_message(MouseButtonInput {
        button: MouseButton::Left,
        state: ButtonState::Pressed,
        window,
    });
    app.update();
    assert_explorer_at(app.world_mut(), root, target, target_ground);

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

fn assert_explorer_at(world: &mut World, root: Entity, up: bevy::math::DVec3, ground: f64) {
    let (cell, transform) = {
        let mut explorers = world.query_filtered::<GridTransform, With<Explorer>>();
        let explorer = explorers.single_mut(world).unwrap();
        (*explorer.cell, *explorer.transform)
    };
    let position = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .grid_position_double(&cell, &transform);
    assert!(position.normalize().abs_diff_eq(up, 1e-6));
    assert!((position.length() - RADIUS - ground - 0.38).abs() < 0.02);
}

#[test]
fn spawn_argument_selects_a_known_region() {
    let parse = |args: &[&str]| spawn_region(args.iter().map(|arg| (*arg).to_owned()));
    assert_eq!(parse(&[]).unwrap().id, PALM_COAST_REGION.id);
    assert_eq!(
        parse(&["--spawn", "palm-coast"]).unwrap().id,
        PALM_COAST_REGION.id
    );
    assert_eq!(
        parse(&["--spawn", "starter-river"]).unwrap().id,
        STARTER_REGION.id
    );
    assert!(parse(&["--spawn", "unknown"]).is_none());
}

#[test]
fn client_assets_match_the_checkout_unless_packaged_or_overridden() {
    let workspace = Path::new(WORKSPACE_ROOT);
    let package = std::env::temp_dir().join(format!("world-client-package-{}", std::process::id()));
    let executable = package.join("world-client");
    let checkout = client_assets(workspace, Some(&executable), None);
    assert_eq!(checkout.directory(), workspace.join("assets"));
    assert!(checkout.directory().join(PALM_MESH).is_file());

    let packaged = package.join("assets");
    std::fs::create_dir_all(packaged.join("world")).unwrap();
    assert_eq!(
        client_assets(workspace, Some(&executable), None).directory(),
        packaged,
    );

    let override_root = package.join("override");
    let overridden = client_assets(workspace, Some(&executable), Some(&override_root));
    assert_eq!(overridden.directory(), override_root.join("assets"));
    std::fs::remove_dir_all(package).unwrap();
}

mod forest_render;

mod cloud_render;

mod terrain_render;
