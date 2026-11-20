use bevy::{
    input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel},
    math::{DQuat, DVec3},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_terrain::{
    big_space::{GridTransform, ReferenceFrames},
    prelude::{DebugCameraController, TerrainViewComponents, TileAtlas, TileTree},
    terrain_data::sample_height,
};

use crate::{
    surface_heights::{terrain_aligned_direction, SurfaceHeights},
    trip::{TravelSpeed, TripPanelInput},
    MAX_HEIGHT, RADIUS, STARTING_RIVER,
};

const SPAWN_ELEVATION: f64 = 494.13;
const WATER_LEVEL: f64 = 1.1;
const MIN_SLOPE_PACE: f64 = 0.15;
const CAMERA_DISTANCE: f64 = 30.0;
pub const CAMERA_HEIGHT: f64 = 55.0;
const GLOBE_RADIUS_MULTIPLIER: f64 = 3.2;
const GLOBE_DRAG_ZOOM: f64 = 0.82;
const POV_EYE_HEIGHT: f64 = 2.2;
const MAX_LOOK_PITCH: f64 = 1.48;
const LOOK_SENSITIVITY: f64 = 0.005;
const POV_FOV: f32 = std::f32::consts::FRAC_PI_3;
const MAP_FOV: f32 = std::f32::consts::FRAC_PI_4;

pub struct MapView {
    zoom: f64,
    target_zoom: f64,
    globe_center: Option<DVec3>,
    first_person: Option<FirstPersonView>,
}

impl Default for MapView {
    fn default() -> Self {
        let zoom = 0.24;
        Self {
            zoom,
            target_zoom: zoom,
            globe_center: None,
            first_person: None,
        }
    }
}

impl MapView {
    pub fn altitude(&self, aspect: f64) -> f64 {
        let half_fov = std::f64::consts::FRAC_PI_8;
        let narrow_axis = (half_fov.tan() * aspect.clamp(0.25, 1.0)).atan();
        let fit_distance = (RADIUS + f64::from(MAX_HEIGHT)) * 1.1 / narrow_axis.sin();
        let max_altitude = fit_distance.max(RADIUS * GLOBE_RADIUS_MULTIPLIER) - RADIUS;
        CAMERA_HEIGHT * (max_altitude / CAMERA_HEIGHT).powf(self.zoom)
    }

    fn globe_blend(&self) -> f64 {
        let t = ((self.zoom - 0.5) / 0.32).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    fn rotate_globe(&mut self, center: DVec3, drag: Vec2) {
        let longitude = center.z.atan2(center.x) - f64::from(drag.x) * 0.005;
        let latitude = (center.y.asin() + f64::from(drag.y) * 0.005).clamp(-1.55, 1.55);
        let (lat_sin, lat_cos) = latitude.sin_cos();
        let (lon_sin, lon_cos) = longitude.sin_cos();
        self.globe_center = Some(DVec3::new(lat_cos * lon_cos, lat_sin, lat_cos * lon_sin));
    }
}

struct FirstPersonView {
    heading: DVec3,
    pitch: f64,
}

impl FirstPersonView {
    fn look(&mut self, up: DVec3, drag: Vec2) {
        let heading = tangent(self.heading, up);
        self.heading = DQuat::from_axis_angle(up, -f64::from(drag.x) * LOOK_SENSITIVITY) * heading;
        self.pitch = (self.pitch - f64::from(drag.y) * LOOK_SENSITIVITY)
            .clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
    }

    fn pose(&mut self, player: &Player) -> (DVec3, Quat) {
        self.heading = tangent(self.heading, player.up);
        let (rise, forward) = self.pitch.sin_cos();
        let direction = self.heading * forward + player.up * rise;
        let position = player.up * (RADIUS + player.height + POV_EYE_HEIGHT);
        let rotation = Transform::IDENTITY
            .looking_to(direction.as_vec3(), player.up.as_vec3())
            .rotation;
        (position, rotation)
    }
}

#[derive(Component)]
pub struct Player {
    up: DVec3,
    forward: DVec3,
    facing: DVec3,
    destination: Option<DVec3>,
    route_blocked: bool,
    terrain_pace: f64,
    height: f64,
    grounded: bool,
    walk_phase: f32,
    moving: bool,
}

impl Default for Player {
    fn default() -> Self {
        let up = terrain_aligned_direction(DVec3::new(-0.35249159, 0.82882267, 0.43451429));
        let forward = tangent(terrain_aligned_direction(STARTING_RIVER), up);
        Self {
            up,
            forward,
            facing: forward,
            destination: None,
            route_blocked: false,
            terrain_pace: 1.0,
            height: SPAWN_ELEVATION,
            grounded: false,
            walk_phase: 0.0,
            moving: false,
        }
    }
}

impl Player {
    /// Planet-local unit vector used for deterministic forest streaming.
    pub fn surface_direction(&self) -> DVec3 {
        self.up
    }

