//! Offscreen altitude sweep and steady-state frame timing on the active adapter.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use bevy::time::TimeUpdateStrategy;

use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    math::DVec3,
    render::{
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{PollType, TextureFormat},
        renderer::RenderDevice,
        view::screenshot::{save_to_disk, Screenshot},
        RenderApp,
    },
    ui::IsDefaultUiCamera,
    window::ExitCondition,
    winit::WinitPlugin,
};

use world_core::{canopy::ForestCanopy, runtime::GridCell};

use super::*;

#[test]
#[ignore = "requires a GPU adapter and writes screenshots"]
fn forest_altitude_sweep() {
    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/forest-sweep"));
    fs::create_dir_all(&output).unwrap();
    let (mut app, target) = render_app();
    let world = app.world_mut();
    let mut visibility = world.query::<(&Name, &mut Visibility)>();
    let (_, mut clouds) = visibility
        .iter_mut(world)
        .find(|(name, _)| name.as_str() == "Planetary clouds")
        .expect("expected a planetary cloud mesh");
    *clouds = Visibility::Hidden;
    let mut hud = world.query_filtered::<&mut Node, Without<ChildOf>>();
    for mut root in hud.iter_mut(world) {
        root.display = Display::None;
    }
    let coast = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    // This inland direction looks over a boreal river valley and broken ridges.
    let woodland = DVec3::new(
        -0.36285174347169147,
        0.8249897523299634,
        0.43327880263186114,
    );
    for (label, up, height, tilt, warmup) in [
        ("ground", coast, 2.2, 0.025, 220),
        ("grazing", woodland, 320.0, 0.16, 120),
        ("handoff", woodland, 1_700.0, 0.78, 75),
        ("aerial", woodland, 24_000.0, 0.87, 65),
        ("orbit", woodland, 1_450_000.0, 1.0, 90),
    ] {
        set_camera(app.world_mut(), up, height, tilt);
        for _ in 0..warmup {
            render_frame(&mut app);
        }
        measure(&mut app, &target, &output, label);
    }
    let layout = app.world().resource::<ForestCanopy>().layout;
    assert_eq!(layout.fade.z, 0.0, "the 1024² faces must finish uploading");
    assert_eq!(
        layout.fade.w, 1.0,
        "the orbital canopy must finish fading in"
    );

    // Compare the same woodland with impostor geometry and terrain canopy.
    // Changing only the forest range keeps the camera, air path and lighting fixed.
    set_camera(app.world_mut(), woodland, 1_250.0, 0.94);
    let settings = *app
        .world()
        .resource::<world_vegetation::VegetationSettings>();
    for (label, forest_range) in [
        ("reference-impostors", 3_000.0),
        ("reference-canopy", 1_200.0),
    ] {
        app.insert_resource(world_vegetation::VegetationSettings {
            forest_range,
            ..settings
        });
        for _ in 0..180 {
            render_frame(&mut app);
        }
        capture(&mut app, &target, &output, label);
    }
}

#[test]
#[ignore = "requires a GPU adapter and writes screenshots"]
fn tree_lod_sweep() {
    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/tree-lod-sweep"));
    fs::create_dir_all(&output).unwrap();
    let (mut app, target) = render_app();
    for _ in 0..300 {
        render_frame(&mut app);
    }
    let world = app.world_mut();
    let root = world.resource::<PlanetRoot>().entity();
    let origin = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction) * RADIUS;
    let mut palms = world.query_filtered::<GridTransform, With<world_vegetation::CoconutPalm>>();
    let positions: Vec<_> = palms
        .iter(world)
        .map(|grid| (*grid.cell, *grid.transform))
        .collect();
    let frame = world.get::<ReferenceFrame>(root).unwrap();
    let tree = positions
        .iter()
        .map(|(cell, transform)| frame.grid_position_double(cell, transform))
        .min_by(|a, b| {
            a.distance_squared(origin)
                .total_cmp(&b.distance_squared(origin))
        })
        .expect("the Palm Coast fixture must stream a palm");
    println!("Tracked palm at {tree:?}");
    let up = tree.normalize();
    let tangent = DVec3::Y.cross(up).normalize();
    let focus = tree + up * 6.5;
    for (label, distance, azimuth, elevation) in [
        ("mesh-close", 17.0, 0.0_f64, 0.06_f64),
        ("mesh-mid", 33.0, 0.0, 0.06),
        ("mesh-far", 51.0, 0.0, 0.06),
        ("handoff", 62.0, 0.0, 0.06),
        ("impostor", 81.0, 0.0, 0.06),
        ("impostor-side", 81.0, 1.4, 0.06),
        ("impostor-above", 81.0, 1.4, 0.8),
        ("forest", 330.0, 1.4, 0.45),
    ] {
        let horizontal = tangent * azimuth.cos() + up.cross(tangent) * azimuth.sin();
        let eye = focus + distance * (horizontal * elevation.cos() + up * elevation.sin());
        aim_at_tree(app.world_mut(), eye, focus, up);
        for _ in 0..75 {
            render_frame(&mut app);
        }
        measure(&mut app, &target, &output, label);
    }

    // From the spawn clearing, keep foreground trees out of this comparison.
    // Hold the view fixed and force the representation on either side of the fade.
    let view = (origin - tree - up * (origin - tree).dot(up)).normalize();
    let position = |distance| focus + distance * (view * 0.997 + up * 0.077);
    aim_at_tree(app.world_mut(), position(43.0), focus, up);
    let defaults = *app
        .world()
        .resource::<world_vegetation::VegetationSettings>();
    for (label, mesh, band) in [
        ("paired-lod0", [180.0, 210.0], [165.0, 205.0]),
        ("paired-lod1", [10.0, 180.0], [165.0, 205.0]),
        ("paired-lod2", [10.0, 20.0], [165.0, 205.0]),
        ("paired-impostor", defaults.mesh_distances, [16.0, 20.0]),
    ] {
        app.insert_resource(world_vegetation::VegetationSettings {
            mesh_distances: mesh,
            impostor_blend: band,
            ..defaults
        });
        for _ in 0..75 {
            render_frame(&mut app);
        }
        capture(&mut app, &target, &output, label);
    }
    app.insert_resource(defaults);
    for _ in 0..75 {
        render_frame(&mut app);
    }
    for frame in 0..=100 {
        let distance = 12.0 + 0.66 * f64::from(frame);
        aim_at_tree(app.world_mut(), position(distance), focus, up);
        render_frame(&mut app);
        if frame % 10 == 0 {
            capture(&mut app, &target, &output, &format!("dolly-{frame:03}"));
        }
    }
}

