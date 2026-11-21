//! Opt-in vegetation HUD. Counts describe residency and active coverage before render culling.
use bevy::prelude::*;

use super::{fern::FernWorld, lod::VegetationSettings, streaming::ForestStream};

#[derive(Resource, Default)]
pub(super) struct LodDiagnostics {
    pub levels: [usize; 3],
    pub blends: usize,
}

#[derive(Component)]
pub(super) struct Overlay;

pub(super) fn spawn(mut commands: Commands) {
    let mut text = TextBundle::from_section(
        "",
        TextStyle {
            font_size: 15.0,
            color: Color::srgb(0.88, 1.0, 0.85),
            ..default()
        },
    )
    .with_style(Style {
        position_type: PositionType::Absolute,
        left: Val::Px(18.0),
        top: Val::Px(39.0),
        padding: UiRect::all(Val::Px(6.0)),
        display: Display::None,
        ..default()
    });
    text.background_color = BackgroundColor(Color::srgba(0.022, 0.054, 0.032, 0.88));
    commands.spawn((Overlay, text));
}

pub(super) fn update(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut elapsed: Local<f32>,
    forest: Res<ForestStream>,
    fern: Res<FernWorld>,
    lods: Res<LodDiagnostics>,
    settings: Res<VegetationSettings>,
    mut text: Query<(&mut Style, &mut Text), With<Overlay>>,
) {
    let Ok((mut style, mut text)) = text.get_single_mut() else {
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
    *elapsed += time.delta_seconds();
    if !toggled && *elapsed < 0.25 {
        return;
    }
    *elapsed = 0.0;
    let data = &forest.stats;
    let fern = &fern.metrics;
    text.sections[0].value = format!(
        "Vegetation  [F8]\nLOD 0 / 1 / 2   {} / {} / {}    Blends {}\nImpostor batches {}    Forest batches {}    Trees resident {}\nNodes {}    Placement queue {}    Mesh jobs {}    Retiring {}\nImpostors {:.0} m    Forest {:.0} m\nWoodland fern ({})  LOD 0 / 1 / 2 / 3   {} / {} / {} / {}\nFern blends {}    Chunks {}    Card batches {}    Waiting {}",
        lods.levels[0], lods.levels[1], lods.levels[2], lods.blends,
        data.impostors, data.parents, data.trees, data.nodes, data.unsampled, data.jobs, data.retirement_queue,
        settings.impostor_range, settings.forest_range,
        fern.status, fern.levels[0], fern.levels[1], fern.levels[2], fern.levels[3],
        fern.blends, fern.chunks, fern.batches, fern.waiting,
    );
}
