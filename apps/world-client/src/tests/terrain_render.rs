//! Matched gameplay captures for the terrain material library.

use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    diagnostic::{DiagnosticPath, DiagnosticsStore},
    math::DVec3,
    render::{
        diagnostic::RenderDiagnosticsPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor, TextureFormat},
        RenderApp,
    },
    time::TimeUpdateStrategy,
    window::ExitCondition,
    winit::WinitPlugin,
};

use super::{
    cloud_render::{capture, press_key, render_frame},
    *,
};

struct Site {
    name: &'static str,
    direction: DVec3,
    heading: DVec3,
}

#[test]
#[ignore = "requires a Vulkan adapter and writes screenshots"]
fn terrain_material_sweep() {
    let output = std::env::var_os("TERRAIN_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/terrain-materials"));
    let output = Path::new(WORKSPACE_ROOT).join(output);
    fs::create_dir_all(&output).unwrap();
    let case = std::env::var("TERRAIN_CAPTURE_CASE").ok();

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
    // Keep the sun and any wind-driven materials fixed across separate runs.
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
            1280,
            720,
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
    press_key(&mut app, KeyCode::F2, Key::F2);
    // Isolate material and vegetation cost from clouds.
    press_key(&mut app, KeyCode::F6, Key::F6);
    press_key(&mut app, KeyCode::F6, Key::F6);
    let sites = sites(app.world().resource::<SurfaceHeights>());
    for site in sites {
        if case.as_deref().is_some_and(|patterns| {
            !patterns
                .split(',')
                .any(|pattern| site.name.contains(pattern))
        }) {
            continue;
        }
        let poses: &[(&str, f64, f64)] = match site.name {
            "distant" => &[("aerial", 4800.0, 0.92)],
            "river-bank" => &[("ground", 2.35, 0.22), ("elevated", 36.0, 0.54)],
            "coastline" | "lake-shore" => &[("ground", 2.35, 0.14), ("elevated", 62.0, 0.56)],
            _ => &[("ground", 2.35, 0.24), ("elevated", 62.0, 0.72)],
        };
        for &(view, altitude, tilt) in poses {
            let ground = set_camera(app.world_mut(), &site, altitude, tilt);
            let label = format!("{}-{view}", site.name);
            println!(
                "{label} direction={:?} surface={ground:.2}m",
                site.direction
            );
            for _ in 0..160 {
                render_frame(&mut app);
            }
            assert_material_pipeline(&app, "shaders/spherical.wesl");
            if site.name == "coastline" {
                assert_material_pipeline(&app, "shaders/ocean.wesl");
            }
            if matches!(site.name, "river-bank" | "lake-shore" | "wet-lowland") {
                assert_material_pipeline(&app, "shaders/river_water.wesl");
            }
            measure(&mut app, &label);
            capture(&mut app, &target, output.join(format!("{label}.png")));
        }
    }
}

fn project(direction: DVec3, tangent: DVec3, meters: f64) -> DVec3 {
    (direction + tangent * (meters / RADIUS)).normalize()
}

