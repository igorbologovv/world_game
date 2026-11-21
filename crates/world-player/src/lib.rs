#![warn(missing_docs)]
//! Explorer controls, camera modes, travel, and player presentation.

mod camera;
mod player;
mod trip;

use bevy::{prelude::*, transform::TransformSystems};
use big_space::prelude::{CellCoord, FloatingOrigin};
use camera::{camera_controller, DebugCameraController};
use world_core::{
    planet::{ExplorerSpawn, RADIUS},
    runtime::{
        CameraMotion, Explorer, PlanetRoot, ReferenceFrame, WorldCamera, WorldMovement, WorldReady,
    },
};

/// Spawn and control the primary explorer and world camera.
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
                    .before(TransformSystems::Propagate),
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
    let grid = frames.get(root.entity()).expect("WorldReady creates the planet grid");
    let player = player::Player::new(initial.0);
    let map = player::MapView::default();
    let (position, rotation) = player.camera_pose(map.altitude(1.0), &map);
    let (cell, translation) = grid.translation_to_grid(position);
    root.spawn(
        &mut commands,
        (
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                near: 0.05,
                far: RADIUS as f32 * 20.0,
                ..default()
            }),
            Transform::from_translation(translation).with_rotation(rotation),
            cell,
            FloatingOrigin,
            DebugCameraController::default(),
            WorldCamera::Map,
        ),
    );
    root.spawn(
        &mut commands,
        (player, Explorer, Transform::IDENTITY, Visibility::Hidden),
    )
    .with_children(|children| player::spawn_model(children, &mut meshes, &mut materials));
    root.spawn(
        &mut commands,
        (
            player::Destination,
            Mesh3d(meshes.add(Torus::new(0.60, 0.84))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.84, 0.25),
                unlit: true,
                ..default()
            })),
            Visibility::Hidden,
        ),
    );
    commands.spawn((
        player::Controls,
        Text::new(player::WALK_CONTROLS),
        TextFont { font_size: FontSize::Px(17.0), ..default() },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(18.0),
            right: Val::Px(18.0),
            bottom: Val::Px(16.0),
            ..default()
        },
    ));
}