    pub fn trip(&self) -> Option<(f64, bool, f64)> {
        self.destination.map(|target| {
            (
                self.distance_to(target),
                self.route_blocked,
                self.terrain_pace,
            )
        })
    }

    fn distance_to(&self, target: DVec3) -> f64 {
        self.up.angle_between(target) * (RADIUS + self.height)
    }

    fn advance(
        &mut self,
        mut budget: f64,
        mut ground: f64,
        ground_at: impl Fn(DVec3) -> f64,
        river_at: impl Fn(DVec3) -> bool,
    ) {
        let Some(target) = self.destination.filter(|_| !self.route_blocked) else {
            return;
        };
        while budget > 0.0 && ground > WATER_LEVEL {
            let distance = self.distance_to(target);
            if distance < 0.35 {
                self.destination = None;
                break;
            }
            let radius = RADIUS + self.height;
            let offset = target - self.up * target.dot(self.up);
            let direction = offset.try_normalize().unwrap_or(self.forward);
            let probe = distance.min(2.0);
            let ahead_up = (self.up + direction * (probe / radius)).normalize();
            let ahead = ground_at(ahead_up);
            if ahead <= WATER_LEVEL || river_at(ahead_up) {
                self.route_blocked = true;
                break;
            }
            let slope = (ahead - ground) / probe;
            self.terrain_pace = slope.hypot(1.0).recip().max(MIN_SLOPE_PACE);
            let step = (budget * self.terrain_pace).min(probe);
            let (next_up, next_height) = if step == probe {
                (ahead_up, ahead)
            } else {
                let up = (self.up + direction * (step / radius)).normalize();
                (up, ground_at(up))
            };
            if next_height <= WATER_LEVEL || (step != probe && river_at(next_up)) {
                self.route_blocked = true;
                break;
            }
            self.up = next_up;
            self.height = next_height;
            self.forward = tangent(self.forward, next_up);
            self.facing = tangent(direction, next_up);
            self.walk_phase = (self.walk_phase + step as f32 * 4.0) % std::f32::consts::TAU;
            self.moving = true;
            ground = next_height;
            budget -= step / self.terrain_pace;
        }
    }

    pub fn camera_pose(&self, eye_height: f64, map: &MapView) -> (DVec3, Quat) {
        let globe = map.globe_blend();
        let globe_center = map.globe_center.unwrap_or(self.up);
        let orbit = DQuat::IDENTITY.slerp(DQuat::from_rotation_arc(self.up, globe_center), globe);
        let radial = orbit * self.up;
        let position = radial * (RADIUS + self.height * (1.0 - globe) + eye_height)
            - self.forward * (CAMERA_DISTANCE * (1.0 - globe));
        let focus = (self.up * (RADIUS + self.height + 1.5) + self.forward * 58.0) * (1.0 - globe);
        let north = (DVec3::Y - radial * radial.y)
            .try_normalize()
            .unwrap_or(DVec3::Z);
        let up = self.up.lerp(north, globe);
        let rotation = Transform::IDENTITY
            .looking_to((focus - position).as_vec3(), up.as_vec3())
            .rotation;
        (position, rotation)
    }
}

#[derive(Component)]
pub struct Boot(f32);

#[derive(Component)]
pub struct Destination;

pub fn spawn_model(
    children: &mut ChildBuilder,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let coat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.92, 0.30, 0.09),
        unlit: true,
        ..default()
    });
    let helmet = materials.add(StandardMaterial {
        base_color: Color::srgb(0.96, 0.86, 0.53),
        unlit: true,
        ..default()
    });
    let gear = materials.add(StandardMaterial {
        base_color: Color::srgb(0.13, 0.18, 0.23),
        unlit: true,
        ..default()
    });
    let body = meshes.add(Capsule3d::new(0.37, 0.65));
    let head = meshes.add(Sphere::new(0.30));
    let boot = meshes.add(Cuboid::new(0.30, 0.60, 0.40));
    let backpack = meshes.add(Cuboid::new(0.51, 0.60, 0.28));
    for (mesh, material, location) in [
        (body, coat, Vec3::new(0.0, 1.11, 0.0)),
        (head, helmet, Vec3::new(0.0, 1.93, -0.04)),
        (backpack, gear.clone(), Vec3::new(0.0, 1.22, 0.35)),
    ] {
        children.spawn(PbrBundle {
            mesh,
            material,
            transform: Transform::from_translation(location),
            ..default()
        });
    }
    for side in [-1.0, 1.0] {
        children.spawn((
            Boot(side),
            PbrBundle {
                mesh: boot.clone(),
                material: gear.clone(),
                transform: Transform::from_xyz(0.19 * side, 0.30, 0.0),
                ..default()
            },
        ));
    }
}

