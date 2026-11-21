use std::collections::{HashMap, HashSet};

use bevy::{
    math::{DVec2, DVec3},
    pbr::{NotShadowCaster, NotShadowReceiver},
    prelude::*,
};
use world_core::{
    coordinates::{cube_direction, surface_uv},
    planet::RADIUS,
    runtime::{GridTransform, PlanetRoot, ReferenceFrames, WorldCamera},
    SurfaceHeights,
};

use super::lod::{coverage, quantize};

use super::{noise3, tangent_basis, FoliageMaterial, MeshBuilder, Random, Wind};

const CELLS: f64 = 8192.0;
const FADE_START: f32 = 44.0;
const FADE_END: f32 = 62.0;
const PRELOAD: f64 = 82.0;
const MAX_PATCHES: usize = 78;
const PATCHES_PER_TICK: usize = 2;
const SAMPLE_GRID: u32 = 10;
const WIND_MARGIN: f32 = 0.20;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct Patch {
    face: usize,
    x: u32,
    y: u32,
}

impl Patch {
    fn at(direction: DVec3) -> Self {
        let (face, uv) = surface_uv(direction);
        let cell = (uv * CELLS).min(DVec2::splat(CELLS - 1.0)).as_uvec2();
        Self {
            face,
            x: cell.x,
            y: cell.y,
        }
    }

    fn direction(self, offset: DVec2) -> DVec3 {
        let uv = (DVec2::new(f64::from(self.x), f64::from(self.y)) + offset) / CELLS;
        cube_direction(self.face, uv)
    }

    fn seed(self) -> u64 {
        (self.face as u64 * 1_002_407)
            ^ (u64::from(self.x) * 73_856_093)
            ^ (u64::from(self.y) * 19_349_663)
    }
}

#[derive(Resource, Default)]
pub(super) struct GroundCover {
    patches: HashMap<Patch, Option<GroundPatch>>,
    next_tick: f64,
}

struct GroundPatch {
    entity: Entity,
    arrival: f32,
    center: DVec3,
    radius: f64,
}

#[derive(Resource)]
pub(super) struct GroundMaterial([Handle<FoliageMaterial>; 13]);

pub(super) fn build_material(
    mut commands: Commands,
    mut materials: ResMut<Assets<FoliageMaterial>>,
) {
    let materials = std::array::from_fn(|step| {
        materials.add(FoliageMaterial {
            base: StandardMaterial {
                perceptual_roughness: 0.91,
                reflectance: 0.12,
                diffuse_transmission: 0.29,
                double_sided: true,
                cull_mode: None,
                ..default()
            },
            extension: Wind {
                coverage: Vec4::new(0.0, coverage(step as u8), FADE_START, FADE_END),
            },
        })
    });
    commands.insert_resource(GroundMaterial(materials));
}

