//! Matched camera poses for inspecting planetary cloud rendering.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    diagnostic::DiagnosticsStore,
    math::DVec3,
    render::{
        diagnostic::RenderDiagnosticsPlugin,
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
    let output = Path::new(WORKSPACE_ROOT).join(
        std::env::var_os("CLOUD_CAPTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target/cloud-volumetric")),
    );
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
    app.add_plugins(RenderDiagnosticsPlugin);
    install_world(&mut app, assets, PALM_COAST_REGION);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::yield_now();
    }
    app.finish();
    app.cleanup();

    let (width, height) = std::env::var("CLOUD_CAPTURE_SIZE")
        .ok()
        .map_or((1024, 576), |size| {
            let (width, height) = size.split_once('x').expect("use WIDTHxHEIGHT");
            (width.parse().unwrap(), height.parse().unwrap())
        });
    let target = app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            width,
            height,
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
    println!("Cloud sweep target: {width}x{height}");
    render_frame(&mut app);
    let case = std::env::var("CLOUD_CASE").ok();
    // The shoreline is about 9.2 km from the Palm Coast spawn toward its look target.
    let shore_distance = 9_190.0;
    for (name, distance, altitude, tilt) in [
        ("01-orbit", 0.0, 11_000_000.0, -1.0),
        ("02-high-altitude", 0.0, 34_000.0, -0.55),
        ("03-above-clouds", 0.0, 9_000.0, -0.16),
        ("04-inside-cloud-layer", 0.0, 2_350.0, 0.015),
        ("05-below-clouds", 0.0, 150.0, 0.28),
        ("06-palm-coast", shore_distance, 45.0, -0.12),
        ("09-backlit", shore_distance, 80.0, 0.0),
        ("09b-silver-lining", shore_distance, 80.0, 0.0),
        ("10-cloud-core", 16_000.0, 2_100.0, -0.08),
    ] {
        if case.as_ref().is_some_and(|case| !name.contains(case)) {
            continue;
        }
        set_camera(app.world_mut(), distance, altitude, tilt);
        match name {
            "09-backlit" => aim_near_sun(app.world_mut(), 0.0),
            "09b-silver-lining" => aim_near_sun(app.world_mut(), 164.0),
            _ => {}
        }
        for _ in 0..65 {
            render_frame(&mut app);
        }
        assert_cloud_pipelines(&app, true);
        for mode in ["volumetric", "legacy", "clear"] {
            let mut times = std::collections::BTreeMap::new();
            for _ in 0..10 {
                render_frame(&mut app);
            }
            let mut last_measurement: std::collections::BTreeMap<_, _> = app
                .world()
                .resource::<DiagnosticsStore>()
                .iter()
                .filter_map(|diagnostic| {
                    let path = diagnostic.path().as_str();
                    diagnostic
                        .measurement()
                        .map(|sample| (path.to_owned(), sample.time))
                })
                .collect();
            if mode == "legacy" {
                assert_cloud_pipelines(&app, false);
            }

            for _ in 0..24 {
                let start = Instant::now();
                render_frame(&mut app);
                times
                    .entry("render_plus_wait".to_owned())
                    .or_insert_with(Vec::new)
                    .push(start.elapsed().as_secs_f64() * 1000.0);
                for diagnostic in app.world().resource::<DiagnosticsStore>().iter() {
                    let path = diagnostic.path().as_str();
                    if path.ends_with("elapsed_gpu") {
                        if let Some(measurement) = diagnostic.measurement() {
                            if last_measurement.insert(path.to_owned(), measurement.time)
                                == Some(measurement.time)
                            {
                                continue;
                            }
                            times
                                .entry(path.to_owned())
                                .or_insert_with(Vec::new)
                                .push(measurement.value);
                        }
                    }
                }
            }
            let march = "render/cloud_ray_march/elapsed_gpu";
            assert_eq!(
                times.contains_key(march),
                mode == "volumetric",
                "{name} {mode}: expected GPU timestamp availability to match the renderer"
            );
            for (path, mut samples) in times {
                samples.sort_by(f64::total_cmp);
                println!("{name} {mode} {path} {:.3} ms", samples[samples.len() / 2]);
            }
            capture(&mut app, &target, output.join(format!("{name}-{mode}.png")));
            toggle_renderer(&mut app);
        }
    }

    if case.is_none() {
        // Move through the floating-origin grid at flight altitude and capture
        // the first frame after a discontinuous move into the cloud layer.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )));
        for step in 0..48 {
            let distance = f64::from(step) * 75.0;
            set_camera(app.world_mut(), distance, 9_000.0 - distance * 0.38, -0.16);
            render_frame(&mut app);
            if step == 22 || step == 47 {
                capture(
                    &mut app,
                    &target,
                    output.join(format!("07-flyover-{step}.png")),
                );
            }
        }
        set_camera(app.world_mut(), 16_000.0, 2_100.0, -0.08);
        capture(&mut app, &target, output.join("08-camera-cut.png"));
    }

    if case.as_deref().is_none_or(|name| name == "passage") {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )));
        set_camera(app.world_mut(), 11_000.0, 2_100.0, -0.08);
        for _ in 0..30 {
            render_frame(&mut app);
        }
        for step in 0..=120 {
            let distance = 11_000.0 + f64::from(step) * 100.0;
            set_camera(app.world_mut(), distance, 2_100.0, -0.08);
            if step % 15 == 0 {
                capture(
                    &mut app,
                    &target,
                    output.join(format!("11-cloud-passage-{step:03}.png")),
                );
            } else {
                render_frame(&mut app);
            }
        }
    }
}

