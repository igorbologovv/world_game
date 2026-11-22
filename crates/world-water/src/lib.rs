#![warn(missing_docs)]
//! Depth-aware pond-inspired shading for rivers, lakes and the planetary ocean.

mod ocean;
mod river_surface;
mod wetlands;

use bevy::{
    gizmos::config::GizmoConfig,
    gizmos::config::GizmoLineConfig,
    light::NotShadowCaster,
    pbr::MaterialPlugin,
    prelude::*,
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
    transform::TransformSystems,
};
use std::vec::IntoIter;

use world_core::{
    runtime::{CameraMotion, PlanetRoot, WorldAssets, WorldReady},
    SurfaceHeights,
};

/// Render baked rivers, lakes and oceans, and stream local wetland pools.
///
/// Install with the shared world space plugin and Bevy's gizmos and PBR plugins.
pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        wetlands::install(app);
        app.add_plugins((
            MaterialPlugin::<ocean::OceanMaterial>::default(),
            MaterialPlugin::<river_surface::RiverMaterial>::default(),
        ))
        .insert_gizmo_config(
            river_surface::RiverMapLines,
            GizmoConfig {
                line: GizmoLineConfig {
                    width: 2.25,
                    ..default()
                },
                ..default()
            },
        )
        .add_systems(Startup, request.after(WorldReady))
        .add_systems(Update, upload)
        .add_systems(
            PostUpdate,
            ocean::cull_patches
                .after(CameraMotion)
                .before(TransformSystems::Propagate),
        )
        .add_systems(
            PostUpdate,
            river_surface::draw_map.after(TransformSystems::Propagate),
        );
    }
}

type OceanPatches = IntoIter<(Mesh, ocean::OceanPatch)>;

#[derive(Resource)]
struct WaterLoading {
    rivers: Option<Task<Option<(Mesh, river_surface::RiverCenterlines)>>>,
    lakes: Option<Task<Option<Mesh>>>,
    ocean: Option<Task<Option<(Image, Vec<(Mesh, ocean::OceanPatch)>)>>>,
    patches: Option<(Handle<ocean::OceanMaterial>, OceanPatches)>,
}

fn request(mut commands: Commands, assets: Res<WorldAssets>, heights: Res<SurfaceHeights>) {
    let pool = AsyncComputeTaskPool::get();
    let assets = assets.directory().to_path_buf();
    let heights = heights.clone();
    let rivers = pool.spawn({
        let (assets, heights) = (assets.clone(), heights.clone());
        async move { river_surface::load(&assets, &heights) }
    });
    let lakes = pool.spawn({
        let (assets, heights) = (assets.clone(), heights.clone());
        async move { river_surface::load_lakes(&assets, &heights) }
    });
    let ocean = pool.spawn(async move { ocean::load(&heights, &assets) });
    commands.insert_resource(WaterLoading {
        rivers: Some(rivers),
        lakes: Some(lakes),
        ocean: Some(ocean),
        patches: None,
    });
}

fn take_ready<T>(task: &mut Option<Task<Option<T>>>) -> Option<T> {
    let ready = block_on(poll_once(task.as_mut()?))?;
    *task = None;
    ready
}

fn upload(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    pending: Option<ResMut<WaterLoading>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut river_materials: ResMut<Assets<river_surface::RiverMaterial>>,
    mut ocean_materials: ResMut<Assets<ocean::OceanMaterial>>,
) {
    let Some(mut pending) = pending else {
        return;
    };
    if let Some((surface, centerlines)) = take_ready(&mut pending.rivers) {
        commands.insert_resource(centerlines);
        root.spawn(
            &mut commands,
            (
                Mesh3d(meshes.add(surface)),
                MeshMaterial3d(
                    river_materials.add(river_surface::RiverMaterial { brightness: 1.0 }),
                ),
                NotShadowCaster,
                river_surface::RiverWater,
            ),
        );
    }

    // Inland basins are generated at their individual spill elevations and
    // cannot be represented by the global sea-level ocean sphere.
    if let Some(lakes) = take_ready(&mut pending.lakes) {
        root.spawn(
            &mut commands,
            (
                Mesh3d(meshes.add(lakes)),
                MeshMaterial3d(
                    river_materials.add(river_surface::RiverMaterial { brightness: 0.93 }),
                ),
                NotShadowCaster,
            ),
        );
    }

    if let Some((mask, patches)) = take_ready(&mut pending.ocean) {
        let material = ocean_materials.add(ocean::OceanMaterial {
            mask: images.add(mask),
        });
        pending.patches = Some((material, patches.into_iter()));
    }
    if let Some((material, patches)) = &mut pending.patches {
        // Avoid one large burst of mesh allocation and render-world transfers.
        for (mesh, bounds) in patches.by_ref().take(12) {
            root.spawn(
                &mut commands,
                (
                    Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(material.clone()),
                    bounds,
                    NotShadowCaster,
                ),
            );
        }
        if patches.len() == 0 {
            pending.patches = None;
        }
    }
    if pending.rivers.is_none()
        && pending.lakes.is_none()
        && pending.ocean.is_none()
        && pending.patches.is_none()
    {
        commands.remove_resource::<WaterLoading>();
    }
}
