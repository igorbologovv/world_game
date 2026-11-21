#![warn(missing_docs)]
//! Explorer controls, camera modes, travel, and player presentation.

mod camera;
mod player;
mod trip;

use bevy::{
    camera::Exposure, core_pipeline::tonemapping::Tonemapping,
    light::AtmosphereEnvironmentMapLight, pbr::AtmosphereSettings, post_process::bloom::Bloom,
    prelude::*, transform::TransformSystems,
};
use big_space::prelude::FloatingOrigin;
use camera::{camera_controller, DebugCameraController};
use world_core::{
    planet::{ExplorerSpawn, RADIUS},
    runtime::{CameraMotion, Explorer, PlanetRoot, ReferenceFrame, WorldMovement, WorldReady},
};

/// Spawn and control the primary explorer and world camera.
pub struct PlayerPlugin {
    initial: InitialSpawn,
}

impl PlayerPlugin {
    /// Use the given region's explorer placement and initial heading.
    pub fn new(spawn: ExplorerSpawn) -> Self {
        Self {
            initial: InitialSpawn {
                spawn,
                first_person: false,
            },
        }
    }

    /// Open the explorer camera in first person when the world starts.
    pub fn with_first_person_view(mut self) -> Self {
        self.initial.first_person = true;
        self
    }
}

#[derive(Clone, Copy, Resource)]
struct InitialSpawn {
    spawn: ExplorerSpawn,
    first_person: bool,
}

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.initial)
            .init_resource::<trip::TravelSpeed>()
            .add_systems(Startup, (spawn.after(WorldReady), trip::spawn))
            .add_systems(
                Update,
                (
                    trip::change_speed,
                    trip::style_buttons,
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
    let grid = frames
        .get(root.entity())
        .expect("WorldReady creates the planet grid");
    let player = player::Player::new(initial.spawn);
    let (map, (position, rotation), mode) =
        player::MapView::at_spawn(&player, initial.first_person);
    commands.insert_resource(map);
    let (cell, translation) = grid.translation_to_grid(position);
    root.spawn(
        &mut commands,
        (
            Camera3d::default(),
            // Physical sky, aerial perspective and sky-derived image-based lighting.
            AtmosphereSettings {
                // Keep the physical aerial perspective over Mars-scale flight
                // distances. The LUT uses its existing resolution and pass.
                aerial_view_lut_max_distance: 160_000.0,
                ..default()
            },
            AtmosphereEnvironmentMapLight {
                // The cubemap supplies ambient/reflection lighting. The sky
                // itself still uses the full atmosphere lookup textures.
                size: UVec2::splat(64),
                ..default()
            },
            Exposure { ev100: 13.0 },
            Tonemapping::AcesFitted,
            Bloom::NATURAL,
            Projection::Perspective(PerspectiveProjection {
                near: 0.05,
                far: RADIUS as f32 * 20.0,
                ..default()
            }),
            Transform::from_translation(translation).with_rotation(rotation),
            cell,
            FloatingOrigin,
            DebugCameraController::default(),
            mode,
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
        TextFont {
            font_size: FontSize::Px(17.0),
            ..default()
        },
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