pub fn walk(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut wheel: EventReader<MouseWheel>,
    mut motion: EventReader<MouseMotion>,
    mut map: Local<MapView>,
    time: Res<Time>,
    frames: ReferenceFrames,
    terrains: Query<(Entity, &TileAtlas)>,
    trees: Res<TerrainViewComponents<TileTree>>,
    surface: Res<SurfaceHeights>,
    speed: Res<TravelSpeed>,
    trip_panel: Query<&Interaction, With<TripPanelInput>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut avatars: Query<
        (GridTransform, &mut Player, &mut Visibility),
        (Without<DebugCameraController>, Without<Destination>),
    >,
    mut cameras: Query<
        (
            Entity,
            GridTransform,
            &DebugCameraController,
            &Camera,
            &GlobalTransform,
            &mut Projection,
        ),
        (Without<Player>, Without<Destination>),
    >,
    mut markers: Query<
        (GridTransform, &mut Visibility),
        (
            With<Destination>,
            Without<Player>,
            Without<DebugCameraController>,
        ),
    >,
) {
    let (camera_entity, mut camera, debug, view, camera_global, mut projection) =
        cameras.single_mut();
    let (mut avatar, mut player, mut visibility) = avatars.single_mut();
    let (mut marker, mut marker_visibility) = markers.single_mut();
    let scroll = wheel
        .read()
        .map(|event| match event.unit {
            MouseScrollUnit::Line => f64::from(event.y),
            MouseScrollUnit::Pixel => f64::from(event.y) / 60.0,
        })
        .sum::<f64>();
    let drag: Vec2 = motion.read().map(|event| event.delta).sum();
    let mut window = windows.single_mut();
    window.cursor.visible = !debug.enabled;
    player.moving = false;
    if debug.enabled || keys.just_pressed(KeyCode::KeyT) {
        if player.grounded {
            *visibility = Visibility::Inherited;
        }
        *marker_visibility = Visibility::Hidden;
        set_fov(&mut projection, MAP_FOV);
        return;
    }

    let escape = keys.just_pressed(KeyCode::Escape);
    let switched_view = keys.just_pressed(KeyCode::KeyV) || (escape && map.first_person.is_some());
    if switched_view {
        map.first_person = if map.first_person.is_none() {
            Some(FirstPersonView {
                heading: player.facing,
                pitch: -0.10,
            })
        } else {
            None
        };
    } else if escape {
        player.destination = None;
    }

    let aspect = f64::from(window.width()) / f64::from(window.height().max(1.0));
    let first_person = map.first_person.is_some();
    let rotation_drag = mouse.pressed(MouseButton::Right) && drag != Vec2::ZERO;
    if let Some(pov) = &mut map.first_person {
        if rotation_drag {
            pov.look(player.up, drag);
        }
    } else {
        map.target_zoom = (map.target_zoom - scroll * 0.10).clamp(0.0, 1.0);
        let zoom_ease = 1.0 - (-18.0 * time.delta_seconds_f64()).exp();
        map.zoom += (map.target_zoom - map.zoom) * zoom_ease;
        if (map.target_zoom - map.zoom).abs() < 0.0001 {
            map.zoom = map.target_zoom;
        }
        if map.zoom <= 0.5 {
            map.globe_center = None;
            if rotation_drag {
                player.forward =
                    DQuat::from_axis_angle(player.up, -f64::from(drag.x) * LOOK_SENSITIVITY)
                        * player.forward;
            }
        } else if map.zoom >= GLOBE_DRAG_ZOOM && rotation_drag {
            let center = map.globe_center.unwrap_or(player.up);
            map.rotate_globe(center, drag);
        }
    }

    let (terrain_entity, atlas) = terrains.single();
    let Some(tree) = trees.get(&(terrain_entity, camera_entity)) else {
        return;
    };
    let frame = frames.parent_frame(camera_entity).unwrap();
    let ground_at = |up: DVec3| {
        surface
            .sample(up)
            .unwrap_or_else(|| f64::from(sample_height(tree, atlas, up * RADIUS)))
    };
    let ground = ground_at(player.up);
    if ground > WATER_LEVEL {
        if !player.grounded {
            info!("Explorer spawned at {ground:.1} m above sea level");
        }
        player.height = ground;
        player.grounded = true;
    }
    *visibility = if player.grounded && !first_person {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };

    if !switched_view
        && mouse.just_pressed(MouseButton::Left)
        && trip_panel
            .iter()
            .all(|interaction| *interaction == Interaction::None)
    {
        let ray = window
            .cursor_position()
            .and_then(|cursor| view.viewport_to_world(camera_global, cursor));
        if let Some(ray) = ray {
            let origin = frame.grid_position_double(&camera.cell, &camera.transform)
                + (ray.origin - camera_global.translation()).as_dvec3();
            if let Some(target) = pick_land(origin, ray.direction.as_dvec3(), ground_at)
                .filter(|target| !surface.is_river(*target))
            {
                player.destination = Some(target);
                player.route_blocked = false;
                player.terrain_pace = 1.0;
            }
        }
    }

    player.advance(
        speed.meters_per_second() * time.delta_seconds_f64(),
        ground,
        ground_at,
        |up| surface.is_river(up),
    );

    let position = player.up * (RADIUS + player.height + 0.38);
    let (cell, translation) = frame.translation_to_grid(position);
    *avatar.cell = cell;
    avatar.transform.translation = translation;
    avatar
        .transform
        .look_to(player.facing.as_vec3(), player.up.as_vec3());

    let (position, rotation) = if let Some(pov) = &mut map.first_person {
        set_fov(&mut projection, POV_FOV);
        pov.pose(&player)
    } else {
        set_fov(&mut projection, MAP_FOV);
        let camera_up =
            (player.up * (RADIUS + player.height) - player.forward * CAMERA_DISTANCE).normalize();
        let eye_height = map
            .altitude(aspect)
            .max(ground_at(camera_up) - player.height + 5.0);
        player.camera_pose(eye_height, &map)
    };
    *marker_visibility = Visibility::Hidden;
    if let Some(target) = player.destination {
        let marker_ground = target * (RADIUS + ground_at(target));
        let scale = (position.distance(marker_ground) / 70.0).max(1.0);
        let marker_position = marker_ground + target * (0.75 * scale);
        let (cell, translation) = frame.translation_to_grid(marker_position);
        *marker.cell = cell;
        marker.transform.translation = translation;
        marker.transform.rotation =
            Quat::from_rotation_arc(Vec3::Y, (position - marker_position).normalize().as_vec3());
        marker.transform.scale = Vec3::splat(scale as f32);
        *marker_visibility = Visibility::Inherited;
    }

    let (cell, translation) = frame.translation_to_grid(position);
    *camera.cell = cell;
    camera.transform.translation = translation;
    camera.transform.rotation = rotation;
}

