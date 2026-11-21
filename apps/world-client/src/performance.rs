//! Reproducible offscreen frame-time benchmark for the Mars client.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use bevy::{
    app::PluginsState,
    camera::RenderTarget,
    diagnostic::DiagnosticsStore,
    render::{
        diagnostic::RenderDiagnosticsPlugin,
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{PollType, TextureFormat, TextureUsages},
        renderer::RenderDevice,
        view::screenshot::{save_to_disk, Screenshot},
        RenderApp,
    },
    time::TimeUpdateStrategy,
    ui::IsDefaultUiCamera,
    window::ExitCondition,
    winit::WinitPlugin,
};
use world_core::{
    planet::{RADIUS, WATER_SURFACE_ELEVATION},
    runtime::{GridTransform, PlanetRoot, ReferenceFrame, WorldCamera},
    SurfaceHeights,
};

use super::*;

const FRAMES: usize = 360;
const STARTUP: usize = 60;

#[test]
#[ignore = "requires a graphics device"]
fn mars_frame_times() {
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
    )
    .add_plugins(RenderDiagnosticsPlugin);
    install_world(&mut app, assets, PALM_COAST_REGION);
    while app.plugins_state() == PluginsState::Adding {
        bevy::tasks::tick_global_task_pools_on_main_thread();
        std::thread::yield_now();
    }
    app.finish();
    app.cleanup();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        20,
    )));
    let mut image = Image::new_target_texture(960, 540, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let camera_target = target.clone();
    app.add_systems(
        PostStartup,
        move |mut commands: Commands, cameras: Query<Entity, With<WorldCamera>>| {
            commands.entity(cameras.single().unwrap()).insert((
                RenderTarget::Image(camera_target.clone().into()),
                IsDefaultUiCamera,
            ));
        },
    );

    let mut update = Vec::new();
    let mut total = Vec::new();
    let mut gpu = Vec::new();
    let mut passes: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut peak_meshes = 0;
    let screenshot = std::env::var("MARS_BENCH_SCREENSHOT").ok();
    for frame in 0..FRAMES {
        if frame > 0 {
            place_camera(app.world_mut(), frame);
        }
        let start = Instant::now();
        app.update();
        let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;
        app.sub_app(RenderApp)
            .world()
            .resource::<RenderDevice>()
            .poll(PollType::wait_indefinitely())
            .unwrap();
        update.push(cpu_ms);
        total.push(start.elapsed().as_secs_f64() * 1000.0);
        peak_meshes = peak_meshes.max(app.world().resource::<Assets<Mesh>>().len());

        // Bevy reports the previous completed frame's per-pass GPU timestamps.
        // Sum leaf spans so nested GPU spans are counted only once.
        let diagnostics = app.world().resource::<DiagnosticsStore>();
        let timestamps: Vec<_> = diagnostics
            .iter()
            .filter_map(|d| Some((d.path().as_str().strip_suffix("/elapsed_gpu")?, d.value()?)))
            .collect();
        if frame > 0 && !timestamps.is_empty() {
            let mut sum = 0.0;
            for &(path, ms) in &timestamps {
                if timestamps.iter().any(|&(other, _)| {
                    other
                        .strip_prefix(path)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                }) {
                    continue;
                }
                sum += ms;
                if frame > STARTUP {
                    passes.entry(path.to_owned()).or_default().push(ms);
                }
            }
            gpu.push((frame - 1, sum));
        }
        if let Some(prefix) = &screenshot {
            if matches!(frame, 55 | 140 | 255 | 350) {
                app.world_mut()
                    .spawn(Screenshot::image(target.clone()))
                    .observe(save_to_disk(format!("{prefix}-{frame}.png")));
            }
        }
    }
    print_stats("startup update", &update[..STARTUP]);
    print_stats("startup wall", &total[..STARTUP]);
    print_stats("travel update", &update[STARTUP..]);
    print_stats("travel wall", &total[STARTUP..]);
    let travel_gpu: Vec<_> = gpu
        .iter()
        .filter_map(|&(frame, ms)| (frame >= STARTUP).then_some(ms))
        .collect();
    assert!(!travel_gpu.is_empty(), "missing Vulkan GPU timestamps");
    print_stats("travel GPU passes", &travel_gpu);
    println!("Peak resident mesh assets: {peak_meshes}");
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        if let Some(rss) = status.lines().find(|line| line.starts_with("VmHWM:")) {
            println!("{rss}");
        }
    }
    let mut hotspots: Vec<_> = passes
        .into_iter()
        .map(|(path, times)| (times.iter().sum::<f64>() / times.len() as f64, path))
        .collect();
    hotspots.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (mean, path) in hotspots.into_iter().take(12) {
        println!("GPU {mean:7.3} ms mean {path}");
    }
    if let Ok(path) = std::env::var("MARS_BENCH_CSV") {
        use std::io::Write;
        let mut output = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        writeln!(output, "frame,update_ms,wall_ms,gpu_pass_ms").unwrap();
        let gpu: BTreeMap<_, _> = gpu.into_iter().collect();
        for (frame, (&update, &total)) in update.iter().zip(&total).enumerate() {
            writeln!(
                output,
                "{frame},{update:.4},{total:.4},{}",
                gpu.get(&frame)
                    .map_or(String::new(), |ms| format!("{ms:.4}"))
            )
            .unwrap();
        }
    }

    if let Some(prefix) = screenshot {
        // Inspect the starting camera after the travel route has warmed the
        // pipelines. Keep this view outside the measured frame windows.
        for frame in 0..90 {
            place_camera(app.world_mut(), 1);
            if frame == 87 {
                app.world_mut()
                    .spawn(Screenshot::image(target.clone()))
                    .observe(save_to_disk(format!("{prefix}-palm-coast.png")));
            }
            app.update();
            app.sub_app(RenderApp)
                .world()
                .resource::<RenderDevice>()
                .poll(PollType::wait_indefinitely())
                .unwrap();
        }
    }
}