#[test]
#[ignore = "requires a GPU adapter and writes screenshots"]
fn broadleaf_lighting_sweep() {
    use bevy::gltf::{Gltf, GltfMesh};
    use bevy::render::render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor};

    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/broadleaf-lighting"));
    fs::create_dir_all(&output).unwrap();
    let (mut app, target) = render_app();
    let asset = app
        .world()
        .resource::<AssetServer>()
        .load::<Gltf>("models/vegetation/tree_part9/tree_lod0.glb");
    let settings = *app
        .world()
        .resource::<world_vegetation::VegetationSettings>();
    let mesh_settings = world_vegetation::VegetationSettings {
        mesh_distances: [180.0, 210.0],
        impostor_blend: [165.0, 205.0],
        ..settings
    };
    app.insert_resource(mesh_settings);
    for _ in 0..360 {
        render_frame(&mut app);
    }
    let coast = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    let world = app.world_mut();
    let gltf = world.resource::<Assets<Gltf>>().get(&asset).unwrap();
    let leaves = &gltf.named_materials["Leaves"];
    let leaf_mesh = gltf
        .meshes
        .iter()
        .filter_map(|mesh| world.resource::<Assets<GltfMesh>>().get(mesh))
        .flat_map(|mesh| &mesh.primitives)
        .find(|primitive| primitive.material.as_ref() == Some(leaves))
        .unwrap()
        .mesh
        .clone();
    let mut parents = world.query::<(&Mesh3d, &ChildOf)>();
    let trees: Vec<_> = parents
        .iter(world)
        .filter(|(mesh, _)| mesh.0 == leaf_mesh)
        .map(|(_, parent)| parent.parent())
        .collect();
    let root = world.resource::<PlanetRoot>().entity();
    let frame = world.get::<ReferenceFrame>(root).unwrap();
    let tree = trees
        .into_iter()
        .map(|entity| {
            frame.grid_position_double(
                world.get::<GridCell>(entity).unwrap(),
                world.get::<Transform>(entity).unwrap(),
            )
        })
        .min_by(|a, b| {
            a.distance_squared(coast * RADIUS)
                .total_cmp(&b.distance_squared(coast * RADIUS))
        })
        .expect("Palm Coast must stream the authored tropical broadleaf");
    println!("Tracked broadleaf at {tree:?}");
    let up = tree.normalize();
    let sun = world
        .query_filtered::<&Transform, With<DirectionalLight>>()
        .single(world)
        .unwrap()
        .rotation
        * Vec3::Z;
    let sun = sun.as_dvec3();
    let toward_sun = (sun - up * sun.dot(up)).normalize();
    let focus = tree + up * 13.0;
    for (side, direction) in [("sunward", toward_sun), ("leeward", -toward_sun)] {
        aim_at_tree(
            app.world_mut(),
            focus + direction * 52.0 + up * 4.0,
            focus,
            up,
        );
        for (representation, lods) in [
            ("mesh", mesh_settings),
            (
                "impostor",
                world_vegetation::VegetationSettings {
                    impostor_blend: [16.0, 20.0],
                    ..settings
                },
            ),
        ] {
            app.insert_resource(lods);
            for _ in 0..120 {
                render_frame(&mut app);
            }
            capture(
                &mut app,
                &target,
                &output,
                &format!("{side}-{representation}"),
            );
        }
    }
    let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
    let assets = app.world().resource::<AssetServer>();
    for path in [
        "shaders/vegetation_fade.wesl",
        "shaders/vegetation_proxy.wesl",
    ] {
        let shader = assets.load::<Shader>(path);
        let mut ready = false;
        for pipeline in cache.pipelines() {
            let PipelineDescriptor::RenderPipelineDescriptor(descriptor) = &pipeline.descriptor
            else {
                continue;
            };
            if descriptor
                .fragment
                .as_ref()
                .is_some_and(|stage| stage.shader == shader)
            {
                assert!(
                    !matches!(pipeline.state, CachedPipelineState::Err(_)),
                    "{path}: {:?}",
                    pipeline.state
                );
                ready |= matches!(pipeline.state, CachedPipelineState::Ok(_));
            }
        }
        assert!(ready, "{path} did not compile");
    }
}

