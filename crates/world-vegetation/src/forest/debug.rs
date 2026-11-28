//! Opt-in vegetation HUD. Counts describe residency and active coverage before render culling.
use bevy::prelude::*;

use super::{fern::FernWorld, lod::VegetationSettings, streaming::ForestStream};

#[derive(Component)]
pub(super) struct Overlay;

pub(super) fn spawn(mut commands: Commands) {
    commands.spawn((
        Overlay,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(15.0),
            ..default()
        },
        TextColor(Color::srgb(0.88, 1.0, 0.85)),
        BackgroundColor(Color::srgba(0.022, 0.054, 0.032, 0.88)),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(18.0),
            top: Val::Px(39.0),
            padding: UiRect::all(Val::Px(6.0)),
            display: Display::None,
            ..default()
        },
    ));
}

pub(super) fn update(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    forest: Res<ForestStream>,
    fern: Res<FernWorld>,
    settings: Res<VegetationSettings>,
    mut text: Query<(&mut Node, &mut Text), With<Overlay>>,
) {
    let Ok((mut style, mut text)) = text.single_mut() else {
        return;
    };
    let toggled = keys.just_pressed(KeyCode::F8);
    if toggled {
        style.display = if style.display == Display::None {
            Display::Flex
        } else {
            Display::None
        };
    }
    if style.display == Display::None {
        return;
    }
    *elapsed += time.delta_secs();
    if !toggled && *elapsed < 0.25 {
        return;
    }
    *elapsed = 0.0;
    let data = &forest.stats;
    let fern = &fern.metrics;
    text.0 = format!(
        "Vegetation  [F8]\nLOD 0 / 1 / 2   {} / {} / {}    Blends {}\nResident triangles {}    Nodes {}\nPlacement queue {}    Placement jobs {}    Forest {:.0} m\nWoodland fern ({})  LOD 0 / 1 / 2 / 3   {} / {} / {} / {}\nFern blends {}    Chunks {}    Card batches {}    Waiting {}",
        data.levels[0], data.levels[1], data.levels[2], data.blends,
        data.triangles, data.nodes, data.unsampled, data.jobs, settings.forest_range,
        fern.status, fern.levels[0], fern.levels[1], fern.levels[2], fern.levels[3],
        fern.blends, fern.chunks, fern.batches, fern.waiting,
    );
}