fn print_stats(label: &str, times: &[f64]) {
    let mut sorted = times.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((p * sorted.len() as f64).ceil() as usize).saturating_sub(1)];
    println!(
        "BENCH {label:20} n={:3} P50={:8.3} P95={:8.3} P99={:8.3} max={:8.3} ms",
        sorted.len(),
        percentile(0.5),
        percentile(0.95),
        percentile(0.99),
        sorted[sorted.len() - 1]
    );
}

fn place_camera(world: &mut World, frame: usize) {
    let (region, distance, altitude, mode) = match frame {
        0..=59 => (PALM_COAST_REGION, 0.0, 2.2, WorldCamera::FirstPerson),
        60..=159 => (
            PALM_COAST_REGION,
            (frame - 60) as f64 * 17.0,
            35.0,
            WorldCamera::Map,
        ),
        160..=279 => (
            STARTER_REGION,
            (frame - 160) as f64 * 33.0,
            96.0,
            WorldCamera::Map,
        ),
        _ => (
            PALM_COAST_REGION,
            (360 - frame) as f64 * 16.0,
            2.2,
            WorldCamera::FirstPerson,
        ),
    };
    let start = terrain_aligned_direction(region.spawn.direction);
    let look = terrain_aligned_direction(region.spawn.look_at);
    let heading = (look - start * start.dot(look)).normalize();
    let up = (start + heading * (distance / RADIUS)).normalize();
    let heading = (heading - up * up.dot(heading)).normalize();
    let ground = world
        .resource::<SurfaceHeights>()
        .sample(up)
        .unwrap()
        .max(WATER_SURFACE_ELEVATION);
    let eye = up * (RADIUS + ground + altitude);
    let root = world.resource::<PlanetRoot>().entity();
    let (cell, translation) = world
        .get::<ReferenceFrame>(root)
        .unwrap()
        .translation_to_grid(eye);
    let mut query = world.query::<(GridTransform, &mut WorldCamera, &mut Projection)>();
    let (mut camera, mut camera_mode, mut projection) = query.single_mut(world).unwrap();
    *camera.cell = cell;
    camera.transform.translation = translation;
    let pitch = if mode == WorldCamera::FirstPerson {
        0.02
    } else {
        0.30
    };
    camera
        .transform
        .look_to((heading - up * pitch).as_vec3(), up.as_vec3());
    *camera_mode = mode;
    if let Projection::Perspective(perspective) = &mut *projection {
        perspective.fov = std::f32::consts::FRAC_PI_3;
        perspective.near = (altitude * 0.025).max(0.05) as f32;
        perspective.far = (RADIUS * 4.0) as f32;
    }
}
