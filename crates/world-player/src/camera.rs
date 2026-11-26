//! Game-specific free camera controller: no dependency on the old terrain renderer.
use bevy::{input::mouse::MouseMotion, math::DVec3, prelude::*};
use world_core::runtime::{GridTransform, ReferenceFrames};

#[derive(Component)]
pub(super) struct DebugCameraController {
    pub enabled: bool,
    pub translation_speed: f64,
    pub translation_velocity: DVec3,
    pub rotation_velocity: Vec2,
}

impl Default for DebugCameraController {
    fn default() -> Self {
        Self {
            enabled: false,
            translation_speed: 300.0,
            translation_velocity: DVec3::ZERO,
            rotation_velocity: Vec2::ZERO,
        }
    }
}

pub(super) fn camera_controller(
    frames: ReferenceFrames,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut mouse: MessageReader<MouseMotion>,
    mut cameras: Query<(Entity, GridTransform, &mut DebugCameraController)>,
) {
    let Ok((entity, mut position, mut controller)) = cameras.single_mut() else {
        return;
    };
    if keys.just_pressed(KeyCode::KeyT) {
        controller.enabled = !controller.enabled;
    }
    if !controller.enabled {
        controller.translation_velocity = DVec3::ZERO;
        controller.rotation_velocity = Vec2::ZERO;
        return;
    }
    let Some(grid) = frames.parent_frame(entity) else {
        return;
    };
    let x =
        (keys.pressed(KeyCode::ArrowRight) as i32 - keys.pressed(KeyCode::ArrowLeft) as i32) as f64;
    let y = (keys.pressed(KeyCode::PageUp) as i32 - keys.pressed(KeyCode::PageDown) as i32) as f64;
    let z =
        (keys.pressed(KeyCode::ArrowDown) as i32 - keys.pressed(KeyCode::ArrowUp) as i32) as f64;
    let delta = mouse.read().map(|m| -m.delta).sum::<Vec2>();
    let dt = time.delta_secs_f64().min(0.1);
    let speed = controller.translation_speed;
    let target = position.transform.rotation.as_dquat()
        * DVec3::new(x, y, z).normalize_or_zero()
        * speed
        * dt;
    controller.translation_velocity = controller.translation_velocity.lerp(target, 0.20);
    controller.rotation_velocity = controller
        .rotation_velocity
        .lerp(delta * (0.1 * dt as f32), 0.20);

    let (cell, local) = grid.translation_to_grid(controller.translation_velocity);
    *position.cell += cell;
    position.transform.translation += local;
    let (yaw, pitch, _) = position.transform.rotation.to_euler(EulerRot::YXZ);
    position.transform.rotation = Quat::from_euler(
        EulerRot::YXZ,
        yaw + controller.rotation_velocity.x,
        (pitch + controller.rotation_velocity.y)
            .clamp(-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2),
        0.0,
    );
}