fn set_fov(projection: &mut Mut<Projection>, fov: f32) {
    if let Projection::Perspective(perspective) = &**projection {
        if perspective.fov == fov {
            return;
        }
    }
    if let Projection::Perspective(perspective) = &mut **projection {
        perspective.fov = fov;
    }
}

pub fn animate(player: Query<&Player>, mut boots: Query<(&Boot, &mut Transform)>) {
    let player = player.single();
    for (boot, mut transform) in &mut boots {
        let swing = if player.moving {
            player.walk_phase.sin() * boot.0
        } else {
            0.0
        };
        transform.translation.z = swing * 0.24;
        transform.translation.y = 0.30 + swing.max(0.0) * 0.16;
    }
}

fn pick_land(origin: DVec3, direction: DVec3, height: impl Fn(DVec3) -> f64) -> Option<DVec3> {
    let closest = -origin.dot(direction);
    let impact = (origin + direction * closest).length_squared();
    let outer = RADIUS + f64::from(MAX_HEIGHT);
    if impact >= outer * outer {
        return None;
    }
    let outer_crossing = (outer * outer - impact).sqrt();
    let start = (closest - outer_crossing).max(0.0);
    let sea_level = RADIUS + WATER_LEVEL;
    let end = if impact <= sea_level * sea_level {
        closest - (sea_level * sea_level - impact).sqrt()
    } else {
        closest + outer_crossing
    };
    if end < start {
        return None;
    }

    let steps = (((end - start) / 100.0).ceil() as usize).clamp(32, 512);
    let mut previous = start;
    for index in 1..=steps {
        let distance = start + (end - start) * index as f64 / steps as f64;
        let point = origin + direction * distance;
        if point.length() <= RADIUS + height(point.normalize()).max(WATER_LEVEL) + 0.001 {
            let mut low = previous;
            let mut high = distance;
            for _ in 0..14 {
                let mid = (low + high) * 0.5;
                let sample = origin + direction * mid;
                if sample.length() > RADIUS + height(sample.normalize()).max(WATER_LEVEL) {
                    low = mid;
                } else {
                    high = mid;
                }
            }
            let hit = (origin + direction * high).normalize();
            return (height(hit) > WATER_LEVEL).then_some(hit);
        }
        previous = distance;
    }
    None
}

