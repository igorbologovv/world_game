//! Analytical ephemeris expressed in the planet-fixed gameplay frame.

use std::f64::consts::{PI, TAU};

use bevy::{
    asset::RenderAssetUsages,
    image::{ImageFilterMode, ImageSampler, ImageSamplerDescriptor},
    light::{light_consts::lux, NotShadowCaster, Skybox, SunDisk},
    math::{DQuat, DVec3},
    pbr::MaterialPlugin,
    prelude::*,
    reflect::TypePath,
    render::render_resource::{
        AsBindGroup, Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor,
        TextureViewDimension,
    },
    shader::ShaderRef,
};
use world_core::{
    planet::RADIUS,
    runtime::{CameraMotion, GridCell, PlanetRoot, ReferenceFrame, WorldCamera, WorldReady},
};

const DAY: f64 = 86_400.0;
const YEAR: f64 = 365.25 * DAY;
const STAR_DISTANCE: f64 = 149.6e9;
// Planet-fixed +Y at the epoch is the spin axis; all orbit angles are radians.
const AXIAL_TILT: f64 = 23.5 * PI / 180.0;
const RATES: [f64; 4] = [1.0, 60.0, 600.0, 3_600.0];
const MOONS: [MoonOrbit; 2] = [
    MoonOrbit {
        radius: 18.0e6,
        body: 150_000.0,
        period: 4.0 * DAY,
        tilt: 5.0 * PI / 180.0,
        node: 0.0,
        phase: -0.1,
        lux: 7600.0,
    },
    MoonOrbit {
        radius: 31.0e6,
        body: 105_000.0,
        period: 11.0 * DAY,
        tilt: 12.0 * PI / 180.0,
        node: 70.0 * PI / 180.0,
        phase: -1.65,
        lux: 2000.0,
    },
];

#[derive(Component)]
pub(super) struct PrimarySun;

#[derive(Component)]
struct Moon(usize);

#[derive(Component)]
struct Moonlight(usize);

#[derive(Resource, Default)]
struct Clock {
    seconds: f64,
    rate: usize,
    paused: bool,
}

#[derive(Resource)]
struct Stars(Handle<Image>);

#[derive(Asset, TypePath, AsBindGroup, Clone)]
struct MoonSurface {
    #[uniform(0)]
    sun: Vec4,
}

impl Material for MoonSurface {
    fn fragment_shader() -> ShaderRef {
        "shaders/moon.wesl".into()
    }
}

struct MoonOrbit {
    radius: f64,
    body: f32,
    period: f64,
    tilt: f64,
    node: f64,
    phase: f64,
    lux: f32,
}

impl MoonOrbit {
    fn orbit_tilt(&self) -> DQuat {
        let normal = ecliptic_normal();
        let reference = ecliptic_reference();
        let node = DQuat::from_axis_angle(normal, self.node) * reference;
        DQuat::from_axis_angle(node, self.tilt)
    }

    fn inertial_position(&self, seconds: f64) -> DVec3 {
        // Positive right-handed rotation about the ecliptic normal is
        // prograde, matching the spin of the planet about +Y.
        let position =
            DQuat::from_axis_angle(ecliptic_normal(), angle(seconds, self.period) + self.phase)
                * ecliptic_reference();
        self.orbit_tilt() * position * self.radius
    }
}

fn angle(seconds: f64, period: f64) -> f64 {
    TAU * (seconds.rem_euclid(period) / period)
}

fn inertial_to_surface(seconds: f64) -> DQuat {
    // Rotate the sky into the surface frame instead of spinning the terrain
    // grid, which also contains the explorer, ocean and floating origin.
    DQuat::from_rotation_y(-angle(seconds, DAY))
}

// Preserve the previous sun's horizontal heading at the epoch. Start at
// northern summer solstice, so its declination is exactly the axial tilt.
fn solstice_heading() -> DVec3 {
    DVec3::new(-0.78, 0.0, -0.18).normalize()
}

fn ecliptic_normal() -> DVec3 {
    let (sin, cos) = AXIAL_TILT.sin_cos();
    (DVec3::Y * cos - solstice_heading() * sin).normalize()
}

// Zero longitude in the orbital plane, chosen near the old moon epoch.
fn ecliptic_reference() -> DVec3 {
    let normal = ecliptic_normal();
    (DVec3::X - normal * normal.dot(DVec3::X)).normalize()
}

fn star_direction(seconds: f64) -> DVec3 {
    let (sin, cos) = AXIAL_TILT.sin_cos();
    let initial = solstice_heading() * cos + DVec3::Y * sin;
    let star_to_planet = DQuat::from_axis_angle(ecliptic_normal(), angle(seconds, YEAR))
        * (-initial * STAR_DISTANCE);
    -star_to_planet.normalize()
}

fn light_rotation(direction: DVec3) -> Quat {
    Transform::IDENTITY
        .looking_to(-direction.as_vec3(), Vec3::Y)
        .rotation
}

