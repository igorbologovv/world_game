//! Offscreen altitude sweep and steady-state frame timing on the active adapter.

use std::{fs, path::PathBuf, time::Instant};

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

use world_core::canopy::ForestCanopy;

use super::*;

#[test]
#[ignore = "requires a GPU adapter and writes screenshots"]
fn forest_altitude_sweep() {
    let output = std::env::var_os("FOREST_CAPTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(WORKSPACE_ROOT).join("target/forest-sweep"));
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
    render_frame(&mut app);
    let coast = terrain_aligned_direction(PALM_COAST_REGION.spawn.direction);
    // This inland direction looks over a boreal river valley and broken ridges.
    let woodland = DVec3::new(
        -0.36285174347169147,
        0.8249897523299634,
        0.43327880263186114,
    );
    for (label, up, height, tilt, warmup) in [
        ("ground", coast, 2.2, 0.025, 220),
        ("handoff", woodland, 1_700.0, 0.78, 75),
        ("aerial", woodland, 24_000.0, 0.87, 65),
        ("orbit", woodland, 1_450_000.0, 1.0, 90),
    ] {
        set_camera(app.world_mut(), up, height, tilt);
        for _ in 0..warmup {
            render_frame(&mut app);
        }
        let mut samples: Vec<_> = (0..36)
            .map(|_| {
                let start = Instant::now();
                render_frame(&mut app);
                start.elapsed().as_secs_f64() * 1000.0
            })
            .collect();
        samples.sort_by(f64::total_cmp);
        println!(
            "{label} median={:.2} ms p95={:.2} ms",
            samples[samples.len() / 2],
            samples[samples.len() * 95 / 100]
        );

        let path = output.join(format!("{label}.png"));
        if path.exists() {
            fs::remove_file(&path).unwrap();
        }
        app.world_mut()
            .spawn(Screenshot::image(target.clone()))
            .observe(save_to_disk(path.clone()));
        for _ in 0..15 {
            render_frame(&mut app);
            if path.metadata().is_ok_and(|info| info.len() > 1_000) {
                break;
            }
        }
        assert!(path.metadata().unwrap().len() > 1_000, "{path:?}");
    }
    let layout = app.world().resource::<ForestCanopy>().layout;
    assert_eq!(layout.fade.z, 0.0, "the 1024² faces must finish uploading");
    assert_eq!(
        layout.fade.w, 1.0,
        "the orbital canopy must finish fading in"
    );
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
