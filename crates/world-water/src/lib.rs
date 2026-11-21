#![warn(missing_docs)]
//! Runtime river and ocean rendering from generated world assets.

mod ocean;
mod river_surface;

use bevy::{
    gizmos::config::GizmoConfig,
    pbr::NotShadowCaster,
    prelude::*,
    render::alpha::AlphaMode,
    transform::TransformSystem,
};
use world_core::runtime::{CameraMotion, PlanetRoot, WorldAssets, WorldReady};

/// Load generated river and water data and register their render systems.
///
/// Install with the shared world space plugin and Bevy's gizmos and PBR plugins.
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.insert_gizmo_config(
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
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if let Some((surface, centerlines)) = river_surface::load(assets.directory()) {
        commands.insert_resource(centerlines);
        root.spawn(
            &mut commands,
            (
                MaterialMeshBundle {
                    mesh: meshes.add(surface),
                    material: materials.add(pond_material()),
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
                    material: materials.add(pond_material()),
                    ..default()
                },
                NotShadowCaster,
            ),
        );
    }

    if let Some((material, patches)) = ocean::load(&mut images, assets.directory()) {
        let material = materials.add(material);
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

/// The actual `material_3` from the source woodland glTF: a translucent
/// warm-white PBR surface, not a hand-picked blue custom water shader.
fn pond_material() -> StandardMaterial {
    StandardMaterial {
        base_color: Color::linear_rgba(1.0, 0.937488, 0.892829, 0.60),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.50,
        metallic: 0.0,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

#[cfg(test)]
mod material_tests {
    use super::*;

    #[test]
    fn pond_material_uses_source_scene_pbr_parameters() {
        let material = pond_material();
        assert!(matches!(material.alpha_mode, AlphaMode::Blend));
        assert!((material.perceptual_roughness - 0.5).abs() < 1e-6);
        assert_eq!(material.metallic, 0.0);
        assert_eq!(material.base_color, Color::linear_rgba(1.0, 0.937488, 0.892829, 0.60));
    }
}
