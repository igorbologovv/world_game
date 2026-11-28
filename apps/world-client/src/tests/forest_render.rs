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
        let path = format!("library/{species}/");
        for frame in 0..720 {
            render_frame(&mut app);
            if frame > 85 && frame % 15 == 0 {
                let world = app.world_mut();
                if world
                    .query::<&Name>()
                    .iter(world)
                    .any(|name| name.as_str().starts_with(&path))
                {
                    break;
                }
            }
        }
        let world = app.world_mut();
        let root = world.resource::<PlanetRoot>().entity();
        let frame = world.get::<ReferenceFrame>(root).unwrap().clone();
        let mut nearby = world.query::<(&Name, &GridCell, &Transform)>();
        let sites: Vec<_> = nearby
            .iter(world)
            .filter(|(name, _, _)| {
                name.as_str() == "Forest tree" || name.as_str().starts_with("library/")
            })
            .map(|(name, cell, transform)| {
                (
                    name.as_str().starts_with(&path),
                    frame.grid_position_double(cell, transform),
                )
            })
            .collect();
        let heights = world.resource::<SurfaceHeights>();
        let sun = DVec3::new(-0.78, 0.60, -0.18).normalize();
        let mut best = None;
        for (_, tree) in sites.iter().filter(|(selected, _)| *selected) {
            let radial = tree.normalize();
            let tangent = (sun - radial * sun.dot(radial)).normalize();
            let focus = tree + radial * (height * 0.47);
            for i in 0..12 {
                let angle = f64::from(i) * std::f64::consts::TAU / 12.0;
                let arm = tangent * angle.cos() + radial.cross(tangent) * angle.sin();
                let eye = tree + radial * (height * 0.53) + arm * (height * 1.34);
                if heights
                    .sample(eye.normalize())
                    .is_some_and(|ground| eye.length() < RADIUS + ground + 2.0)
                {
                    continue;
                }
                let ray = eye - focus;
                let clearance = sites
                    .iter()
                    .filter(|(_, other)| *other != *tree)
                    .map(|(_, other)| {
                        let center = *other + other.normalize() * (height * 0.5);
                        let t = ((center - focus).dot(ray) / ray.length_squared()).clamp(0.05, 1.0);
                        center.distance(focus + ray * t)
                    })
                    .fold(35.0_f64, f64::min);
                let score = clearance + tangent.dot(arm) * 2.5;
                if best.is_none_or(|(old, _, _, _)| score > old) {
                    best = Some((score, *tree, radial, arm));
                }
            }
        }
        let (_, tree, radial, arm) =
            best.unwrap_or_else(|| panic!("{species} did not stream in its habitat"));
        for (label, distance) in [("detail", height * 1.34), ("distance", height * 8.0)] {
            let mut eye = tree + radial * (height * 0.53) + arm * distance;
            if label == "distance" {
                let direction = eye.normalize();
                let ground = app
                    .world()
                    .resource::<SurfaceHeights>()
                    .sample(direction)
                    .unwrap_or(0.0);
                let minimum = RADIUS + ground + height * 1.3;
                eye = direction * eye.length().max(minimum);
            }
            aim_at_tree(
                app.world_mut(),
                eye,
                tree + radial * (height * 0.47),
                radial,
            );
            for _ in 0..64 {
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
        for _ in 0..warmup {
            render_frame(&mut app);
        }
        measure(&mut app, &target, &output, label);
        if label == "ground" {
            assert_hybrid_pipelines(&app);
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
    for (label, forest_range) in [
        ("reference-hybrid-meshes", 3_000.0),
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

    // From the spawn clearing, keep foreground trees out of this comparison.
    // Hold the view fixed and force the representation on either side of the fade.
    let view = (origin - tree - up * (origin - tree).dot(up)).normalize();
    let position = |distance| focus + distance * (view * 0.997 + up * 0.077);
    aim_at_tree(app.world_mut(), position(400.0), focus, up);
    let defaults = *app
        .world()
        .resource::<world_vegetation::VegetationSettings>();
    let mut compared = std::collections::HashSet::new();
    for (label, mesh) in [
        ("paired-lod0", [1_200.0, 1_700.0]),
        ("paired-lod1", [10.0, 1_200.0]),
        ("paired-lod2", [10.0, 20.0]),
    ] {
        app.insert_resource(world_vegetation::VegetationSettings {
            mesh_distances: mesh,
            ..defaults
        });
        for _ in 0..75 {
            render_frame(&mut app);
        }
        let world = app.world_mut();
        let frame = world.get::<ReferenceFrame>(root).unwrap().clone();
        let mut palms = world.query_filtered::<
            (&GridCell, &Transform, &Mesh3d, &bevy::mesh::MeshTag),
            With<world_vegetation::CoconutPalm>,
        >();
        let (_, _, mesh, phase) = palms
            .iter(world)
            .find(|(cell, transform, _, _)| {
                frame
                    .grid_position_double(cell, transform)
                    .distance_squared(tree)
                    < 0.0001
            })
            .expect("the same planetary palm must remain resident");
        assert_eq!(phase.value, 0, "{label} should have settled");
        assert!(
            compared.insert(mesh.id()),
            "{label} must exercise a distinct mesh prefix"
        );
        capture(&mut app, &target, &output, label);
    }
    assert_hybrid_pipelines(&app);
    app.insert_resource(defaults);
    for _ in 0..75 {
        render_frame(&mut app);
    }
    for frame in 0..=100 {
        let distance = 45.0 + 4.2 * f64::from(frame);
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
    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/broadleaf-lighting"));
    fs::create_dir_all(&output).unwrap();
    let (mut app, target) = render_app();
    let settings = *app
        .world()
        .resource::<world_vegetation::VegetationSettings>();
    let mesh_settings = world_vegetation::VegetationSettings {
        mesh_distances: [180.0, 210.0],
        ..settings
    };
    app.insert_resource(mesh_settings);
    for _ in 0..360 {
        render_frame(&mut app);
    }
    let coast = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    let world = app.world_mut();
    let root = world.resource::<PlanetRoot>().entity();
    let frame = world.get::<ReferenceFrame>(root).unwrap().clone();
    let mut sites = world.query::<(&Name, &GridCell, &Transform)>();
    let tree = sites
        .iter(world)
        .filter(|(name, _, _)| name.as_str() == "tree_part9/tree")
        .map(|(_, cell, transform)| frame.grid_position_double(cell, transform))
        .min_by(|a, b| {
            a.distance_squared(coast * RADIUS)
                .total_cmp(&b.distance_squared(coast * RADIUS))
        })
        .expect("Palm Coast must stream the authored tropical broadleaf");
    println!("Tracked broadleaf at {tree:?}");
    let up = tree.normalize();
    let sun = world
        .query::<(&Transform, &bevy::light::SunDisk)>()
        .iter(world)
        .find(|(_, disk)| disk.intensity > 0.0)
        .unwrap()
        .0
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
                "hybrid-middle",
                world_vegetation::VegetationSettings {
                    mesh_distances: [10.0, 180.0],
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
    assert_hybrid_pipelines(&app);
}

fn assert_hybrid_pipelines(app: &App) {
    use bevy::render::render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor};

    let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
    let assets = app.world().resource::<AssetServer>();
    for path in [
        "shaders/hybrid_tree.wesl",
        "shaders/hybrid_tree_prepass.wesl",
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

    let opaque_pass = DiagnosticPath::const_new("render/main_opaque_pass_3d/elapsed_gpu");
    let gpu_measurement = |app: &App| {
        app.world()
            .get_resource::<DiagnosticsStore>()
            .and_then(|store| store.get(&opaque_pass))
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
        println!("{label} opaque GPU median={median:.2} ms p95={p95:.2} ms");
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