fn sites(heights: &SurfaceHeights) -> [Site; 9] {
    let sun = DVec3::new(-0.90, 0.40, -0.21).normalize();
    let lit_side = |up: DVec3| (up * up.dot(sun) - sun + up.cross(sun) * 0.55).normalize();
    let site = |name, direction: DVec3| {
        let direction = direction.normalize();
        Site {
            name,
            direction,
            heading: lit_side(direction),
        }
    };
    let forest = site(
        "dense-forest",
        DVec3::new(-0.4677238208, 0.7499886686, -0.4677086960),
    );
    let conifer = site(
        "conifer",
        DVec3::new(-0.3628517435, 0.8249897523, 0.4332788026),
    );
    let meadow = site(
        "grassland",
        DVec3::new(-0.9918296980, 0.0499786832, 0.1173711264),
    );
    let palm = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);

    let pool = heights
        .local_pools(palm, 240.0)
        .into_iter()
        .max_by(|a, b| a.radius.total_cmp(&b.radius))
        .expect("Palm Coast wetland");
    let up = project(pool.center, -lit_side(pool.center), pool.radius + 5.0);
    let wetland = Site {
        name: "wet-lowland",
        direction: up,
        heading: lit_side(up),
    };

    let toward = terrain_aligned_direction(PALM_COAST_REGION.spawn.look_at);
    let toward = (toward - palm * palm.dot(toward)).normalize();
    let mut interval = [0.0, 15_000.0];
    for _ in 0..28 {
        let middle = (interval[0] + interval[1]) * 0.5;
        let height = heights.sample(project(palm, toward, middle)).unwrap();
        interval[usize::from(height <= WATER_SURFACE_ELEVATION)] = middle;
    }
    let shore = project(palm, toward, interval[0] - 8.0);
    let to_sea = (toward - shore * shore.dot(toward)).normalize();
    let coastline = Site {
        name: "coastline",
        direction: shore,
        heading: (shore.cross(to_sea) * 0.88 + to_sea * 0.47).normalize(),
    };

    let mountain = site("mountain", find_mountain_slope(heights, conifer.direction));
    let distant = site(
        "distant",
        DVec3::new(-0.4024915160440224, 0.8031141035346536, 0.4393271175517328),
    );
    let [river, lake] = inland_sites(heights);
    [
        forest, conifer, wetland, mountain, meadow, coastline, distant, river, lake,
    ]
}

fn inland_sites(heights: &SurfaceHeights) -> [Site; 2] {
    use world_core::WaterKind;

    let start = terrain_aligned_direction(STARTER_REGION.spawn.direction);
    let classify = |up| {
        heights
            .sample(up)
            .and_then(|h| heights.water_at(up, h))
            .map(|water| water.kind)
    };
    let toward = |from: DVec3, to: DVec3| (to - from * to.dot(from)).normalize();
    let reach = heights
        .river_reaches()
        .iter()
        .filter(|reach| {
            (15.0..650.0).contains(&reach.half_width(0.5))
                && (3.0..850.0).contains(&reach.elevation(0.5))
                && (reach.elevation(0.0) - reach.elevation(1.0)) / reach.length() < 0.016
                && classify(reach.direction(0.5, 0.0)) == Some(WaterKind::River)
        })
        .min_by(|a, b| {
            a.direction(0.5, 0.0)
                .distance_squared(start)
                .total_cmp(&b.direction(0.5, 0.0).distance_squared(start))
        })
        .expect("river reach near the starter region");
    let river_center = reach.direction(0.5, 0.0);
    let bank = reach.direction(0.5, 1.0 + 7.0 / reach.half_width(0.5));
    let cross = toward(bank, river_center);
    let river = Site {
        name: "river-bank",
        direction: bank,
        heading: (cross * 0.62 + bank.cross(cross) * 0.78).normalize(),
    };

    let mut shore = None;
    let mut distance = f64::INFINITY;
    for tile in heights.lake_tiles() {
        let center = tile.corners.iter().sum::<DVec3>().normalize();
        let d = center.distance_squared(start);
        if d >= distance || classify(center) != Some(WaterKind::Lake) {
            continue;
        }
        for edge in 0..4 {
            let land = (tile.corners[edge] + tile.corners[(edge + 1) % 4]).normalize();
            if heights
                .sample(land)
                .is_none_or(|h| h < tile.elevation + 1.0)
            {
                continue;
            }
            let (mut wet, mut dry) = (center, land);
            for _ in 0..20 {
                let next = (wet + dry).normalize();
                if heights.sample(next).is_some_and(|h| h < tile.elevation) {
                    wet = next;
                } else {
                    dry = next;
                }
            }
            let cross = toward(dry, center);
            let bank = project(dry, -cross, 6.0);
            shore = Some(Site {
                name: "lake-shore",
                direction: bank,
                heading: (cross * 0.59 + bank.cross(cross) * 0.81).normalize(),
            });
            distance = d;
            break;
        }
    }
    [river, shore.expect("lake with an exposed sloping shore")]
}