fn reflected_fraction(sun: DVec3, moon: DVec3) -> f32 {
    // Lambert sphere phase function. The dot product is -1 at full moon.
    let phase = (-sun.dot(moon)).clamp(-1.0, 1.0).acos();
    ((phase.sin() + (PI - phase) * phase.cos()) / PI) as f32
}

pub(super) fn install(app: &mut App) {
    app.add_plugins(MaterialPlugin::<MoonSurface>::default())
        .init_resource::<Clock>()
        .add_systems(Startup, spawn.after(WorldReady))
        .add_systems(Update, (advance_clock, place_celestials).chain())
        .add_systems(PostUpdate, update_skybox.after(CameraMotion));
}

fn spawn(
    mut commands: Commands,
    root: Res<PlanetRoot>,
    grids: Query<&ReferenceFrame>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<MoonSurface>>,
    mut images: ResMut<Assets<Image>>,
) {
    let sun = star_direction(0.0);
    commands.spawn((
        Name::new("Sun"),
        PrimarySun,
        DirectionalLight {
            illuminance: lux::RAW_SUNLIGHT * 0.6,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_rotation(light_rotation(sun)),
        SunDisk::EARTH,
    ));

    let grid = grids.get(root.entity()).unwrap();
    let sphere = meshes.add(Sphere::new(1.0).mesh().ico(5).unwrap());
    for (index, orbit) in MOONS.iter().enumerate() {
        let position = orbit.inertial_position(0.0);
        let (cell, translation) = grid.translation_to_grid(position);
        root.spawn(
            &mut commands,
            (
                Name::new(format!("Moon {}", index + 1)),
                Moon(index),
                cell,
                Transform::from_translation(translation).with_scale(Vec3::splat(orbit.body)),
                Mesh3d(sphere.clone()),
                MeshMaterial3d(materials.add(MoonSurface {
                    sun: sun.as_vec3().extend(0.0),
                })),
                NotShadowCaster,
            ),
        );

        commands.spawn((
            Name::new(format!("Moonlight {}", index + 1)),
            Moonlight(index),
            DirectionalLight {
                color: Color::srgb(0.79, 0.86, 1.0),
                ..default()
            },
            // The mesh renders the disk. Bevy still uses its angular size
            // to attenuate the reflected light at the horizon.
            SunDisk {
                angular_size: 2.0 * orbit.body / orbit.radius as f32,
                intensity: 0.0,
            },
            Transform::from_rotation(light_rotation(position.normalize())),
        ));
    }
    commands.insert_resource(Stars(images.add(starfield())));
}

fn advance_clock(
    time: Res<Time>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mut clock: ResMut<Clock>,
) {
    if let Some(keys) = keys {
        let pause = keys.just_pressed(KeyCode::F2);
        let change_rate = keys.just_pressed(KeyCode::F3);
        if pause {
            clock.paused = !clock.paused;
        }
        if change_rate {
            clock.rate = (clock.rate + 1) % RATES.len();
        }
        if pause || change_rate {
            info!(
                "Celestial clock {} at {}×",
                if clock.paused { "paused" } else { "running" },
                RATES[clock.rate]
            );
        }
    }
    let delta = time.delta_secs_f64() * RATES[clock.rate];
    if !clock.paused && delta > 0.0 {
        clock.seconds += delta;
    }
}

fn place_celestials(
    clock: Res<Clock>,
    root: Res<PlanetRoot>,
    grids: Query<&ReferenceFrame>,
    mut sun: Query<&mut Transform, With<PrimarySun>>,
    mut moons: Query<
        (
            &Moon,
            &mut GridCell,
            &mut Transform,
            &MeshMaterial3d<MoonSurface>,
        ),
        (Without<PrimarySun>, Without<Moonlight>),
    >,
    mut lights: Query<(&Moonlight, &mut Transform, &mut DirectionalLight), Without<PrimarySun>>,
    mut materials: ResMut<Assets<MoonSurface>>,
) {
    if !clock.is_changed() {
        return;
    }
    let grid = grids.get(root.entity()).unwrap();
    let spin = inertial_to_surface(clock.seconds);
    let direction = spin * star_direction(clock.seconds);
    if let Ok(mut sun) = sun.single_mut() {
        sun.rotation = light_rotation(direction);
    }

    let positions = MOONS
        .each_ref()
        .map(|orbit| spin * orbit.inertial_position(clock.seconds));
    for (moon, mut cell, mut transform, handle) in &mut moons {
        (*cell, transform.translation) = grid.translation_to_grid(positions[moon.0]);
        if let Some(mut material) = materials.get_mut(&handle.0) {
            material.sun = direction.as_vec3().extend(0.0);
        }
    }
    for (moon, mut transform, mut light) in &mut lights {
        let moon_direction = positions[moon.0].normalize();
        transform.rotation = light_rotation(moon_direction);
        light.illuminance = MOONS[moon.0].lux * reflected_fraction(direction, moon_direction);
    }
}

fn update_skybox(
    mut commands: Commands,
    clock: Res<Clock>,
    stars: Option<Res<Stars>>,
    root: Res<PlanetRoot>,
    grids: Query<&ReferenceFrame>,
    mut views: Query<(Entity, &GridCell, &Transform, Option<&mut Skybox>), With<WorldCamera>>,
) {
    let Some(stars) = stars else { return };
    let Ok(grid) = grids.get(root.entity()) else {
        return;
    };
    let spin = inertial_to_surface(clock.seconds);
    let sun = spin * star_direction(clock.seconds);
    for (entity, cell, transform, skybox) in &mut views {
        let eye = grid.grid_position_double(cell, transform);
        let twilight = (1.0 - (eye.normalize_or_zero().dot(sun) + 0.23) / 0.18).clamp(0.0, 1.0);
        let space = ((eye.length() - RADIUS - 24_000.0) / 48_000.0).clamp(0.0, 1.0);
        let brightness = 4400.0 * twilight.max(space) as f32;
        if let Some(mut skybox) = skybox {
            skybox.brightness = brightness;
            skybox.rotation = spin.as_quat();
        } else {
            commands.entity(entity).insert(Skybox {
                image: Some(stars.0.clone()),
                brightness,
                rotation: spin.as_quat(),
            });
        }
    }
}

fn starfield() -> Image {
    const SIZE: u32 = 512;
    let mut data = Vec::with_capacity((6 * SIZE * SIZE * 4) as usize);
    let mut rng = 0x6c8e_9cf5_u32;
    let mut random = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng >> 8) as f32 / 16_777_216.0
    };
    for _ in 0..6 {
        for y in 0..SIZE {
            for x in 0..SIZE {
                let uv = (Vec2::new(x as f32, y as f32) + 0.5) * (2.0 / SIZE as f32) - 1.0;
                // Weight by solid angle to keep cube corners from clumping.
                let solid_angle = (1.0 + uv.length_squared()).powf(-1.5);
                if random() < 0.007 * solid_angle {
                    let value = 0.09 + 0.91 * random().powi(5);
                    let tint = random();
                    data.extend(
                        [
                            value * (0.76 + 0.24 * tint),
                            value * 0.88,
                            value * (1.0 - 0.24 * tint),
                        ]
                        .map(|channel| (255.0 * channel) as u8),
                    );
                } else {
                    data.extend([0, 0, 0]);
                }
                data.push(255);
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 6,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::Cube),
        ..default()
    });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_circular_orbits_and_rotation_close_at_their_periods() {
        let t = 2.7 * DAY;
        for moon in &MOONS {
            let p = moon.inertial_position(t);
            assert!((p.length() - moon.radius).abs() < 1e-6);
            assert!(p.distance(moon.inertial_position(t + moon.period)) < 1e-6);
            let frame = ReferenceFrame::default();
            let (cell, translation) = frame.translation_to_grid(p);
            assert!(
                p.distance(
                    frame.grid_position_double(&cell, &Transform::from_translation(translation))
                ) < 0.001
            );
        }
        assert!(
            MOONS[0]
                .inertial_position(t)
                .distance(MOONS[0].inertial_position(t + MOONS[1].period))
                > 1e6
        );
        assert!(star_direction(t).distance(star_direction(t + YEAR)) < 1e-12);
        assert!(star_direction(t).dot(star_direction(t + YEAR * 0.5)) < -0.999);
        let direction = inertial_to_surface(t) * DVec3::X;
        assert!(direction.dot(inertial_to_surface(t + DAY * 0.5) * DVec3::X) < -0.999);
    }

    #[test]
    fn axial_tilt_controls_seasons_and_moons_are_prograde() {
        let normal = ecliptic_normal();
        assert!((normal.dot(DVec3::Y).acos() - AXIAL_TILT).abs() < 1e-12);
        for (fraction, declination) in [
            (0.0, AXIAL_TILT),
            (0.25, 0.0),
            (0.5, -AXIAL_TILT),
            (0.75, 0.0),
        ] {
            let sun = star_direction(YEAR * fraction);
            assert!(normal.dot(sun).abs() < 1e-12);
            assert!((sun.y.asin() - declination).abs() < 1e-12);
        }
        for moon in &MOONS {
            // Recover the normal from motion rather than trusting the
            // configured tilt or the implementation's rotation sign.
            let p = moon.inertial_position(0.0).normalize();
            let q = moon.inertial_position(moon.period / 4.0).normalize();
            let orbit_normal = p.cross(q).normalize();
            assert!((orbit_normal.dot(normal).clamp(-1.0, 1.0).acos() - moon.tilt).abs() < 1e-12);
            assert!(orbit_normal.dot(DVec3::Y) > 0.0);
            assert!(orbit_normal.dot(normal) > 0.0);
        }
    }

    #[test]
    fn moonlight_follows_illuminated_phase() {
        assert!(reflected_fraction(DVec3::X, DVec3::X) < 1e-6);
        assert!((reflected_fraction(DVec3::X, -DVec3::X) - 1.0).abs() < 1e-6);
        assert!((reflected_fraction(DVec3::X, DVec3::Y) - 1.0 / PI as f32).abs() < 1e-6);
    }
}
