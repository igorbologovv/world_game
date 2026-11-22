use bevy::{camera::CameraUpdateSystems, prelude::*, transform::TransformSystems, ui::UiSystems};
use world_core::runtime::WorldCamera;
use world_vegetation::CoconutPalm;

pub(super) fn install(app: &mut App) {
    app.add_systems(Startup, spawn).add_systems(
        PostUpdate,
        guide
            .after(TransformSystems::Propagate)
            .after(CameraUpdateSystems)
            .before(UiSystems::Layout),
    );
}

#[derive(Component)]
struct PalmGuide;

const LABEL_WIDTH: f32 = 146.0;
const LABEL_HEIGHT: f32 = 29.0;

fn spawn(mut commands: Commands) {
    commands.spawn((
        PalmGuide,
        Text::new("Coconut palm"),
        TextLayout::no_wrap(),
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
    cameras: Query<(&Camera, &GlobalTransform, &WorldCamera)>,
    palms: Query<&GlobalTransform, With<CoconutPalm>>,
    mut label: Query<(&mut Node, &mut Visibility), With<PalmGuide>>,
) {
    let (Ok((camera, global, mode)), Ok((mut node, mut visibility))) =
        (cameras.single(), label.single_mut())
    else {
        return;
    };
    let position = (|| {
        if *mode != WorldCamera::FirstPerson {
            return None;
        }
        let viewport = camera.logical_viewport_rect()?;
        // The marker follows the real Mars-scale instance in floating-origin
        // render space. The old 100 km landmark can be hundreds of meters off.
        palms
            .iter()
            .filter_map(|palm| {
                let distance = global.translation().distance_squared(palm.translation());
                if distance > 150.0_f32.powi(2) {
                    return None;
                }
                let point = palm.transform_point(Vec3::Y * 5.4);
                let projected = camera.world_to_viewport(global, point).ok()?;
                viewport
                    .contains(projected)
                    .then_some((distance, projected))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, point)| (point, viewport))
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