fn find_mountain_slope(heights: &SurfaceHeights, center: DVec3) -> DVec3 {
    let east = center.any_orthonormal_vector();
    let north = center.cross(east);
    let elevation = |up| heights.sample(up).unwrap_or(0.0);
    let mut best = (0.0, center);
    for x in -32..=32 {
        for y in -32..=32 {
            let up = (center + (east * f64::from(x) + north * f64::from(y)) * (4_200.0 / RADIUS))
                .normalize();
            let h = elevation(up);
            if !(1500.0..2450.0).contains(&h) {
                continue;
            }
            let ge =
                (elevation(project(up, east, 60.0)) - elevation(project(up, east, -60.0))) / 120.0;
            let gn = (elevation(project(up, north, 60.0)) - elevation(project(up, north, -60.0)))
                / 120.0;
            let grade = ge.hypot(gn);
            let score = 1.0 - (grade - 0.62).abs() + (h / 3000.0).min(1.0) * 0.50;
            if score > best.0 && (0.28..0.92).contains(&grade) {
                best = (score, up);
            }
        }
    }
    assert!(best.0 > 0.0, "expected an exposed mountain slope");
    best.1
}

fn set_camera(world: &mut World, site: &Site, altitude: f64, tilt: f64) -> f64 {
    let up = site.direction;
    let ground = world.resource::<SurfaceHeights>().sample(up).unwrap();
    let forward = site.heading * (1.0 - tilt * tilt).sqrt() - up * tilt;
    let root = world.resource::<PlanetRoot>().entity();
    let (cell, translation) = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .translation_to_grid(up * (RADIUS + ground + altitude));
    let mut cameras = world.query::<(GridTransform, &mut WorldCamera, &mut Projection)>();
    let (mut camera, mut mode, mut projection) = cameras.single_mut(world).unwrap();
    *camera.cell = cell;
    *camera.transform =
        Transform::from_translation(translation).looking_to(forward.as_vec3(), up.as_vec3());
    *mode = WorldCamera::Free;
    if let Projection::Perspective(perspective) = &mut *projection {
        perspective.near = 0.09;
    }
    ground
}

fn assert_material_pipeline(app: &App, path: &'static str) {
    let shader = app.world().resource::<AssetServer>().load::<Shader>(path);
    let cache = app.sub_app(RenderApp).world().resource::<PipelineCache>();
    let mut ready = false;
    for pipeline in cache.pipelines() {
        let PipelineDescriptor::RenderPipelineDescriptor(descriptor) = &pipeline.descriptor else {
            continue;
        };
        if descriptor
            .fragment
            .as_ref()
            .is_some_and(|stage| stage.shader == shader)
        {
            assert!(
                !matches!(pipeline.state, CachedPipelineState::Err(_)),
                "{:?}",
                pipeline.state
            );
            ready |= matches!(pipeline.state, CachedPipelineState::Ok(_));
        }
    }
    assert!(ready, "{path} pipeline did not compile");
}

fn measure(app: &mut App, label: &str) {
    let opaque = DiagnosticPath::const_new("render/main_opaque_pass_3d/elapsed_gpu");
    let measurement = |app: &App| {
        app.world()
            .resource::<DiagnosticsStore>()
            .get(&opaque)
            .and_then(|diagnostic| diagnostic.measurement())
            .map(|sample| (sample.time, sample.value))
    };
    let mut previous = measurement(app).map(|sample| sample.0);
    let mut frame_times = Vec::new();
    let mut opaque_times = Vec::new();
    for _ in 0..48 {
        let start = Instant::now();
        render_frame(app);
        frame_times.push(start.elapsed().as_secs_f64() * 1000.0);
        if let Some((time, value)) = measurement(app) {
            if previous != Some(time) {
                opaque_times.push(value);
                previous = Some(time);
            }
        }
    }
    frame_times.sort_by(f64::total_cmp);
    opaque_times.sort_by(f64::total_cmp);
    assert!(
        !opaque_times.is_empty(),
        "GPU timestamps are required for the comparison"
    );
    println!(
        "terrain-timing,{label},{:.3},{:.3},{:.3}",
        opaque_times[opaque_times.len() / 2],
        frame_times[frame_times.len() / 2],
        frame_times[frame_times.len() * 95 / 100]
    );
}
