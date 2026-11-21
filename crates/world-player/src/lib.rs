#![warn(missing_docs)]
//! Explorer controls, camera modes, travel, and player presentation.

mod player;
mod trip;

use bevy::{pbr::FogSettings, prelude::*, transform::TransformSystem};
use bevy_terrain::{debug::camera::camera_controller, prelude::DebugCameraBundle};
use world_core::{
    planet::{ExplorerSpawn, RADIUS},
    runtime::{
        CameraMotion, Explorer, PlanetRoot, ReferenceFrame, WorldCamera, WorldMovement, WorldReady,
    },
};

/// Spawn and control the primary explorer and world camera.
///
/// Install with Bevy's default client plugins and the shared world space plugin.
/// Press F to take off or land on dry ground, and R to center the camera on the explorer.
pub struct PlayerPlugin {
    spawn: ExplorerSpawn,
}

impl PlayerPlugin {
    /// Use the given region's explorer placement and initial heading.
    pub fn new(spawn: ExplorerSpawn) -> Self {
        Self { spawn }
    }
}

#[derive(Resource)]
struct InitialSpawn(ExplorerSpawn);

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(InitialSpawn(self.spawn))
            .init_resource::<trip::TravelSpeed>()
            .add_systems(Startup, (spawn.after(WorldReady), trip::spawn))
            .add_systems(
                Update,
                (
                    trip::change_speed,
                    player::walk.in_set(WorldMovement),
                    player::animate,
                    trip::update,
                    player::update_controls,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                camera_controller
                    .in_set(CameraMotion)
                    .before(TransformSystem::TransformPropagate),
            );
    }
}

fn spawn(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    frames: Query<&ReferenceFrame>,
    initial: Res<InitialSpawn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let frame = frames
        .get(root.entity())
        .expect("WorldReady creates the planet reference frame");
    let player = player::Player::new(initial.0);
    let map = player::MapView::default();
    let (camera_position, rotation) = player.camera_pose(map.altitude(1.0), &map);
    let mut camera = DebugCameraBundle::new(camera_position, 300.0, frame);
    camera.camera.transform.rotation = rotation;
    if let Projection::Perspective(perspective) = &mut camera.camera.projection {
        perspective.near = 0.05;
        perspective.far = RADIUS as f32 * 20.0;
    }
    root.spawn(
        &mut commands,
        (camera, WorldCamera::Map, FogSettings::default()),
    );
    root.spawn(
        &mut commands,
        (player, Explorer, SpatialBundle::HIDDEN_IDENTITY),
    )
    .with_children(|children| player::spawn_model(children, &mut meshes, &mut materials));
    root.spawn(
        &mut commands,
        (
            player::Destination,
            PbrBundle {
                mesh: meshes.add(Torus::new(0.60, 0.84)),
                material: materials.add(StandardMaterial {
                    base_color: Color::srgb(1.0, 0.84, 0.25),
                    unlit: true,
                    ..default()
                }),
                visibility: Visibility::Hidden,
                ..default()
            },
        ),
    );

    commands.spawn((
        player::Controls,
        TextBundle::from_section(
            player::WALK_CONTROLS,
            TextStyle {
                font_size: 17.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            position_type: PositionType::Absolute,
            left: Val::Px(18.0),
            right: Val::Px(18.0),
            bottom: Val::Px(16.0),
            ..default()
        }),
    ));
}