fn aim_at_tree(world: &mut World, eye: DVec3, focus: DVec3, up: DVec3) {
    let root = world.resource::<PlanetRoot>().entity();
    let (cell, translation) = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .translation_to_grid(eye);
    let mut cameras = world.query::<(GridTransform, &mut WorldCamera)>();
    let (mut camera, mut mode) = cameras.single_mut(world).unwrap();
    *camera.cell = cell;
    *camera.transform =
        Transform::from_translation(translation).looking_to((focus - eye).as_vec3(), up.as_vec3());
    *mode = WorldCamera::Map;
}

fn render_app() -> (App, Handle<Image>) {
    let override_root = std::env::var_os("BEVY_ASSET_ROOT").map(PathBuf::from);
    let assets = client_assets(Path::new(WORKSPACE_ROOT), None, override_root.as_deref());
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<TransformPlugin>()
            .disable::<WinitPlugin>()
            .disable::<PipelinedRenderingPlugin>()
            .set(asset_plugin(&assets))
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            }),
    );
    install_world(&mut app, assets, PALM_COAST_REGION);
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::yield_now();
    }
    app.finish();
    app.cleanup();
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            832,
            468,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    let camera_target = target.clone();
    app.add_systems(
        PostStartup,
        move |mut commands: Commands, camera: Query<Entity, With<WorldCamera>>| {
            commands.entity(camera.single().unwrap()).insert((
                RenderTarget::Image(camera_target.clone().into()),
                IsDefaultUiCamera,
            ));
        },
    );
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
    render_frame(&mut app);
    (app, target)
}

fn measure(app: &mut App, target: &Handle<Image>, output: &std::path::Path, label: &str) {
    let mut samples: Vec<_> = (0..36)
        .map(|_| {
            let start = Instant::now();
            render_frame(app);
            start.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    samples.sort_by(f64::total_cmp);
    println!(
        "{label} median={:.2} ms p95={:.2} ms",
        samples[samples.len() / 2],
        samples[samples.len() * 95 / 100]
    );

    capture(app, target, output, label);
}

fn capture(app: &mut App, target: &Handle<Image>, output: &std::path::Path, label: &str) {
    let path = output.join(format!("{label}.png"));
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    app.world_mut()
        .spawn(Screenshot::image(target.clone()))
        .observe(save_to_disk(path.clone()));
    for _ in 0..15 {
        render_frame(app);
        if path.metadata().is_ok_and(|info| info.len() > 1_000) {
            break;
        }
    }
    assert!(path.metadata().unwrap().len() > 1_000, "{path:?}");
}

fn set_camera(world: &mut World, up: DVec3, height: f64, tilt: f64) {
    let ground = world.resource::<SurfaceHeights>().sample(up).unwrap();
    let root = world.resource::<PlanetRoot>().entity();
    let (cell, translation) = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .translation_to_grid(up * (RADIUS + ground + height));
    let target = terrain_aligned_direction(PALM_COAST_REGION.spawn.look_at);
    let tangent = (target - up * target.dot(up)).normalize();
    let forward = tangent * (1.0 - tilt * tilt).sqrt() - up * tilt;
    let screen_up = if tilt < 1.0 { up.as_vec3() } else { Vec3::Y };
    let mut cameras = world.query::<(GridTransform, &mut WorldCamera, &mut Projection)>();
    let (mut camera, mut mode, mut projection) = cameras.single_mut(world).unwrap();
    *camera.cell = cell;
    *camera.transform =
        Transform::from_translation(translation).looking_to(forward.as_vec3(), screen_up);
    *mode = if height < 5.0 {
        WorldCamera::FirstPerson
    } else {
        WorldCamera::Map
    };
    let Projection::Perspective(camera) = &mut *projection else {
        panic!("expected a perspective camera");
    };
    camera.near = (height * 0.005).max(0.05) as f32;
    camera.far = (RADIUS * 20.0) as f32;
}

fn render_frame(app: &mut App) {
    app.update();
    app.sub_app(RenderApp)
        .world()
        .resource::<RenderDevice>()
        .poll(PollType::wait_indefinitely())
        .unwrap();
}
