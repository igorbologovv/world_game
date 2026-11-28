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
    diagnostic::{DiagnosticPath, DiagnosticsStore},
    math::DVec3,
    render::{
        diagnostic::RenderDiagnosticsPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{PollType, TextureFormat},
        renderer::RenderDevice,
        view::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured},
        RenderApp,
    },
    ui::IsDefaultUiCamera,
    window::ExitCondition,
    winit::WinitPlugin,
};

use world_core::canopy::ForestCanopy;
use world_vegetation::ForestRenderStats;

use super::*;

#[test]
#[ignore = "requires a GPU adapter and writes gameplay screenshots"]
fn tree_library_pilot() {
    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/tree-library-pilot"));
    fs::create_dir_all(&output).unwrap();
    let (mut app, target) = render_app();
    for mut hud in app
        .world_mut()
        .query_filtered::<&mut Node, Without<ChildOf>>()
        .iter_mut(app.world_mut())
    {
        hud.display = Display::None;
    }
    for (species, direction, height) in [
        (
            "norway-spruce",
            DVec3::new(-0.6533087936, 0.7499886686, 0.1034631195),
            20.0,
        ),
        (
            "european-beech",
            DVec3::new(-0.4677238208, 0.7499886686, -0.4677086960),
            16.0,
        ),
        (
            "umbrella-thorn",
            DVec3::new(-0.9918296980, 0.0499786832, 0.1173711264),
            10.8,
        ),
    ] {
        let up = direction.normalize();
        set_camera(app.world_mut(), up, 2.5, 0.0);
        wait_for_forest(&mut app);
        for (label, distance) in [("detail", height * 1.7), ("distance", height * 8.0)] {
            set_camera(app.world_mut(), up, distance, 0.85);
            for _ in 0..80 {
                render_frame(&mut app);
            }
            measure(&mut app, &target, &output, &format!("{species}-{label}"));
        }
    }
}

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
        if label == "ground" {
            wait_for_forest(&mut app);
        }
        for _ in 0..warmup {
            render_frame(&mut app);
        }
        measure(&mut app, &target, &output, label);
        match label {
            "ground" | "grazing" | "handoff" => assert_forest_drawn(&app),
            "aerial" | "orbit" => {
                let stats = app.world().resource::<ForestRenderStats>();
                assert_eq!(stats.triangles(), 0, "{label} uses the terrain canopy");
            }
            _ => unreachable!(),
        }
    }
    let layout = app.world().resource::<ForestCanopy>().layout;
    assert_eq!(layout.fade.z, 0.0, "the 1024² faces must finish uploading");
    assert_eq!(
        layout.fade.w, 1.0,
        "the orbital canopy must finish fading in"
    );

    // Compare the same woodland with hybrid meshes and terrain canopy.
    // Changing only the forest range keeps the camera, air path and lighting fixed.
    set_camera(app.world_mut(), woodland, 1_250.0, 0.94);
    let settings = *app
        .world()
        .resource::<world_vegetation::VegetationSettings>();
    for (label, forest_range, expect_meshes) in [
        ("reference-hybrid-meshes", 3_000.0, true),
        ("reference-canopy", 1_000.0, false),
    ] {
        app.insert_resource(world_vegetation::VegetationSettings {
            forest_range,
            ..settings
        });
        for _ in 0..180 {
            render_frame(&mut app);
        }
        capture(&mut app, &target, &output, label);
        let stats = app.world().resource::<ForestRenderStats>();
        assert_eq!(stats.triangles() > 0, expect_meshes, "{label}");
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
    wait_for_forest(&mut app);
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
        ("mesh-near-oblique", 33.0, 0.0, 0.06),
        ("mesh-near-distance", 51.0, 0.0, 0.06),
        ("mesh-near-edge", 62.0, 0.0, 0.06),
        ("mesh-middle", 165.0, 0.0, 0.06),
        ("mesh-middle-side", 165.0, 1.4, 0.06),
        ("mesh-middle-above", 165.0, 1.4, 0.8),
        ("mesh-far", 490.0, 1.4, 0.45),
    ] {
        let horizontal = tangent * azimuth.cos() + up.cross(tangent) * azimuth.sin();
        let eye = focus + distance * (horizontal * elevation.cos() + up * elevation.sin());
        aim_at_tree(app.world_mut(), eye, focus, up);
        for _ in 0..75 {
            render_frame(&mut app);
        }
        measure(&mut app, &target, &output, label);
    }

    let close_eye = focus + tangent * 17.0 + up;
    aim_at_tree(app.world_mut(), close_eye, focus, up);
    let world = app.world_mut();
    let camera = world
        .query_filtered::<Entity, With<WorldCamera>>()
        .single(world)
        .unwrap();
    let samples = *world.get::<Msaa>(camera).unwrap();
    for msaa in [Msaa::Off, samples] {
        app.world_mut().entity_mut(camera).insert(msaa);
        verify_shared_depth(&mut app, &target, close_eye, focus, up);
    }

    // Fly away from the same palm and back through both transition bands.
    let view = (origin - tree - up * (origin - tree).dot(up)).normalize();
    let position = |distance| focus + distance * (view * 0.97 + up * 0.24);
    aim_at_tree(app.world_mut(), position(30.0), focus, up);
    for _ in 0..50 {
        render_frame(&mut app);
    }
    let mut seen_levels = [false; 3];
    let mut seen_transitions = [false; 2];
    for frame in 0..=240 {
        let distance = 30.0 + 6.1 * f64::from(frame.min(240 - frame));
        aim_at_tree(app.world_mut(), position(distance), focus, up);
        render_frame(&mut app);
        let stats = app.world().resource::<ForestRenderStats>();
        for (seen, count) in seen_levels.iter_mut().zip(stats.levels()) {
            *seen |= count != 0;
        }
        for (seen, count) in seen_transitions.iter_mut().zip(stats.transitions()) {
            *seen |= count != 0;
        }
        if frame % 40 == 0 {
            capture(&mut app, &target, &output, &format!("dolly-{frame:03}"));
        }
    }
    assert_eq!(seen_levels, [true; 3], "Igor must draw all three LODs");
    assert_eq!(
        seen_transitions, [true; 2],
        "Igor must traverse both transition bands"
    );
    assert_forest_drawn(&app);
}