fn tangent(direction: DVec3, up: DVec3) -> DVec3 {
    (direction - up * direction.dot(up)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_person_look_rotates_and_keeps_the_eye_above_ground() {
        let player = Player::default();
        let mut pov = FirstPersonView {
            heading: player.facing,
            pitch: 0.0,
        };
        let (_, before) = pov.pose(&player);
        pov.look(player.up, Vec2::new(140.0, -5_000.0));
        let (eye, after) = pov.pose(&player);
        assert!((eye.length() - RADIUS - player.height - POV_EYE_HEIGHT).abs() < 1e-8);
        assert_eq!(pov.pitch, MAX_LOOK_PITCH);
        assert!((before * Vec3::NEG_Z).dot(after * Vec3::NEG_Z) < 0.5);
        assert!((after * Vec3::NEG_Z).dot(player.up.as_vec3()) > 0.9);
        assert!(after.is_finite());
    }

    #[test]
    fn picks_the_visible_hemisphere_from_globe_view() {
        let origin = DVec3::X * (RADIUS * GLOBE_RADIUS_MULTIPLIER);
        let target = pick_land(origin, -DVec3::X, |_| 1_000.0).unwrap();
        assert!(target.abs_diff_eq(DVec3::X, 1e-9));
    }

    #[test]
    fn steep_land_can_be_climbed_at_a_slower_pace() {
        let mut player = Player::default();
        let start = player.up;
        player.destination = Some((start + player.forward * (40.0 / RADIUS)).normalize());
        let slope = |up| SPAWN_ELEVATION + 4.0 * start.angle_between(up) * RADIUS;

        player.advance(20.0, SPAWN_ELEVATION, slope, |_| false);
        let climbed = start.angle_between(player.up) * RADIUS;
        assert!(climbed > 4.0 && climbed < 7.0);
        assert!(player.terrain_pace < 0.3);
        assert!(!player.route_blocked);

        player.advance(300.0, slope(player.up), slope, |_| false);
        assert!(player.destination.is_none());
        assert!(player.height > SPAWN_ELEVATION + 150.0);
    }

    #[test]
    fn high_speed_does_not_skip_a_water_obstacle() {
        let mut player = Player::default();
        let start = player.up;
        player.destination = Some((start + player.forward * (50.0 / RADIUS)).normalize());
        let ground_at = |up| {
            let distance = start.angle_between(up) * RADIUS;
            if (10.0..14.0).contains(&distance) {
                -1.0
            } else {
                SPAWN_ELEVATION
            }
        };
        player.advance(100.0, SPAWN_ELEVATION, ground_at, |_| false);
        assert!(player.route_blocked);
        assert!(start.angle_between(player.up) * RADIUS < 10.0);
        assert!(player.destination.is_some());
    }

    #[test]
    fn river_blocks_a_trip_even_above_sea_level() {
        let mut player = Player::default();
        let start = player.up;
        player.destination = Some((start + player.forward * (50.0 / RADIUS)).normalize());
        let river_at = |up| (10.0..14.0).contains(&(start.angle_between(up) * RADIUS));
        player.advance(100.0, SPAWN_ELEVATION, |_| SPAWN_ELEVATION, river_at);
        assert!(player.route_blocked);
        assert!(start.angle_between(player.up) * RADIUS < 10.0);
    }

    #[test]
    fn rejects_water_in_front_of_land_and_rays_that_miss() {
        let origin = DVec3::X * (RADIUS * GLOBE_RADIUS_MULTIPLIER);
        assert!(pick_land(origin, -DVec3::X, |up| if up.x > 0.0 {
            -100.0
        } else {
            2_000.0
        })
        .is_none());
        assert!(pick_land(origin, DVec3::Y, |_| 1_000.0).is_none());
    }
}
