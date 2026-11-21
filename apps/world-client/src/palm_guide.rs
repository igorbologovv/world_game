use bevy::{
    camera::CameraUpdateSystems, math::DVec3, prelude::*, transform::TransformSystems,
    ui::UiSystems,
};
use world_core::{
    coordinates::terrain_aligned_direction,
    planet::RADIUS,
    runtime::{GridTransform, ReferenceFrames, WorldCamera, WorldReady},
    SurfaceHeights,
};
use world_regions::PALM_COAST_REGION;

pub(super) fn install(app: &mut App) {
    app.add_systems(Startup, spawn.after(WorldReady))
        .add_systems(
            PostUpdate,
            guide
                .after(TransformSystems::Propagate)
                .after(CameraUpdateSystems)
                .before(UiSystems::Layout),
        );
}

#[derive(Component)]
struct PalmGuide(DVec3);

const LABEL_WIDTH: f32 = 130.0;
const LABEL_HEIGHT: f32 = 29.0;

fn spawn(mut commands: Commands, heights: Res<SurfaceHeights>) {
    // The authored heading points at a canonical streamed palm site.
    let up = terrain_aligned_direction(PALM_COAST_REGION.spawn.look_at);
    let Some(ground) = heights.sample(up) else {
        return;
    };
    commands.spawn((
        PalmGuide(up * (RADIUS + ground + 5.4)),
        Text::new("Coconut palm"),
        TextFont {
            font_size: FontSize::Px(16.0),
            ..default()
        },
        TextColor(Color::WHITE),
        Node {
            position_type: PositionType::Absolute,
            width: Val::Px(LABEL_WIDTH),
            height: Val::Px(LABEL_HEIGHT),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(4.0)),
            border: UiRect::left(Val::Px(3.0)),
            ..default()
        },
        BorderColor::all(Color::srgb(0.98, 0.77, 0.28)),
        BackgroundColor(Color::srgba(0.06, 0.11, 0.13, 0.88)),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

fn guide(
    frames: ReferenceFrames,
    cameras: Query<(
        Entity,
        &Camera,
        &GlobalTransform,
        GridTransform,
        &WorldCamera,
    )>,
    mut label: Query<(&PalmGuide, &mut Node, &mut Visibility)>,
) {
    let (Ok((entity, camera, global, grid, mode)), Ok((palm, mut node, mut visibility))) =
        (cameras.single(), label.single_mut())
    else {
        return;
    };
    let position = (|| {
        if *mode != WorldCamera::FirstPerson {
            return None;
        }
        let frame = frames.parent_frame(entity)?;
        let offset = palm.0 - frame.grid_position_double(&grid.cell, &grid.transform);
        if offset.length_squared() > 90.0_f64.powi(2) {
            return None;
        }
        let point = camera
            .world_to_viewport(global, global.translation() + offset.as_vec3())
            .ok()?;
        let viewport = camera.logical_viewport_rect()?;
        viewport.contains(point).then_some((point, viewport))
    })();
    if let Some((point, viewport)) = position {
        let limit = (viewport.max - Vec2::new(LABEL_WIDTH, LABEL_HEIGHT)).max(viewport.min);
        let origin = (point + Vec2::new(12.0, -LABEL_HEIGHT * 0.5)).clamp(viewport.min, limit);
        let (left, top) = (Val::Px(origin.x), Val::Px(origin.y));
        if (node.left, node.top) != (left, top) {
            node.left = left;
            node.top = top;
        }
        visibility.set_if_neq(Visibility::Inherited);
    } else {
        visibility.set_if_neq(Visibility::Hidden);
    }
}