pub(super) fn stream(
    mut commands: Commands,
    time: Res<Time>,
    mut cover: ResMut<GroundCover>,
    mut meshes: ResMut<Assets<Mesh>>,
    material: Res<GroundMaterial>,
    heights: Res<SurfaceHeights>,
    cameras: Query<(Entity, GridTransform), With<WorldCamera>>,
    frames: ReferenceFrames,
    root: Res<PlanetRoot>,
    mut renderables: Query<(&mut Handle<FoliageMaterial>, &mut Visibility)>,
) {
    let Ok((camera_entity, camera)) = cameras.get_single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(camera_entity) else {
        return;
    };
    let parent = root.entity();
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    for patch in cover.patches.values_mut().flatten() {
        if let Ok((mut handle, mut visibility)) = renderables.get_mut(patch.entity) {
            let in_range = eye.distance(patch.center) <= f64::from(FADE_END) + patch.radius;
            visibility.set_if_neq(if in_range {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
            if patch.arrival < 1.0 {
                patch.arrival = (patch.arrival + time.delta_seconds().min(0.075) * 3.5).min(1.0);
                handle.set_if_neq(material.0[quantize(patch.arrival) as usize].clone());
            }
        }
    }
    let now = time.elapsed_seconds_f64();
    if now < cover.next_tick {
        return;
    }
    cover.next_tick = now + 0.12;
    let Some(up) = eye.try_normalize() else {
        return;
    };
    let altitude = eye.length() - RADIUS - heights.sample(up).unwrap_or(0.0);
    let wanted = if altitude <= PRELOAD + 22.0 {
        visible_patches(up)
    } else {
        Vec::new()
    };
    let active: HashSet<_> = wanted.iter().copied().collect();
    // Ground cover is a single renderable per patch. Bound retirement during
    // travel and release each patch mesh along with its entity.
    let retiring: Vec<_> = cover
        .patches
        .keys()
        .filter(|key| !active.contains(key))
        .copied()
        .take(6)
        .collect();
    for patch in retiring {
        if let Some(Some(old)) = cover.patches.remove(&patch) {
            commands.entity(old.entity).despawn_recursive();
        }
    }
    let available = MAX_PATCHES
        .saturating_sub(cover.patches.len())
        .min(PATCHES_PER_TICK);
    let pending: Vec<_> = wanted
        .into_iter()
        .filter(|key| !cover.patches.contains_key(key))
        .take(available)
        .collect();
    for patch in pending {
        let entity = load_patch(patch, &heights).map(|(mesh, position, rotation)| {
            let (cell, translation) = frame.translation_to_grid(position);
            let mut bounds = mesh.compute_aabb().expect("plant patches contain vertices");
            bounds.half_extents += WIND_MARGIN;
            let id = commands
                .spawn((
                    cell,
                    MaterialMeshBundle::<FoliageMaterial> {
                        mesh: meshes.add(mesh),
                        material: material.0[0].clone(),
                        transform: Transform::from_translation(translation).with_rotation(rotation),
                        ..default()
                    },
                    bounds,
                    NotShadowCaster,
                    NotShadowReceiver,
                ))
                .id();
            commands.entity(parent).add_child(id);
            GroundPatch {
                entity: id,
                arrival: 0.0,
                center: position + (rotation * Vec3::from(bounds.center)).as_dvec3(),
                radius: f64::from(bounds.half_extents.length()),
            }
        });
        cover.patches.insert(patch, entity);
    }
}

fn visible_patches(up: DVec3) -> Vec<Patch> {
    let (east, north) = tangent_basis(up);
    let spacing = 13.0 / RADIUS;
    let mut candidates = HashSet::new();
    for y in -7..=7 {
        for x in -7..=7 {
            candidates.insert(Patch::at(
                up + spacing * (east * f64::from(x) + north * f64::from(y)),
            ));
        }
    }
    let mut sorted: Vec<_> = candidates
        .into_iter()
        .filter_map(|key| {
            let distance = key.direction(DVec2::splat(0.5)).distance_squared(up) * RADIUS.powi(2);
            (distance <= PRELOAD * PRELOAD).then_some((distance, key))
        })
        .collect();
    sorted.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    sorted
        .into_iter()
        .take(MAX_PATCHES)
        .map(|(_, key)| key)
        .collect()
}

fn load_patch(key: Patch, heights: &SurfaceHeights) -> Option<(Mesh, DVec3, Quat)> {
    let up = key.direction(DVec2::splat(0.5));
    let patch_height = heights.sample(up)?;
    let origin = up * (RADIUS + patch_height);
    let rotation = Quat::from_rotation_arc(Vec3::Y, up.as_vec3());
    let (east, north) = tangent_basis(up);
    let wet = 0.51 + noise3(up.as_vec3() * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let temperature = 1.07 - 0.78 * up.y.abs() as f32 - patch_height.max(0.0) as f32 / 16_000.0;
    let mut mesh = MeshBuilder::default();
    let mut rng = Random::new(key.seed());
    for y in 0..SAMPLE_GRID {
        for x in 0..SAMPLE_GRID {
            let choice = rng.unit();
            let offset = (DVec2::new(f64::from(x) + rng.unit64(), f64::from(y) + rng.unit64()))
                / f64::from(SAMPLE_GRID);
            let direction = key.direction(offset);
            let Some(elevation) = heights.sample(direction) else {
                continue;
            };
            if !(1.35..3200.0).contains(&elevation) || heights.is_river(direction) {
                continue;
            }
            let slope_step = 3.0 / RADIUS;
            let east_height = heights
                .sample((direction + east * slope_step).normalize())
                .unwrap_or(elevation);
            let north_height = heights
                .sample((direction + north * slope_step).normalize())
                .unwrap_or(elevation);
            if (east_height - elevation).hypot(north_height - elevation) > 2.15 {
                continue;
            }
            let bank = elevation < 9.0
                || (choice < 0.22
                    && [east, north, -east, -north].into_iter().any(|axis| {
                        heights.is_river((direction + axis * (9.0 / RADIUS)).normalize())
                    }));
            let ground = (direction * (RADIUS + elevation)).as_vec3();
            let cover = noise3(ground * 0.21 + Vec3::new(4.1, 11.8, -7.3));
            let density = (0.49 + 0.44 * wet + 1.34 * cover).clamp(0.12, 0.96);
            let position = direction * (RADIUS + elevation - 0.13);
            let root = rotation.inverse() * (position - origin).as_vec3();
            let plant = Plant {
                root,
                phase: rng.range(0.0, std::f32::consts::TAU),
            };
            if choice > density || (temperature < 0.16 && choice > 0.18) {
                if choice < density + 0.09 {
                    plant.gravel(&mut mesh, &mut rng, wet);
                }
            } else if bank {
                plant.grass(&mut mesh, &mut rng, true, wet);
            } else if temperature > 0.33 && wet > 0.43 && choice < 0.18 {
                plant.fern(&mut mesh, &mut rng);
            } else if temperature > 0.24 && choice < 0.27 {
                plant.flowers(&mut mesh, &mut rng);
            } else {
                plant.grass(&mut mesh, &mut rng, false, wet);
            }
        }
    }
    (!mesh.indices.is_empty()).then(|| (mesh.finish(), origin, rotation))
}

struct Plant {
    root: Vec3,
    phase: f32,
}

impl Plant {
    fn gravel(&self, mesh: &mut MeshBuilder, rng: &mut Random, wet: f32) {
        let tone = Vec3::new(0.22, 0.20, 0.16).lerp(Vec3::new(0.087, 0.095, 0.076), wet);
        for rock in 0..3 {
            let angle = self.phase + rock as f32 * 2.39996;
            let (sin, cos) = angle.sin_cos();
            let center = self.root + Vec3::new(cos, 0.0, sin) * rng.range(0.03, 0.27);
            let radius = rng.range(0.075, 0.26);
            let hue = tone * rng.range(0.83, 1.19);
            let mut vertex = |point: Vec3, normal: Vec3, light: f32| {
                let color = hue * light;
                mesh.wind.push([0.0; 2]);
                mesh.vertex(point, normal, Vec2::ZERO, [color.x, color.y, color.z, 1.0])
            };
            let top = vertex(
                center + Vec3::new(-radius * 0.18, 0.13 + radius * 0.52, radius * 0.12),
                Vec3::Y,
                1.12,
            );
            let rim: [u32; 5] = std::array::from_fn(|index| {
                let angle = angle + index as f32 * std::f32::consts::TAU / 5.0;
                let (sin, cos) = angle.sin_cos();
                let axis = Vec3::new(cos, 0.0, sin);
                let point = center
                    + axis * radius * rng.range(0.69, 1.14)
                    + Vec3::Y * rng.range(0.08, 0.15);
                vertex(point, axis + Vec3::Y * 0.5, rng.range(0.69, 0.98))
            });
            for i in 0..rim.len() {
                mesh.triangle(top, rim[(i + 1) % rim.len()], rim[i]);
            }
        }
    }

    fn vertex(&self, mesh: &mut MeshBuilder, point: Vec3, normal: Vec3, tint: Vec3) -> u32 {
        mesh.wind
            .push([(point.y * 0.11).clamp(0.0, 0.17), self.phase]);
        mesh.vertex(
            self.root + point,
            normal,
            Vec2::ZERO,
            [tint.x, tint.y, tint.z, 1.0],
        )
    }

    fn ribbon(
        &self,
        mesh: &mut MeshBuilder,
        start: Vec3,
        tip: Vec3,
        half_width: Vec3,
        color: Vec3,
    ) {
        let delta = tip - start;
        let middle = start + delta * 0.46 + Vec3::Y * delta.length() * 0.21;
        let normal = half_width.cross(delta).normalize_or_zero();
        let a = self.vertex(mesh, start - half_width * 0.55, normal, color * 0.43);
        let b = self.vertex(mesh, start + half_width * 0.55, normal, color * 0.50);
        let c = self.vertex(mesh, middle + half_width, normal, color * 0.90);
        let d = self.vertex(mesh, middle - half_width, normal, color * 0.76);
        let e = self.vertex(mesh, tip, normal, color * 1.20);
        mesh.triangle(a, b, d);
        mesh.triangle(b, c, d);
        mesh.triangle(d, c, e);
    }

    fn grass(&self, mesh: &mut MeshBuilder, rng: &mut Random, reed: bool, wet: f32) {
        let color =
            Vec3::new(0.18, 0.22, 0.043).lerp(Vec3::new(0.068, 0.23, 0.046), wet.clamp(0.0, 1.0));
        for blade in 0..if reed { 8 } else { 6 } {
            let angle = self.phase + blade as f32 * 2.39996 + rng.range(-0.28, 0.28);
            let (sin, cos) = angle.sin_cos();
            let direction = Vec3::new(cos, 0.0, sin);
            let start = direction * rng.range(0.03, 0.24);
            let height = if reed {
                rng.range(0.75, 1.52)
            } else {
                rng.range(0.28, 0.67)
            };
            let tip = start + Vec3::Y * height + direction * rng.range(0.08, 0.47);
            let tint = color * rng.range(0.73, 1.36);
            self.ribbon(
                mesh,
                start,
                tip,
                Vec3::Y.cross(direction) * rng.range(0.018, 0.044),
                tint,
            );
            if reed && blade % 3 == 0 {
                let seed = Vec3::new(0.22, 0.125, 0.036);
                let head = tip + Vec3::Y * 0.18;
                self.ribbon(mesh, tip - Vec3::Y * 0.08, head, direction * 0.039, seed);
                self.ribbon(
                    mesh,
                    tip - Vec3::Y * 0.08,
                    head,
                    direction.cross(Vec3::Y) * 0.039,
                    seed,
                );
            }
        }
    }

    fn fern(&self, mesh: &mut MeshBuilder, rng: &mut Random) {
        for frond in 0..6 {
            let angle = self.phase + frond as f32 * 2.39996;
            let (sin, cos) = angle.sin_cos();
            let forward = Vec3::new(cos, 0.0, sin);
            let side = Vec3::Y.cross(forward);
            let length = rng.range(0.57, 1.06);
            let green = Vec3::new(0.069, 0.28, 0.058) * rng.range(0.75, 1.22);
            let shape =
                |t: f32| forward * length * t + Vec3::Y * (0.22 + 1.9 * t - 1.36 * t * t) * length;
            self.ribbon(mesh, Vec3::ZERO, shape(0.64), side * 0.023, green);
            for row in 1..=5 {
                let t = row as f32 / 6.0;
                let spread = (std::f32::consts::PI * t).sin() * length * 0.28;
                for sign in [-1.0, 1.0] {
                    let from = shape(t);
                    let to = shape((t + 0.18).min(1.0)) + side * spread * sign;
                    self.ribbon(mesh, from, to, forward * spread * 0.18, green);
                }
            }
            self.ribbon(mesh, shape(0.62), shape(1.0), side * 0.05, green * 1.16);
        }
    }

    fn flowers(&self, mesh: &mut MeshBuilder, rng: &mut Random) {
        let color = match rng.next_u32() % 4 {
            0 => Vec3::new(0.72, 0.53, 0.13),
            1 => Vec3::new(0.48, 0.21, 0.64),
            2 => Vec3::new(0.77, 0.75, 0.61),
            _ => Vec3::new(0.63, 0.26, 0.35),
        };
        let green = Vec3::new(0.084, 0.23, 0.032);
        for stem in 0..4 {
            let angle = self.phase + stem as f32 * 2.4;
            let (sin, cos) = angle.sin_cos();
            let side = Vec3::new(cos, 0.0, sin);
            let start = side * rng.range(0.02, 0.22);
            let bloom = start + Vec3::Y * rng.range(0.40, 0.77) + side * 0.11;
            self.ribbon(mesh, start, bloom, side * 0.019, green);
            let leaf = start.lerp(bloom, 0.48);
            self.ribbon(
                mesh,
                leaf,
                leaf + side * 0.23 + Vec3::Y * 0.13,
                Vec3::Y.cross(side) * 0.042,
                green,
            );
            let center = self.vertex(
                mesh,
                bloom + Vec3::Y * 0.027,
                Vec3::Y,
                Vec3::new(0.65, 0.33, 0.055),
            );
            for petal in 0..5 {
                let angle = self.phase + petal as f32 * std::f32::consts::TAU / 5.0;
                let (sin, cos) = angle.sin_cos();
                let axis = Vec3::new(cos, 0.0, sin);
                let spread = axis.cross(Vec3::Y) * 0.044;
                let left = self.vertex(mesh, bloom + axis * 0.085 - spread, Vec3::Y, color * 0.82);
                let tip = self.vertex(mesh, bloom + axis * 0.125 + Vec3::Y * 0.034, Vec3::Y, color);
                let right = self.vertex(mesh, bloom + axis * 0.085 + spread, Vec3::Y, color * 0.94);
                mesh.triangle(center, tip, left);
                mesh.triangle(center, right, tip);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_patches_are_bounded_deterministic_and_cross_cube_seams() {
        let eye = DVec3::new(1.0, 0.82, 1.0).normalize();
        let patches = visible_patches(eye);
        assert_eq!(patches, visible_patches(eye));
        assert!(patches.len() <= MAX_PATCHES);
        assert!(patches.iter().any(|patch| patch.face == 1));
        assert!(patches.iter().any(|patch| patch.face == 3));
    }

    #[test]
    fn ground_cover_is_generated_near_the_starting_explorer() {
        let heights = SurfaceHeights::load(std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets"
        )));
        let up = world_core::coordinates::terrain_aligned_direction(
            world_regions::STARTER_REGION.spawn.direction,
        );
        assert!(visible_patches(up)
            .into_iter()
            .take(8)
            .any(|patch| load_patch(patch, &heights).is_some()));
    }

    #[test]
    fn plant_blades_have_wind_weights_and_bounded_geometry() {
        let mut mesh = MeshBuilder::default();
        let plant = Plant {
            root: Vec3::ZERO,
            phase: 0.83,
        };
        let mut rng = Random::new(237);
        plant.fern(&mut mesh, &mut rng);
        plant.flowers(&mut mesh, &mut rng);
        plant.grass(&mut mesh, &mut rng, true, 0.7);
        assert_eq!(mesh.wind.len(), mesh.positions.len());
        assert!(mesh.indices.len() / 3 < 350);
        assert!(mesh
            .wind
            .iter()
            .all(|weight| (0.0..=WIND_MARGIN).contains(&weight[0])));
    }
}
