use bevy::{gltf::GltfAssetLabel, math::DVec3, prelude::*};
use world_core::{
    coordinates::terrain_aligned_direction,
    planet::RADIUS,
    runtime::{PlanetRoot, ReferenceFrame},
    SurfaceHeights,
};
use world_regions::STARTER_REGION;
use world_vegetation::ForestClearing;

const SCENE: &str = "models/starter/low_poly_tree_scene/scene.glb";
const DISTANCE_AHEAD: f64 = 39.0;
const SCENE_SCALE: f64 = 0.72;
const HALF_WIDTH: f64 = 44.0 * SCENE_SCALE;
const HALF_DEPTH: f64 = 28.0 * SCENE_SCALE;
const POND_BED: f64 = -4.0 * SCENE_SCALE;
const TREE_CLEARANCE: f64 = 48.0;

pub(super) fn spawn(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    frames: Query<&ReferenceFrame>,
    terrain: Res<SurfaceHeights>,
    assets: Res<AssetServer>,
) {
    let frame = frames
        .get(root.entity())
        .expect("WorldReady creates the planet reference frame");
    let start = STARTER_REGION.spawn;
    let radial = terrain_aligned_direction(start.direction);
    let ahead = terrain_aligned_direction(start.look_at);
    let heading = (ahead - radial * radial.dot(ahead)).normalize();
    let center_direction = (radial * RADIUS + heading * DISTANCE_AHEAD).normalize();
    commands.insert_resource(ForestClearing {
        direction: center_direction,
        radius: TREE_CLEARANCE,
    });
    let center = surface_position(&terrain, center_direction, start.elevation);
    let forward = (heading - center_direction * center_direction.dot(heading)).normalize();
    let right = forward.cross(center_direction);

    // Follow the local terrain grade so the perimeter of the authored ground
    // meets the landscape. Keep the pond bed above the baked terrain.
    let sample =
        |offset: DVec3| surface_position(&terrain, (center + offset).normalize(), start.elevation);
    let across = sample(right * 10.0) - sample(-right * 10.0);
    let along = sample(forward * 10.0) - sample(-forward * 10.0);
    let up = across.cross(along).normalize();
    let mut clearance = 0.0_f64;
    for x in [-1.0, -0.5, 0.0, 0.5, 1.0] {
        for z in [-1.0, -0.5, 0.0, 0.5, 1.0] {
            let offset = right * (x * HALF_WIDTH) + forward * (z * HALF_DEPTH);
            clearance = clearance.max((sample(offset) - center).dot(up));
        }
    }
    let anchor = center + up * (clearance - POND_BED + 0.10);
    let (cell, translation) = frame.translation_to_grid(anchor);
    let transform = Transform::from_translation(translation)
        .looking_to(along.as_vec3(), up.as_vec3())
        .with_scale(Vec3::splat(SCENE_SCALE as f32));
    root.spawn(
        &mut commands,
        (
            Name::new("Low Poly Tree Scene Free"),
            cell,
            SceneBundle {
                scene: assets.load(GltfAssetLabel::Scene(0).from_asset(SCENE)),
                transform,
                ..default()
            },
        ),
    );
}

fn surface_position(terrain: &SurfaceHeights, direction: DVec3, fallback: f64) -> DVec3 {
    direction * (RADIUS + terrain.sample(direction).unwrap_or(fallback))
}