fn toggle_renderer(app: &mut App) {
    for state in [ButtonState::Pressed, ButtonState::Released] {
        app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::F6,
            logical_key: Key::F6,
            state,
            window: Entity::PLACEHOLDER,
            text: None,
            repeat: false,
        });
        render_frame(app);
    }
}

fn assert_cloud_pipelines(app: &App, experimental: bool) {
    let labels: &[&str] = if experimental {
        &["march_clouds", "resolve_clouds", "upscale_clouds"]
    } else {
        &["legacy cloud"]
    };
    let legacy: Handle<Shader> = app
        .world()
        .resource::<AssetServer>()
        .load("shaders/planet_clouds.wesl");
    let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
    for label in labels {
        let mut ready = false;
        for pipeline in cache.pipelines() {
            let PipelineDescriptor::RenderPipelineDescriptor(descriptor) = &pipeline.descriptor
            else {
                continue;
            };
            let matched = if experimental {
                descriptor.label.as_deref() == Some(*label)
            } else {
                descriptor
                    .fragment
                    .as_ref()
                    .is_some_and(|stage| stage.shader == legacy)
            };
            if matched {
                assert!(
                    !matches!(pipeline.state, CachedPipelineState::Err(_)),
                    "{label} pipeline failed: {:?}",
                    pipeline.state,
                );
                ready |= matches!(pipeline.state, CachedPipelineState::Ok(_));
            }
        }
        assert!(ready, "{label} pipeline never compiled");
    }
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

fn aim_near_sun(world: &mut World, offset: f64) {
    let sun = world
        .query_filtered::<&Transform, With<bevy::light::SunDisk>>()
        .single(world)
        .unwrap()
        .back()
        .as_vec3();
    let root = world.resource::<PlanetRoot>().entity();
    let grid = world.get::<ReferenceFrame>(root).unwrap().clone();
    let mut cameras = world.query_filtered::<GridTransform, With<WorldCamera>>();
    let mut camera = cameras.single_mut(world).unwrap();
    let eye = grid.grid_position_double(&camera.cell, &camera.transform);
    let up = eye.normalize();
    let away_from_sun = (up * sun.as_dvec3().dot(up) - sun.as_dvec3()).normalize();
    let radial = (eye + offset * away_from_sun).normalize();
    let (cell, translation) = grid.translation_to_grid(radial * eye.length());
    let up = radial.as_vec3();
    let aim = (sun - up * 0.19).normalize();
    *camera.cell = cell;
    *camera.transform = Transform::from_translation(translation).looking_to(aim, up);
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
