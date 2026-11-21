#![warn(missing_docs)]
//! Lightweight on-screen CPU frame pacing readout for development builds.
//! Bevy's FPS/frametime diagnostics are frame-loop measurements, not GPU timings.
use bevy::{
    diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin},
    prelude::*,
};

/// Register Bevy frame-time metrics and the on-screen readout.
///
/// Install with Bevy's diagnostics plugin, which provides the metric store.
pub struct FrameStatsPlugin;

#[derive(Component)]
struct FrameStatsText;

impl Plugin for FrameStatsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(FrameTimeDiagnosticsPlugin)
            .add_systems(Startup, spawn_stats)
            .add_systems(Update, update_stats);
    }
}

fn spawn_stats(mut commands: Commands) {
    commands.spawn((
        FrameStatsText,
        TextBundle::from_section(
            "FPS: ... | Frame: ...",
            TextStyle {
                font_size: 15.0,
                color: Color::srgb(0.95, 0.95, 0.9),
                ..default()
            },
        )
        .with_style(Style {
            position_type: PositionType::Absolute,
            left: Val::Px(18.0),
            top: Val::Px(16.0),
            ..default()
        }),
    ));
}

fn update_stats(
    time: Res<Time>,
    mut elapsed: Local<f32>,
    store: Res<DiagnosticsStore>,
    mut readout: Query<&mut Text, With<FrameStatsText>>,
) {
    *elapsed += time.delta_seconds();
    if *elapsed < 0.5 {
        return;
    }
    *elapsed = 0.0;
    let fps = store
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|metric| metric.smoothed())
        .unwrap_or(0.0);
    let ms = store
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|metric| metric.smoothed())
        .unwrap_or(0.0);
    if let Ok(mut text) = readout.get_single_mut() {
        text.sections[0].value = format!("FPS: {:.0}   Frame: {:.1} ms", fps, ms);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::{diagnostic::DiagnosticsPlugin, time::TimeUpdateStrategy};

    use super::*;

    #[test]
    fn readout_displays_the_frame_time_and_fps() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, DiagnosticsPlugin, FrameStatsPlugin))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )));
        for _ in 0..8 {
            app.update();
        }

        let world = app.world_mut();
        let text = world
            .query_filtered::<&Text, With<FrameStatsText>>()
            .single(world);
        assert_eq!(text.sections[0].value, "FPS: 10   Frame: 100.0 ms");
    }
}
