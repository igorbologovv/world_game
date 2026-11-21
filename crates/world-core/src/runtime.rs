//! Bevy contracts for clients using a meter-scale, floating-origin planetary scene.

use std::{
    env,
    path::{Path, PathBuf},
};

use bevy::{ecs::system::EntityCommands, prelude::*};
use bevy_terrain::big_space::{BigSpaceCommands, BigSpacePlugin, GridCell};

pub use bevy_terrain::big_space::{GridTransform, ReferenceFrame, ReferenceFrames};

use crate::SurfaceHeights;

/// Asset directory shared by the Bevy asset server and synchronous world data readers.
#[derive(Clone, Resource)]
pub struct WorldAssets(PathBuf);

impl WorldAssets {
    /// Use an explicit directory containing the world assets.
    pub fn new(directory: PathBuf) -> Self {
        Self(std::path::absolute(&directory).unwrap_or(directory))
    }

    /// Resolve packaged, workspace, or explicitly overridden game assets.
    ///
    /// `BEVY_ASSET_ROOT` names the directory containing `assets/`.
    pub fn resolve(workspace_root: &Path) -> Self {
        if let Ok(root) = env::var("BEVY_ASSET_ROOT") {
            return Self::new(PathBuf::from(root).join("assets"));
        }
        for root in [
            env::current_dir().ok(),
            env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf)),
        ]
        .into_iter()
        .flatten()
        {
            let directory = root.join("assets");
            if directory.is_dir() {
                return Self::new(directory);
            }
        }
        Self::new(workspace_root.join("assets"))
    }

    /// Path containing the `world/`, `terrains/`, `models/`, and `shaders/` directories.
    pub fn directory(&self) -> &Path {
        &self.0
    }
}

/// The current view of the shared world camera.
#[derive(Clone, Copy, Component, Debug, Default, Eq, PartialEq)]
pub enum WorldCamera {
    /// Third-person exploration and globe map.
    #[default]
    Map,
    /// Ground-level, first-person exploration.
    FirstPerson,
    /// Free camera.
    Free,
}

/// Marker for the primary explorer entity.
#[derive(Component)]
pub struct Explorer;

/// The planet's root reference frame.
#[derive(Resource)]
pub struct PlanetRoot(Entity);

impl PlanetRoot {
    /// Entity holding the planet's reference frame.
    pub fn entity(&self) -> Entity {
        self.0
    }

    /// Spawn a floating-origin child at the planet origin with a custom bundle.
    pub fn spawn<'a>(&self, commands: &'a mut Commands, bundle: impl Bundle) -> EntityCommands<'a> {
        // Bevy 0.20 has no SpatialBundle; transform/visibility use required components.
        let mut child = commands.spawn((Transform::default(), Visibility::default(), GridCell::default()));
        child.insert(bundle).set_parent_in_place(self.0);
        child
    }
}

/// Startup phase that publishes the reference frame and terrain height queries.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct WorldReady;

/// Update systems that move the explorer and its gameplay camera.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct WorldMovement;

/// PostUpdate systems that move the free camera before transform propagation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, SystemSet)]
pub struct CameraMotion;

/// Installs the floating origin and publishes shared planetary data in `WorldReady`.
///
/// Disable Bevy's default `TransformPlugin` when using this plugin.
pub struct WorldSpacePlugin {
    assets: WorldAssets,
}

impl WorldSpacePlugin {
    /// Set the asset source for the synchronous height and river queries.
    pub fn new(assets: WorldAssets) -> Self {
        Self { assets }
    }
}

impl Plugin for WorldSpacePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(BigSpacePlugin::default())
            .insert_resource(self.assets.clone())
            .add_systems(Startup, prepare_world.in_set(WorldReady));
    }
}

fn prepare_world(mut commands: Commands, assets: Res<WorldAssets>) {
    commands.insert_resource(SurfaceHeights::load(assets.directory()));
    let mut root = Entity::PLACEHOLDER;
    commands.spawn_big_space(ReferenceFrame::default(), |frame| root = frame.id());
    commands.insert_resource(PlanetRoot(root));
}
