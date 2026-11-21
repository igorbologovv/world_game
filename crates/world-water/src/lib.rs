#![warn(missing_docs)]
//! Runtime river and ocean rendering from generated world assets.

mod ocean;
mod river_surface;

use bevy::{
    gizmos::config::GizmoConfig,
    pbr::{MaterialPlugin, NotShadowCaster},
    prelude::*,
    transform::TransformSystem,
};
use world_core::{
    runtime::{CameraMotion, PlanetRoot, WorldAssets, WorldReady},
    SurfaceHeights,
};

/// Load generated river and water data and register their render systems.
///
/// Install with the shared world space plugin and Bevy's gizmos and PBR plugins.
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<ocean::OceanMaterial>::default(),
            MaterialPlugin::<river_surface::RiverMaterial>::default(),
        ))
        .insert_gizmo_config(
            river_surface::RiverMapLines,
            GizmoConfig {
                line_width: 2.25,
                ..default()
            },
        )
        .add_systems(Startup, spawn.after(WorldReady))
        .add_systems(
            PostUpdate,
            ocean::cull_patches
                .after(CameraMotion)
                .before(TransformSystem::TransformPropagate),
        )
        .add_systems(
            PostUpdate,
            river_surface::draw_map.after(TransformSystem::TransformPropagate),
        );
    }
}

fn spawn(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    assets: Res<WorldAssets>,
    heights: Res<SurfaceHeights>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut river_materials: ResMut<Assets<river_surface::RiverMaterial>>,
    mut ocean_materials: ResMut<Assets<ocean::OceanMaterial>>,
) {
    if let Some((surface, centerlines)) = river_surface::load(assets.directory()) {
        commands.insert_resource(centerlines);
        root.spawn(
            &mut commands,
            (
                MaterialMeshBundle {
                    mesh: meshes.add(surface),
                    material: river_materials.add(river_surface::RiverMaterial { brightness: 1.0 }),
                    ..default()
                },
                NotShadowCaster,
                river_surface::RiverWater,
            ),
        );
    }

    // Inland basins are generated at their individual spill elevations and
    // cannot be represented by the global sea-level ocean sphere.
    if let Some(lakes) = river_surface::load_lakes(assets.directory()) {
        root.spawn(
            &mut commands,
            (
                MaterialMeshBundle {
                    mesh: meshes.add(lakes),
                    material: river_materials
                        .add(river_surface::RiverMaterial { brightness: 0.93 }),
                    ..default()
                },
                NotShadowCaster,
            ),
        );
    }

    if let Some((material, patches)) = ocean::load(&mut images, &heights, assets.directory()) {
        let material = ocean_materials.add(material);
        for (mesh, bounds) in patches {
            root.spawn(
                &mut commands,
                (
                    MaterialMeshBundle {
                        mesh: meshes.add(mesh),
                        material: material.clone(),
                        ..default()
                    },
                    bounds,
                    NotShadowCaster,
                ),
            );
        }
    }
}
