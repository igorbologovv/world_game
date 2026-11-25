#![warn(missing_docs)]
//! Depth-aware pond-inspired shading for rivers, lakes and the planetary ocean.

mod ocean;
mod river_surface;
mod wetlands;

use bevy::{
    asset::AssetEventSystems,
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
        river_surface::install(app);
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
            ocean::sync_material
                .after(TransformSystems::Propagate)
                .before(AssetEventSystems),
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
    ocean: Option<Task<Option<ocean::OceanSurface>>>,
    patches: Option<(Handle<ocean::OceanMaterial>, OceanPatches)>,
}

fn request(mut commands: Commands, assets: Res<WorldAssets>, heights: Res<SurfaceHeights>) {
    let pool = AsyncComputeTaskPool::get();
    let assets = assets.directory().to_path_buf();
    let heights = heights.clone();
    let ocean = pool.spawn(async move { ocean::load(&heights, &assets) });
    commands.insert_resource(WaterLoading {
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
    mut ocean_materials: ResMut<Assets<ocean::OceanMaterial>>,
) {
    let Some(mut pending) = pending else {
        return;
    };
    if let Some(ocean::OceanSurface {
        mask,
        wave_normals,
        patches,
    }) = take_ready(&mut pending.ocean)
    {
        let material = ocean_materials.add(ocean::OceanMaterial::new(
            images.add(mask),
            images.add(wave_normals),
        ));
        ocean::spawn_detail(&root, &mut commands, &mut meshes, material.clone());
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
    if pending.ocean.is_none() && pending.patches.is_none() {
        commands.remove_resource::<WaterLoading>();
    }
}
