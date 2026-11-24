//! Matched camera poses for inspecting planetary cloud rendering.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    math::DVec3,
    render::{
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{
            CachedPipelineState, PipelineCache, PipelineDescriptor, PollType, TextureFormat,
        },
        renderer::RenderDevice,
        view::screenshot::{save_to_disk, Screenshot},
        RenderApp,
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};

use super::*;

#[test]
#[ignore = "requires a Vulkan adapter and writes screenshots"]
fn cloud_altitude_sweep() {
    let output = std::env::var_os("CLOUD_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/cloud-sweep"));
    fs::create_dir_all(&output).unwrap();
    let assets = WorldAssets::new(Path::new(WORKSPACE_ROOT).join("assets"));
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
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
        1.0 / 60.0,
    )));
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
            1024,
            576,
            TextureFormat::Rgba8UnormSrgb,
            None,
        ));
    let camera_target = target.clone();
    app.add_systems(
        PostStartup,
        move |mut commands: Commands,
              cameras: Query<Entity, With<WorldCamera>>,
              mut hud: Query<&mut Node, Without<ChildOf>>| {
            commands.entity(cameras.single().unwrap()).insert((
                RenderTarget::Image(camera_target.clone().into()),
                bevy::ui::IsDefaultUiCamera,
            ));
            for mut root in &mut hud {
                root.display = Display::None;
            }
        },
    );
    render_frame(&mut app);
    let asset_server = app.world().resource::<AssetServer>();
    let shaders: [(&str, Handle<Shader>); 2] = [
        ("cloud", asset_server.load("shaders/planet_clouds.wesl")),
        ("ocean", asset_server.load("shaders/ocean.wesl")),
    ];

    // The shoreline is about 9.2 km from the Palm Coast spawn toward its look target.
    let shore_distance = 9_190.0;
    for (name, distance, altitude, tilt) in [
        ("01-orbit", 0.0, 11_000_000.0, -1.0),
        ("02-high-altitude", 0.0, 34_000.0, -0.55),
        ("03-above-clouds", 0.0, 5_500.0, -0.16),
        ("04-inside-cloud-layer", 0.0, 2_350.0, 0.015),
        ("05-below-clouds", 0.0, 150.0, 0.28),
        ("06-palm-coast", shore_distance, 45.0, -0.12),
    ] {
        set_camera(app.world_mut(), distance, altitude, tilt);
        for _ in 0..70 {
            render_frame(&mut app);
        }
        let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
        for (label, shader) in &shaders {
            let mut ready = false;
            for pipeline in cache.pipelines() {
                let PipelineDescriptor::RenderPipelineDescriptor(descriptor) = &pipeline.descriptor
                else {
                    continue;
                };
                if descriptor
                    .fragment
                    .as_ref()
                    .is_some_and(|stage| &stage.shader == shader)
                {
                    assert!(
                        !matches!(pipeline.state, CachedPipelineState::Err(_)),
                        "{name}: {label} pipeline failed: {:?}",
                        pipeline.state,
                    );
                    ready |= matches!(pipeline.state, CachedPipelineState::Ok(_));
                }
            }
            assert!(ready, "{name}: {label} pipeline never compiled");
        }

        let mut frame_times = [0.0; 16];
        for elapsed in &mut frame_times {
            let start = Instant::now();
            render_frame(&mut app);
            *elapsed = start.elapsed().as_secs_f64() * 1000.0;
        }
        frame_times.sort_by(f64::total_cmp);
        println!("{name} median render+GPU wait {:.2} ms", frame_times[8]);

        if distance == shore_distance {
            // Keep the waves at the same point in their animation for the two shoreline captures.
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        }
        capture(&mut app, &target, output.join(format!("{name}.png")));
    }

    let world = app.world_mut();
    let mut visibility = world.query::<(&Name, &mut Visibility)>();
    let (_, mut visibility) = visibility
        .iter_mut(world)
        .find(|(name, _)| name.as_str() == "Planetary clouds")
        .expect("expected a planetary cloud mesh");
    *visibility = Visibility::Hidden;
    capture(&mut app, &target, output.join("07-palm-coast-clear.png"));
}

fn capture(app: &mut App, target: &Handle<Image>, path: PathBuf) {
    if path.exists() {
        fs::remove_file(&path).unwrap();
    }
    app.world_mut()
        .spawn(Screenshot::image(target.clone()))
        .observe(save_to_disk(path.clone()));
    for _ in 0..12 {
        render_frame(app);
        if path.metadata().is_ok_and(|metadata| metadata.len() > 1_000) {
            return;
        }
    }
    panic!("screenshot was not saved: {path:?}");
}

fn set_camera(world: &mut World, distance: f64, altitude: f64, tilt: f64) {
    let spawn = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    let toward_coast = terrain_aligned_direction(PALM_COAST_REGION.spawn.look_at);
    let tangent = (toward_coast - spawn * toward_coast.dot(spawn)).normalize();
    let up = (spawn + tangent * (distance / RADIUS)).normalize();
    let tangent = (tangent - up * tangent.dot(up)).normalize();
    let forward: DVec3 = tangent * (1.0 - tilt * tilt).sqrt() + up * tilt;
    let screen_up = if tilt.abs() == 1.0 {
        Vec3::Y
    } else {
        up.as_vec3()
    };
    let ground = world.resource::<SurfaceHeights>().sample(up).unwrap();
    let height = altitude.max(ground + 12.0);
    let root = world.resource::<PlanetRoot>().entity();
    let (cell, translation) = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .translation_to_grid(up * (RADIUS + height));
    let mut cameras = world.query::<(GridTransform, &mut WorldCamera, &mut Projection)>();
    let (mut camera, mut mode, mut projection) = cameras.single_mut(world).unwrap();
    *camera.cell = cell;
    *camera.transform =
        Transform::from_translation(translation).looking_to(forward.as_vec3(), screen_up);
    *mode = WorldCamera::Map;

    let Projection::Perspective(projection) = &mut *projection else {
        panic!("expected a perspective camera");
    };
    projection.near = (height * 0.0005).max(0.05) as f32;
}

fn render_frame(app: &mut App) {
    app.update();
    assert!(app.should_exit().is_none(), "the render app requested exit");
    app.sub_app(RenderApp)
        .world()
        .resource::<RenderDevice>()
        .poll(PollType::wait_indefinitely())
        .unwrap();
}