#[test]
#[ignore = "requires a GPU adapter and writes screenshots"]
fn broadleaf_lighting_sweep() {
    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/broadleaf-lighting"));
    fs::create_dir_all(&output).unwrap();
    let (mut app, target) = render_app();
    let up = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    set_camera(app.world_mut(), up, 35.0, 0.85);
    wait_for_forest(&mut app);
    let ground = app.world().resource::<SurfaceHeights>().sample(up).unwrap();
    let focus = up * (RADIUS + ground + 6.0);
    let world = app.world_mut();
    let sun = world
        .query::<(&DirectionalLight, &GlobalTransform)>()
        .iter(world)
        .max_by(|(a, _), (b, _)| a.illuminance.total_cmp(&b.illuminance))
        .map(|(_, transform)| transform.back().as_dvec3())
        .expect("terrain supplies a directional sun");
    let toward_sun = (sun - up * sun.dot(up)).normalize();
    for (side, direction) in [("sunward", toward_sun), ("leeward", -toward_sun)] {
        for (label, distance) in [("near", 52.0), ("distance", 250.0)] {
            aim_at_tree(
                app.world_mut(),
                focus + direction * distance + up * 20.0,
                focus,
                up,
            );
            for _ in 0..80 {
                render_frame(&mut app);
            }
            assert_forest_drawn(&app);
            capture(&mut app, &target, &output, &format!("{side}-{label}"));
        }
    }
}

fn verify_shared_depth(app: &mut App, target: &Handle<Image>, eye: DVec3, focus: DVec3, up: DVec3) {
    render_frame(app);
    let world = app.world_mut();
    let forward = (focus - eye).normalize().as_vec3();
    let camera = world
        .query_filtered::<&GlobalTransform, With<WorldCamera>>()
        .single(world)
        .unwrap();
    let translation = camera.translation() + forward * 2.0;
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::new(10.0, 10.0, 0.2));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.0, 1.0),
            unlit: true,
            ..default()
        });
    let occluder = world
        .spawn((
            Transform::from_translation(translation).looking_to(forward, up.as_vec3()),
            Mesh3d(mesh),
            MeshMaterial3d(material),
        ))
        .id();
    for _ in 0..24 {
        render_frame(app);
    }
    assert_forest_drawn(app);
    let (tx, rx) = std::sync::mpsc::channel();
    app.world_mut()
        .spawn(Screenshot::image(target.clone()))
        .observe(move |capture: On<ScreenshotCaptured>| {
            tx.send(capture.image.clone()).unwrap();
        });
    let image = (0..20)
        .find_map(|_| {
            render_frame(app);
            rx.try_recv().ok()
        })
        .expect("shared depth screenshot");
    let center = UVec2::new(image.width() / 2, image.height() / 2);
    for y in center.y - 16..center.y + 16 {
        for x in center.x - 16..center.x + 16 {
            let p = image.get_color_at(x, y).unwrap().to_srgba();
            assert!(
                p.red > p.green + 0.35 && p.blue > p.green + 0.35,
                "forest drew through an opaque foreground mesh at ({x}, {y}): {p:?}"
            );
        }
    }
    app.world_mut().entity_mut(occluder).despawn();
}

fn wait_for_forest(app: &mut App) {
    let start = Instant::now();
    render_frame(app);
    while app.world().resource::<ForestRenderStats>().triangles() == 0 {
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "Igor forest did not stream"
        );
        render_frame(app);
    }
}

fn assert_forest_drawn(app: &App) {
    assert!(app.world().resource::<ForestRenderStats>().triangles() > 0);
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
    if std::env::var("FOREST_GPU_TIMESTAMPS").as_deref() != Ok("0")
        && !app.is_plugin_added::<RenderDiagnosticsPlugin>()
    {
        app.add_plugins(RenderDiagnosticsPlugin);
    }
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
    fn percentiles(mut samples: Vec<f64>) -> (f64, f64) {
        samples.sort_by(f64::total_cmp);
        (
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100],
        )
    }

    let forest_pass = DiagnosticPath::const_new("render/igor_hybrid_forest/elapsed_gpu");
    let gpu_measurement = |app: &App| {
        app.world()
            .get_resource::<DiagnosticsStore>()
            .and_then(|store| store.get(&forest_pass))
            .and_then(|diagnostic| diagnostic.measurement())
            .map(|sample| (sample.time, sample.value))
    };
    let mut last_gpu_measurement = gpu_measurement(app);
    let mut samples = Vec::with_capacity(36);
    let mut gpu_samples = Vec::with_capacity(36);
    for _ in 0..36 {
        let start = Instant::now();
        render_frame(app);
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        if let Some((time, value)) = gpu_measurement(app) {
            if last_gpu_measurement.is_none_or(|(previous, _)| previous != time) {
                gpu_samples.push(value);
                last_gpu_measurement = Some((time, value));
            }
        }
    }
    let (median, p95) = percentiles(samples);
    println!("{label} median={median:.2} ms p95={p95:.2} ms",);
    if !gpu_samples.is_empty() {
        let (median, p95) = percentiles(gpu_samples);
        println!("{label} forest GPU median={median:.2} ms p95={p95:.2} ms");
    }

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
