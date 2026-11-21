use bevy::{prelude::*, ui::FocusPolicy};
use crate::camera::DebugCameraController;

use crate::player::Player;

const BASE_SPEED: f64 = 7.0;
const MULTIPLIERS: [f64; 10] = [0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0];

#[derive(Resource)]
pub(super) struct TravelSpeed(usize);

impl Default for TravelSpeed {
    fn default() -> Self {
        Self(1)
    }
}

impl TravelSpeed {
    pub(super) fn meters_per_second(&self) -> f64 {
        BASE_SPEED * MULTIPLIERS[self.0]
    }

    fn adjust(&mut self, change: isize) {
        self.0 = self
            .0
            .saturating_add_signed(change)
            .min(MULTIPLIERS.len() - 1);
    }
}

#[derive(Component)]
pub(super) struct TripPanelInput;

#[derive(Component)]
pub(super) struct SpeedButton(isize);

#[derive(Component)]
pub(super) struct SpeedReadout;

#[derive(Component)]
pub(super) struct TripReadout;

pub(super) fn spawn(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(16.0),
                right: Val::Px(16.0),
                padding: UiRect::all(Val::Px(12.0)),
                width: Val::Px(258.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(Color::srgba(0.06, 0.10, 0.14, 0.93)),
            FocusPolicy::Block,
            Interaction::None,
            TripPanelInput,
        ))
        .with_children(|panel| {
            panel.spawn((label("No destination", 16.0), TripReadout));
            panel.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|controls| {
                speed_button(controls, "−", -1);
                controls.spawn((label("1× · 7 m/s", 15.0), SpeedReadout));
                speed_button(controls, "+", 1);
            });
        });
}

fn label(value: &str, font_size: f32) -> impl Bundle {
    (
        Text::new(value),
        TextFont { font_size: FontSize::Px(font_size), ..default() },
        TextColor(Color::srgb(0.92, 0.95, 0.98)),
    )
}

fn speed_button(
    builder: &mut bevy::ecs::relationship::RelatedSpawnerCommands<'_, ChildOf>,
    symbol: &str,
    adjustment: isize,
) {
    builder.spawn((
        Button::default(),
        Node {
            width: Val::Px(36.0),
            height: Val::Px(32.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(button_color(Interaction::None).0),
        SpeedButton(adjustment),
        TripPanelInput,
    ))
    .with_children(|button| {
        button.spawn(label(symbol, 19.0));
    });
}

pub(super) fn change_speed(
    keys: Res<ButtonInput<KeyCode>>,
    mut speed: ResMut<TravelSpeed>,
    mut buttons: Query<(&Interaction, &SpeedButton, &mut BackgroundColor), Changed<Interaction>>,
) {
    for (interaction, button, mut color) in &mut buttons {
        *color = button_color(*interaction);
        if *interaction == Interaction::Pressed {
            speed.adjust(button.0);
        }
    }
    if keys.any_just_pressed([KeyCode::Minus, KeyCode::NumpadSubtract]) {
        speed.adjust(-1);
    }
    if keys.any_just_pressed([KeyCode::Equal, KeyCode::NumpadAdd]) {
        speed.adjust(1);
    }
}

fn button_color(interaction: Interaction) -> BackgroundColor {
    BackgroundColor(match interaction {
        Interaction::Pressed => Color::srgb(0.16, 0.34, 0.41),
        Interaction::Hovered => Color::srgb(0.21, 0.36, 0.44),
        Interaction::None => Color::srgb(0.12, 0.23, 0.30),
    })
}

pub(super) fn update(
    player: Query<&Player>,
    camera: Query<&DebugCameraController>,
    speed: Res<TravelSpeed>,
    mut trip_text: Query<&mut Text, (With<TripReadout>, Without<SpeedReadout>)>,
    mut speed_text: Query<&mut Text, (With<SpeedReadout>, Without<TripReadout>)>,
) {
    let meters_per_second = speed.meters_per_second();
    let Ok(player) = player.single() else { return; };
    let trip = match player.trip() {
        Some((distance, blocked, pace)) => {
            let remaining = if distance >= 1000.0 {
                format!("{:.1} km remaining", distance / 1000.0)
            } else {
                format!("{:.0} m remaining", distance)
            };
            let status = if blocked {
                "Route blocked. Press F to fly".to_string()
            } else if camera.single().is_ok_and(|camera| camera.enabled) {
                "Trip paused".to_string()
            } else {
                format!("ETA ~{}", format_eta(distance / (meters_per_second * pace)))
            };
            format!("{status}\n{remaining}")
        }
        None => "No destination".to_string(),
    };
    let Ok(mut trip_text) = trip_text.single_mut() else { return; };
    if trip_text.0 != trip {
        trip_text.0 = trip;
    }
    if speed.is_changed() {
        if let Ok(mut text) = speed_text.single_mut() {
            text.0 = format!("{}× · {:.1} m/s", MULTIPLIERS[speed.0], meters_per_second);
        }
    }
}

fn format_eta(seconds: f64) -> String {
    let seconds = seconds.ceil() as u64;
    let minutes = seconds / 60;
    let hours = minutes / 60;
    if hours >= 24 {
        format!("{}d {}h", hours / 24, hours % 24)
    } else if hours > 0 {
        format!("{hours}h {}m", minutes % 60)
    } else if minutes > 0 {
        format!("{minutes}m {}s", seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn faster_travel_shortens_eta_and_speed_stays_in_range() {
        let mut speed = TravelSpeed::default();
        assert_eq!(format_eta(840.0 / speed.meters_per_second()), "2m 0s");
        speed.adjust(1);
        assert_eq!(format_eta(840.0 / speed.meters_per_second()), "1m 0s");
        speed.adjust(100);
        assert_eq!(speed.meters_per_second(), BASE_SPEED * 256.0);
        speed.adjust(-100);
        assert_eq!(speed.meters_per_second(), BASE_SPEED * 0.5);
    }
}
