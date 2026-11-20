//! Deterministic procedural trees streamed in small patches around the explorer.
//! Meshes are built once per species/variant/LOD and shared by all instances.
//! The forest uses the same planet-local climate noise as spherical.wgsl.
use std::collections::{HashMap, HashSet};

use bevy::{
    math::DVec3,
    pbr::{NotShadowCaster, NotShadowReceiver},
    prelude::*,
    render::{
        mesh::Indices,
        render_asset::RenderAssetUsages,
        render_resource::{Extent3d, PrimitiveTopology, TextureDimension, TextureFormat},
    },
};
use bevy_terrain::{
    big_space::{GridTransform, ReferenceFrame, ReferenceFrames},
    prelude::DebugCameraController,
};

use crate::{
    planet::RADIUS,
    player,
    surface_heights::{terrain_aligned_direction, SurfaceHeights},
};

const PATCH_METERS: f64 = 145.0;
const PATCH_ANGLE: f64 = PATCH_METERS / RADIUS;
// Render only a small, camera-facing neighborhood, even on a spherical world.
const PATCH_RADIUS: i32 = 4;
const CANDIDATES_PER_PATCH: usize = 112;
// High quality only inside 50m; cheaper foliage before 100m;
// only sparse geometry / textured impostors past that.
const LOD_DISTANCES: [f64; 3] = [50.0, 100.0, 170.0];
const FIRST_PERSON_RANGE: f64 = 275.0;
const THIRD_PERSON_RANGE: f64 = 375.0;
const PATCH_PRELOAD_MARGIN: f64 = 86.0;
const PATCH_UNLOAD_MARGIN: f64 = 260.0;
const MAX_PATCHES_PER_TICK: usize = 2;
const CANDIDATES_PER_TICK_PER_PATCH: usize = 14;
const MAX_NEW_TREES_PER_TICK: usize = 6;
const MAX_ACTIVE_TREES: usize = 320;
const STREAM_INTERVAL: f64 = 0.10;
const LOD_INTERVAL: f64 = 0.08;
const MAX_FOREST_ABOVE_GROUND: f64 = 700.0;
const VARIANTS: usize = 3;

#[derive(Clone, Copy, Debug)]
#[repr(usize)]
enum Species {
    Oak = 0,
    Pine = 1,
    Acacia = 2,
    Jungle = 3,
}

impl Species {
    const ALL: [Self; 4] = [Self::Oak, Self::Pine, Self::Acacia, Self::Jungle];

    fn colors(self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Oak => ([0.35, 0.22, 0.12], [0.23, 0.53, 0.18]),
            Self::Pine => ([0.30, 0.20, 0.13], [0.13, 0.34, 0.23]),
            Self::Acacia => ([0.34, 0.25, 0.15], [0.42, 0.55, 0.19]),
            Self::Jungle => ([0.29, 0.20, 0.14], [0.14, 0.48, 0.17]),
        }
    }
}

struct ModelLod {
    wood: Option<Handle<Mesh>>,
    foliage: Handle<Mesh>,
    foliage_material: Handle<StandardMaterial>,
}

struct VariantModels {
    lods: Vec<ModelLod>,
}

#[derive(Resource)]
struct ForestModels {
    variants: Vec<Vec<VariantModels>>,
    bark_material: Handle<StandardMaterial>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct PatchKey {
    row: i32,
    col: i32,
}

#[derive(Default)]
struct PatchState {
    entities: Vec<Entity>,
    next_candidate: usize,
}
/// Updated by player control after the first/third-person camera is positioned.
#[derive(Resource, Default)]
pub struct ForestView {
    pub first_person: bool,
}

#[derive(Resource, Default)]
struct ForestWorld {
    patches: HashMap<PatchKey, PatchState>,
    last_check: f64,
}

// Every tree has only two renderable children, rather than spawning all four
// LODs at once. All instances share one of the precomputed meshes.
#[derive(Component)]
struct TreeInstance {
    position: DVec3,
    species: Species,
    variant: usize,
    current_lod: Option<usize>,
    trunk: Entity,
    canopy: Entity,
}

#[derive(Component)]
struct TrunkMesh;

#[derive(Component)]
struct CanopyMesh;

pub struct ProceduralForestPlugin;

impl Plugin for ProceduralForestPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ForestWorld>()
            .init_resource::<ForestView>()
            .add_systems(Startup, build_models)
            // Read the current camera position, not the previous frame's.
            .add_systems(Update, (stream_patches, update_lods).chain().after(player::walk));
    }
}

fn build_models(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let bark_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.94,
        ..default()
    });
    // One small procedural alpha-cutout atlas for both leaflets and broad
    // canopy clusters: substantial foliage volume without thousands of quads.
    let foliage_texture = images.add(foliage_cluster_texture());
    let foliage_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(foliage_texture),
        alpha_mode: AlphaMode::Mask(0.38),
        perceptual_roughness: 0.86,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let mut variants = Vec::with_capacity(Species::ALL.len());
    for species in Species::ALL {
        let impostor_image = images.add(impostor_texture(species));
        let impostor_material = materials.add(StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: Some(impostor_image),
            alpha_mode: AlphaMode::Mask(0.42),
            double_sided: true,
            cull_mode: None,
            perceptual_roughness: 0.98,
            ..default()
        });
        let mut species_variants = Vec::with_capacity(VARIANTS);
        for variant in 0..VARIANTS {
            let mut lods = Vec::with_capacity(4);
            for level in 0..3 {
                let (wood, leaves) = generate_tree(species, variant as u64, level);
                lods.push(ModelLod {
                    wood: Some(meshes.add(wood.finish())),
                    foliage: meshes.add(leaves.finish()),
                    foliage_material: foliage_material.clone(),
                });
            }
            let mut card = MeshBuilder::default();
            let height = tree_height(species, variant as u64);
            let crown_width = match species {
                Species::Oak => height * 0.72,
                Species::Pine => height * 0.42,
                Species::Acacia => height * 1.25,
                Species::Jungle => height * 0.69,
            };
            card.crossed_cards(height, crown_width);
            lods.push(ModelLod {
                wood: None,
                foliage: meshes.add(card.finish()),
                foliage_material: impostor_material.clone(),
            });
            species_variants.push(VariantModels { lods });
        }
        variants.push(species_variants);
    }
    commands.insert_resource(ForestModels { variants, bark_material });
}

fn stream_patches(
    mut commands: Commands,
    time: Res<Time>,
    mut forest: ResMut<ForestWorld>,
    models: Res<ForestModels>,
    heights: Res<SurfaceHeights>,
    view_mode: Res<ForestView>,
    cameras: Query<(Entity, GridTransform, &Projection), With<DebugCameraController>>,
    reference_frames: ReferenceFrames,
    frame_entities: Query<Entity, With<ReferenceFrame>>,
) {
    let Ok((camera_entity, camera, projection)) = cameras.get_single() else { return };
    let Some(frame) = reference_frames.parent_frame(camera_entity) else { return };
    let Ok(frame_entity) = frame_entities.get_single() else { return };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    // Camera altitude above its local terrain, not above the planet's sea level.
    // High-elevation jungle must still render from eye height.
    let local_ground = heights.sample(eye.normalize()).unwrap_or(0.0);
    let altitude = eye.length() - RADIUS - local_ground;
    let view_range = forest_range(view_mode.first_person);

    if altitude > MAX_FOREST_ABOVE_GROUND {
        for (_, patch) in forest.patches.drain() {
            for entity in patch.entities { commands.entity(entity).despawn_recursive(); }
        }
        return;
    }
    let now = time.elapsed_seconds_f64();
    if now - forest.last_check < STREAM_INTERVAL { return }
    forest.last_check = now;
    let forward = (camera.transform.rotation * Vec3::NEG_Z).as_dvec3();

    // Keep already generated patches behind the camera until physically far
    // away. Turning around must not tear down and rebuild a whole forest.
    forest.patches.retain(|key, patch| {
        let center = patch_center_direction(*key);
        let anchor = center * (RADIUS + heights.sample(center).unwrap_or(0.0).max(0.0));
        if eye.distance(anchor) <= view_range + PATCH_UNLOAD_MARGIN
            && within_horizon(eye, anchor, PATCH_UNLOAD_MARGIN) { return true }
        for entity in patch.entities.drain(..) {
            commands.entity(entity).despawn_recursive();
        }
        false
    });

    let mut wanted: Vec<_> = nearby_patches(eye.normalize())
        .into_iter()
        .filter_map(|key| {
            let direction = patch_center_direction(key);
            let anchor = direction * (RADIUS + heights.sample(direction).unwrap_or(0.0).max(0.0));
            let distance = eye.distance(anchor);
            (distance <= view_range + PATCH_PRELOAD_MARGIN
                && within_horizon(eye, anchor, PATCH_PRELOAD_MARGIN)
                && within_camera_cone(eye, forward, projection, anchor, PATCH_PRELOAD_MARGIN))
                .then_some((key, distance))
        }).collect();
    wanted.sort_by(|a,b| a.1.total_cmp(&b.1)
        .then_with(|| a.0.row.cmp(&b.0.row))
        .then_with(|| a.0.col.cmp(&b.0.col)));

    let mut active = forest.patches.values().map(|p| p.entities.len()).sum::<usize>();
    // If the global budget is full, evict the farthest off-screen patches.
    // Without this, travelling forward eventually shows empty terrain while
    // cached trees behind the explorer consume every available slot.
    if active >= MAX_ACTIVE_TREES {
        let mut eviction: Vec<_> = forest.patches.keys().filter_map(|&key| {
            let center = patch_center_direction(key);
            let location = center * (RADIUS + heights.sample(center).unwrap_or(0.0).max(0.0));
            let distance = eye.distance(location);
            (!within_camera_cone(eye, forward, projection, location, PATCH_METERS)
                && distance > view_range * 0.55)
                .then_some((key, distance))
        }).collect();
        eviction.sort_by(|a,b| b.1.total_cmp(&a.1));
        for (key, _) in eviction {
            if active <= MAX_ACTIVE_TREES - MAX_NEW_TREES_PER_TICK * 3 { break }
            if let Some(old) = forest.patches.remove(&key) {
                active = active.saturating_sub(old.entities.len());
                for entity in old.entities {
                    commands.entity(entity).despawn_recursive();
                }
            }
        }
    }
    // Materialize worklist before mutating the HashMap (avoids E0502).
    let work: Vec<_> = wanted.into_iter()
        .filter(|(key,_)| forest.patches.get(key)
            .map(|patch| patch.next_candidate < CANDIDATES_PER_PATCH)
            .unwrap_or(true))
        .take(MAX_PATCHES_PER_TICK)
        .map(|(key,_)|key).collect();

    let mut remaining = MAX_NEW_TREES_PER_TICK;
    for key in work {
        if remaining == 0 || active >= MAX_ACTIVE_TREES { break }
        let start = forest.patches.get(&key).map_or(0, |p|p.next_candidate);
        let end = (start + CANDIDATES_PER_TICK_PER_PATCH).min(CANDIDATES_PER_PATCH);
        let (min_latitude,longitude_step) = patch_bounds(key);
        let mut roots = Vec::new();
        let mut progressed = start;
        for candidate in start..end {
            if remaining == 0 || active >= MAX_ACTIVE_TREES { break }
            progressed = candidate + 1;
            let seed = patch_seed(key) ^ (candidate as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15);
            let mut rng = Random::new(seed);
            let latitude = (min_latitude + rng.unit64() * PATCH_ANGLE)
                .clamp(-std::f64::consts::FRAC_PI_2,std::f64::consts::FRAC_PI_2);
            let longitude = (f64::from(key.col) + rng.unit64()) * longitude_step;
            let (sin_lat,cos_lat)=latitude.sin_cos();
            let direction=DVec3::new(cos_lat*longitude.cos(),sin_lat,cos_lat*longitude.sin());
            // Align direction first so sampled height and rendered terrain
            // address the same cube-face texel. Sink roots slightly into soil.
            let aligned = terrain_aligned_direction(direction);
            let Some(ground)=heights.sample(aligned) else {continue};
            if !(3.5..=2600.0).contains(&ground) || heights.is_river(aligned) {continue}
            let Some(species)=select_species(direction,ground,&mut rng) else {continue};
            let (east,north)=tangent_basis(direction);
            let offset=4.0/RADIUS;
            let he=heights.sample((direction+east*offset).normalize()).unwrap_or(ground);
            let hn=heights.sample((direction+north*offset).normalize()).unwrap_or(ground);
            if (he-ground).hypot(hn-ground)/4.0 > 0.58 {continue}
            let pos=aligned*(RADIUS+ground-1.6);
            let distance=eye.distance(pos);
            if distance>view_range+PATCH_PRELOAD_MARGIN {continue}
            let variant=(rng.next_u32() as usize)%VARIANTS;
            let scale=rng.range(0.84,1.28);
            let yaw=rng.range(0.0,std::f32::consts::TAU);
            roots.push(spawn_tree(
                &mut commands,frame_entity,frame,pos,species,variant,scale,yaw,distance,&models));
            active+=1;
            remaining-=1;
        }
        let patch=forest.patches.entry(key).or_default();
        patch.next_candidate=progressed;
        patch.entities.extend(roots);
    }
}

fn spawn_tree(
    commands: &mut Commands,
    frame_entity: Entity,
    frame: &ReferenceFrame,
    position: DVec3,
    species: Species,
    variant: usize,
    scale: f32,
    yaw: f32,
    distance: f64,
    models: &ForestModels,
) -> Entity {
    let (cell, translation) = frame.translation_to_grid(position);
    let rotation = Quat::from_rotation_arc(Vec3::Y, position.normalize().as_vec3())
        * Quat::from_rotation_y(yaw);
    let level = select_lod(distance).unwrap_or(3);
    let model = &models.variants[species as usize][variant].lods[level];

    let root = commands.spawn((
        cell,
        SpatialBundle {
            transform: Transform {
                translation,
                rotation,
                scale: Vec3::splat(scale),
            },
            ..default()
        },
    )).id();
    commands.entity(frame_entity).add_child(root);

    let trunk = commands.spawn((
        TrunkMesh,
        NotShadowCaster,
        NotShadowReceiver,
        PbrBundle {
            mesh: model.wood.clone().unwrap_or_default(),
            material: models.bark_material.clone(),
            visibility: if model.wood.is_some() {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
            ..default()
        },
    )).id();
    let canopy = commands.spawn((
        CanopyMesh,
        NotShadowCaster,
        NotShadowReceiver,
        PbrBundle {
            mesh: model.foliage.clone(),
            material: model.foliage_material.clone(),
            ..default()
        },
    )).id();
    commands.entity(root).add_child(trunk);
    commands.entity(root).add_child(canopy);
    commands.entity(root).insert(TreeInstance {
        position,
        species,
        variant,
        current_lod: Some(level),
        trunk,
        canopy,
    });
    root
}

fn update_lods(
    time: Res<Time>,
    mut last_update: Local<f64>,
    models: Res<ForestModels>,
    cameras: Query<(Entity, GridTransform, &Projection), With<DebugCameraController>>,
    frames: ReferenceFrames,
    view_mode: Res<ForestView>,
    mut trees: Query<
        (&mut TreeInstance, &mut Visibility),
        (Without<TrunkMesh>, Without<CanopyMesh>),
    >,
    mut trunks: Query<
        (&mut Handle<Mesh>, &mut Visibility),
        (With<TrunkMesh>, Without<CanopyMesh>),
    >,
    mut canopies: Query<
        (&mut Handle<Mesh>, &mut Handle<StandardMaterial>),
        (With<CanopyMesh>, Without<TrunkMesh>),
    >,
) {
    let now = time.elapsed_seconds_f64();
    if now - *last_update < LOD_INTERVAL { return }
    *last_update = now;
    let Ok((camera_entity, camera, projection)) = cameras.get_single() else { return };
    let Some(frame) = frames.parent_frame(camera_entity) else { return };
    let eye = frame.grid_position_double(&camera.cell, &camera.transform);
    let forward = (camera.transform.rotation * Vec3::NEG_Z).as_dvec3();
    let view_range = forest_range(view_mode.first_person);

    for (mut tree, mut visibility) in &mut trees {
        let in_view = eye.distance(tree.position) <= view_range
            && within_horizon(eye, tree.position, 25.0)
            && within_camera_cone(eye, forward, projection, tree.position, 24.0);
        let next = if in_view {
            select_lod(eye.distance(tree.position))
        } else {
            None
        };
        let desired_visibility = if next.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != desired_visibility { *visibility = desired_visibility; }

        if tree.current_lod == next { continue }
        if let Some(level) = next {
            let model = &models.variants[tree.species as usize][tree.variant].lods[level];
            if let Ok((mut mesh, mut visible)) = trunks.get_mut(tree.trunk) {
                if let Some(wood) = &model.wood {
                    *mesh = wood.clone();
                    *visible = Visibility::Inherited;
                } else {
                    *visible = Visibility::Hidden;
                }
            }
            if let Ok((mut mesh, mut material)) = canopies.get_mut(tree.canopy) {
                *mesh = model.foliage.clone();
                *material = model.foliage_material.clone();
            }
        }
        tree.current_lod = next;
    }
}

fn forest_range(first_person: bool) -> f64 {
    if first_person { FIRST_PERSON_RANGE } else { THIRD_PERSON_RANGE }
}

fn select_lod(distance: f64) -> Option<usize> {
    if distance > THIRD_PERSON_RANGE { return None }
    Some(LOD_DISTANCES.iter().position(|&d| distance < d).unwrap_or(3))
}

// A conservative spherical-horizon test; large trees remain visible
// slightly beyond the ground horizon to avoid clipping their crowns.
fn within_horizon(eye: DVec3, position: DVec3, extra_height: f64) -> bool {
    let radius = position.length();
    eye.dot(position) + eye.length() * extra_height >= radius * radius
}

// Perspective camera frustum approximation with a diagonal cone, expanded by
// tree/patch extent. Conservative for both tall and wide windows.
fn within_camera_cone(
    eye: DVec3,
    forward: DVec3,
    projection: &Projection,
    position: DVec3,
    radius: f64,
) -> bool {
    let vector = position - eye;
    let distance = vector.length();
    if distance <= radius.max(0.1) { return true }
    let half_angle = match projection {
        Projection::Perspective(p) => {
            let vertical = f64::from(p.fov) * 0.5;
            let horizontal = (vertical.tan() * f64::from(p.aspect_ratio)).atan();
            vertical.hypot(horizontal)
        }
        Projection::Orthographic(_) => return true,
    };
    let extension = (radius / distance).clamp(0.0, 1.0).asin() + 0.06;
    let angle = (half_angle + extension).min(std::f64::consts::PI);
    forward.dot(vector / distance) >= angle.cos()
}

fn patch_center_direction(key: PatchKey) -> DVec3 {
    let (min_latitude, longitude_step) = patch_bounds(key);
    let latitude = (min_latitude + PATCH_ANGLE * 0.5)
        .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2);
    let longitude = (f64::from(key.col) + 0.5) * longitude_step;
    let (sin_lat, cos_lat) = latitude.sin_cos();
    DVec3::new(
        cos_lat * longitude.cos(),
        sin_lat,
        cos_lat * longitude.sin(),
    )
}

fn nearby_patches(direction: DVec3) -> HashSet<PatchKey> {
    let latitude = direction.y.clamp(-1.0, 1.0).asin();
    let longitude = direction.z.atan2(direction.x).rem_euclid(std::f64::consts::TAU);
    let center = ((latitude + std::f64::consts::FRAC_PI_2) / PATCH_ANGLE).floor() as i32;
    let max_row = (std::f64::consts::PI / PATCH_ANGLE).floor() as i32;
    let mut result = HashSet::new();
    for row in (center - PATCH_RADIUS).max(0)..=(center + PATCH_RADIUS).min(max_row) {
        let count = longitude_cells(row);
        let center_col = (longitude / (std::f64::consts::TAU / f64::from(count))).floor() as i32;
        for dc in -PATCH_RADIUS..=PATCH_RADIUS {
            result.insert(PatchKey { row, col: (center_col + dc).rem_euclid(count) });
        }
    }
    result
}

fn longitude_cells(row: i32) -> i32 {
    let latitude = -std::f64::consts::FRAC_PI_2 + (f64::from(row) + 0.5) * PATCH_ANGLE;
    ((std::f64::consts::TAU * latitude.cos().abs().max(0.08) / PATCH_ANGLE).round() as i32).max(1)
}

fn patch_bounds(key: PatchKey) -> (f64, f64) {
    (-std::f64::consts::FRAC_PI_2 + f64::from(key.row) * PATCH_ANGLE,
     std::f64::consts::TAU / f64::from(longitude_cells(key.row)))
}

fn patch_seed(key: PatchKey) -> u64 {
    let mut n = ((key.row as i64 as u64) << 32) ^ key.col as i64 as u64;
    n ^= n >> 30;
    n = n.wrapping_mul(0xbf58476d1ce4e5b9);
    n ^= n >> 27;
    n = n.wrapping_mul(0x94d049bb133111eb);
    n ^ (n >> 31)
}

fn tangent_basis(up: DVec3) -> (DVec3, DVec3) {
    let reference = if up.y.abs() > 0.94 { DVec3::X } else { DVec3::Y };
    let east = up.cross(reference).normalize();
    (east, east.cross(up).normalize())
}

// Mirrors the WGSL macro_noise and its climate inputs, to avoid trees in deserts
// or tundra when the terrain shader draws a different material there.
fn hash_shader(p: Vec3) -> f32 {
    let scaled = p * Vec3::new(0.1031, 0.1030, 0.0973);
    let mut h = scaled - scaled.floor();
    let other = Vec3::new(h.y, h.z, h.x) + Vec3::splat(33.33);
    h += Vec3::splat(h.dot(other));
    let v = (h.x + h.y) * h.z;
    v - v.floor()
}

fn noise3(position: Vec3) -> f32 {
    let cell = position.floor();
    let t = position - cell;
    let w = t * t * (Vec3::splat(3.0) - 2.0 * t);
    let a = hash_shader(cell);
    let b = hash_shader(cell + Vec3::X);
    let c = hash_shader(cell + Vec3::Y);
    let d = hash_shader(cell + Vec3::X + Vec3::Y);
    let e = hash_shader(cell + Vec3::Z);
    let f = hash_shader(cell + Vec3::X + Vec3::Z);
    let g = hash_shader(cell + Vec3::Y + Vec3::Z);
    let h = hash_shader(cell + Vec3::ONE);
    let near = (a + (b - a) * w.x) + ((c + (d - c) * w.x) - (a + (b - a) * w.x)) * w.y;
    let far = (e + (f - e) * w.x) + ((g + (h - g) * w.x) - (e + (f - e) * w.x)) * w.y;
    near + (far - near) * w.z - 0.5
}

fn select_species(direction: DVec3, height: f64, random: &mut Random) -> Option<Species> {
    let dir = direction.as_vec3();
    let latitude = dir.y.abs();
    let weather = noise3(dir * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let patches = noise3(dir * 17.0 + Vec3::new(-2.1, 9.4, 4.6));
    let moisture = (0.50 + 0.96 * weather + 0.32 * patches).clamp(0.0, 1.0);
    let temp = (1.07 - 0.78 * latitude - (height.max(0.0) as f32) / 16_000.0
        + 0.13 * noise3(dir * 4.1 + Vec3::new(3.2, 18.6, -8.4))).clamp(0.0, 1.0);
    // Smooth, spatially coherent groves, not isolated dice-roll trees.
    // The broad noise varies on a scale of a few hundred metres and makes
    // forest clearings and densely packed stands.
    let grove = noise3(dir * (RADIUS as f32 / 260.0) + Vec3::new(7.4, -12.8, 3.7));
    let density = (0.82 + grove * 0.90).clamp(0.25, 1.0);
    let roll = random.unit();
    if height > 2_600.0 || latitude > 0.96 || temp < 0.13 { return None }
    if temp > 0.68 && moisture > 0.49 && roll < density * 0.98 {
        return Some(Species::Jungle);
    }
    if temp > 0.66 && (0.19..0.61).contains(&moisture) && roll < density * 0.34 {
        return Some(Species::Acacia);
    }
    if temp > 0.29 && moisture > 0.41 && roll < density * 0.94 {
        return if random.unit() < 0.77 { Some(Species::Oak) } else { Some(Species::Pine) };
    }
    if temp <= 0.46 && moisture > 0.27 && roll < density * 0.87 {
        return Some(Species::Pine);
    }
    None
}

struct Random(u64);
impl Random {
    fn new(seed: u64) -> Self { Self(seed ^ 0x9e3779b97f4a7c15) }
    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(2685821657736338717) >> 32) as u32
    }
    fn unit(&mut self) -> f32 { self.next_u32() as f32 / u32::MAX as f32 }
    fn unit64(&mut self) -> f64 { f64::from(self.next_u32()) / f64::from(u32::MAX) }
    fn range(&mut self, low: f32, high: f32) -> f32 { low + (high - low) * self.unit() }
}

#[derive(Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

impl MeshBuilder {
    fn vertex(&mut self, position: Vec3, normal: Vec3, uv: Vec2, color: [f32; 4]) -> u32 {
        let index = self.positions.len() as u32;
        self.positions.push(position.to_array());
        self.normals.push(normal.normalize_or_zero().to_array());
        self.uvs.push(uv.to_array());
        self.colors.push(color);
        index
    }
    fn triangle(&mut self, a: u32, b: u32, c: u32) { self.indices.extend([a, b, c]); }
    fn tube(&mut self, start: Vec3, end: Vec3, radius_a: f32, radius_b: f32, sides: usize, color: [f32; 4]) {
        let vector = end - start;
        if vector.length_squared() < 0.0001 { return }
        let along = vector.normalize();
        let reference = if along.y.abs() > 0.90 { Vec3::X } else { Vec3::Y };
        let tangent = along.cross(reference).normalize();
        let bitangent = along.cross(tangent).normalize();
        let base = self.positions.len() as u32;
        for row in 0..2 {
            let radius = if row == 0 { radius_a } else { radius_b };
            let center = if row == 0 { start } else { end };
            for side in 0..=sides {
                let angle = std::f32::consts::TAU * side as f32 / sides as f32;
                let radial = tangent * angle.cos() + bitangent * angle.sin();
                let normal = (radial + along * ((radius_a - radius_b) / vector.length())).normalize();
                let varied = 0.87 + 0.13 * (angle * 3.0).cos();
                let tint = [color[0] * varied, color[1] * varied, color[2] * varied, 1.0];
                self.vertex(center + radial * radius, normal, Vec2::new(side as f32 / sides as f32, row as f32), tint);
            }
        }
        let stride = sides as u32 + 1;
        for side in 0..sides as u32 {
            let a = base + side;
            let b = a + stride;
            self.triangle(a, a + 1, b);
            self.triangle(a + 1, b + 1, b);
        }
    }
    fn leaf(&mut self, center: Vec3, direction: Vec3, length: f32, width: f32, color: [f32; 4]) {
        let forward = direction.normalize_or_zero();
        if forward.length_squared() < 0.5 { return }
        let reference = if forward.y.abs() > 0.93 { Vec3::X } else { Vec3::Y };
        let sideways = forward.cross(reference).normalize();
        let normal = sideways.cross(forward).normalize();
        let base = center - forward * (length * 0.43);
        let tip = center + forward * (length * 0.57);
        let lifted = center + normal * length * 0.12;
        let a = self.vertex(base, normal, Vec2::new(0.5, 1.0), color);
        let b = self.vertex(lifted - sideways * width, normal, Vec2::new(0.0, 0.52), color);
        let c = self.vertex(tip, normal, Vec2::new(0.5, 0.0), color);
        let d = self.vertex(lifted + sideways * width, normal, Vec2::new(1.0, 0.52), color);
        self.triangle(a, b, c);
        self.triangle(a, c, d);
    }
    // Layered perpendicular cutout cards give each branch a leafy 3D crown.
    // Far cheaper than representing every tiny leaf as an independent polygon.
    fn foliage_lobe(&mut self, center: Vec3, extent: f32, color: [f32; 4], rng: &mut Random, lod: usize) {
        let card_count = if lod == 2 { 1 } else { 2 };
        let angle = rng.range(0.0, std::f32::consts::TAU);
        for axis in 0..card_count {
            let turn = angle + axis as f32 * std::f32::consts::FRAC_PI_2;
            let across = Vec3::new(turn.cos(), 0.0, turn.sin());
            let normal = Vec3::Y.cross(across).normalize();
            let half_width = extent * if axis == 0 { 1.0 } else { 0.85 };
            let half_height = extent * 0.73;
            let tint = 0.88 + rng.range(0.0, 0.22);
            let shaded = [
                (color[0] * tint).min(1.0),
                (color[1] * tint).min(1.0),
                (color[2] * tint).min(1.0),
                1.0,
            ];
            let positions = [
                center - across * half_width - Vec3::Y * half_height,
                center + across * half_width - Vec3::Y * half_height,
                center + across * half_width + Vec3::Y * half_height,
                center - across * half_width + Vec3::Y * half_height,
            ];
            let base = self.positions.len() as u32;
            for (point, uv) in positions.into_iter().zip([
                Vec2::new(0.0, 1.0), Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0), Vec2::new(0.0, 0.0),
            ]) {
                self.vertex(point, normal, uv, shaded);
            }
            self.triangle(base, base + 1, base + 2);
            self.triangle(base, base + 2, base + 3);
        }
    }

    fn crossed_cards(&mut self, height: f32, width: f32) {
        for rotation in [0.0_f32, std::f32::consts::FRAC_PI_2] {
            let axis = Vec3::new(rotation.cos(), 0.0, rotation.sin());
            let normal = Vec3::Y.cross(axis).normalize();
            let corners = [
                (-width * 0.5, 0.0, Vec2::new(0.0, 1.0)),
                ( width * 0.5, 0.0, Vec2::new(1.0, 1.0)),
                ( width * 0.5, height, Vec2::new(1.0, 0.0)),
                (-width * 0.5, height, Vec2::new(0.0, 0.0)),
            ];
            let vertices: Vec<u32> = corners.iter().map(|(x,y,uv)|
                self.vertex(axis * *x + Vec3::Y * *y, normal, *uv, [1.0; 4])
            ).collect();
            self.triangle(vertices[0],vertices[1],vertices[2]);
            self.triangle(vertices[0],vertices[2],vertices[3]);
        }
    }
    fn finish(self) -> Mesh {
        let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colors);
        mesh.insert_indices(Indices::U32(self.indices));
        mesh
    }
}

fn tree_height(species: Species, variant: u64) -> f32 {
    let offset = variant as f32 * 0.82;
    match species {
        Species::Oak => 14.0 + offset,
        Species::Pine => 19.0 + offset,
        Species::Acacia => 11.5 + offset,
        Species::Jungle => 25.0 + offset,
    }
}

fn generate_tree(species: Species, variant: u64, lod: usize) -> (MeshBuilder, MeshBuilder) {
    let mut wood = MeshBuilder::default();
    let mut leaves = MeshBuilder::default();
    let (bark, green) = species.colors();
    let bark = [bark[0],bark[1],bark[2],1.0];
    let height = tree_height(species, variant);
    let mut rng = Random::new(0xb5ee2dc653b6f7d9 ^ ((species as u64) << 20) ^ variant);
    let sides = [8, 6, 4][lod];
    let trunk_radius = match species { Species::Jungle => 0.75, Species::Oak=>0.65, Species::Pine=>0.44,Species::Acacia=>0.52 };
    let trunk_length = match species { Species::Pine => height * 0.92, Species::Jungle=>height * 0.78,Species::Oak=>height * 0.72,Species::Acacia=>height * 0.57};
    let trunk_segments = [7, 5, 3][lod];
    // Extend the root below ground to hide tile-LOD height differences.
    // The visible trunk starts at ground level as before.
    wood.tube(
        Vec3::Y * -2.6,
        Vec3::Y * 0.10,
        trunk_radius * 1.18,
        trunk_radius,
        sides,
        bark,
    );
    let mut prev = Vec3::ZERO;
    for segment in 1..=trunk_segments {
        let t = segment as f32 / trunk_segments as f32;
        let end = Vec3::new(
            0.35 * (t * 3.2 + variant as f32).sin() * t,
            t * trunk_length,
            0.27 * (t * 4.0 + variant as f32 * 0.7).cos() * t,
        );
        let before = (segment - 1) as f32 / trunk_segments as f32;
        wood.tube(prev,end,trunk_radius*(1.0-0.72*before),trunk_radius*(1.0-0.72*t),sides,bark);
        prev=end;
    }
    if matches!(species,Species::Jungle) && lod == 0 {
        for i in 0..5 {
            let angle = i as f32 * std::f32::consts::TAU / 5.0;
            let root = Vec3::new(angle.cos()*2.1,0.04,angle.sin()*2.1);
            wood.tube(root,Vec3::Y*3.4,0.16,0.34,5,bark);
        }
    }

    let levels = match species {
        Species::Pine => [9,6,4][lod],
        Species::Acacia => [3,3,2][lod],
        _ => [5,4,2][lod],
    };
    let twigs = if matches!(species, Species::Pine) {
        [2, 1, 1][lod]
    } else {
        [3, 2, 1][lod]
    };
    let leaves_per_cluster = match species {
        Species::Pine => [8, 5, 3][lod],
        _ => [13, 7, 3][lod],
    };
    for tier in 0..levels {
        let t = (tier as f32 + 1.0)/(levels as f32 + 1.0);
        let (branch_y, length, branches) = match species {
            Species::Oak => (height*(0.36+0.45*t), (height*0.40)*(1.0-0.53*t), 5),
            Species::Pine => (height*(0.18+0.67*t), height*0.29*(1.0-0.75*t), 6),
            Species::Acacia => (height*(0.45+0.17*t), height*0.53*(1.0-0.21*t), 5),
            Species::Jungle => (height*(0.58+0.32*t), height*0.34*(1.0-0.39*t), 5),
        };
        for branch in 0..branches {
            let angle = std::f32::consts::TAU * (branch as f32 / branches as f32 + t * 0.37)
                + rng.range(-0.18,0.18);
            let horizontal = Vec3::new(angle.cos(),0.0,angle.sin());
            let base = Vec3::new(0.0,branch_y,0.0);
            let lift = match species {Species::Acacia=>0.06,Species::Pine=>-0.10,_=>0.24};
            let middle=base + horizontal*length*0.51 + Vec3::Y*length*lift;
            let endpoint=base + horizontal*length + Vec3::Y*length*(lift+0.16);
            let radius = match species {Species::Acacia=>0.22,Species::Pine=>0.16,_=>0.21}*(1.0-0.38*t);
            wood.tube(base,middle,radius,radius*0.67,sides.min(6),bark);
            wood.tube(middle,endpoint,radius*0.67,radius*0.25,sides.min(6),bark);
            for twig in 0..twigs {
                let offset = (twig as f32 - (twigs-1) as f32 * 0.5) * 0.65;
                let twig_dir = Vec3::new((angle+offset).cos(),0.12+rng.range(-0.1,0.4),(angle+offset).sin()).normalize();
                let root = middle.lerp(endpoint,0.49 + twig as f32*0.19/twigs as f32);
                let tip = root + twig_dir*(length * rng.range(0.18,0.39));
                if lod < 2 { wood.tube(root,tip,radius*0.29,0.012,sides.min(5),bark); }
                let cluster_radius = match species {Species::Pine=>length*0.20,Species::Acacia=>length*0.27,_=>length*0.28};
                leaf_cluster(&mut leaves,tip,cluster_radius,leaves_per_cluster,species,green,&mut rng,lod);
            }
        }
    }
    // Low undergrowth attached to jungle trees makes the ground-level view
    // feel like a rainforest rather than bare poles below the canopy.
    if matches!(species, Species::Jungle) {
        let shrubs = [5, 3, 0][lod];
        for i in 0..shrubs {
            let angle = i as f32 * std::f32::consts::TAU / shrubs as f32 + rng.range(-0.24, 0.24);
            let center = Vec3::new(
                angle.cos() * rng.range(1.0, 3.1),
                rng.range(1.25, 3.4),
                angle.sin() * rng.range(1.0, 3.1),
            );
            leaves.foliage_lobe(center, rng.range(1.4, 2.5), [0.12, 0.37, 0.13, 1.0], &mut rng, lod);
            for _ in 0..3 {
                let leaf_dir = Vec3::new(
                    rng.range(-1.0, 1.0), rng.range(0.0, 0.8), rng.range(-1.0, 1.0)
                ).normalize();
                leaves.leaf(center + leaf_dir * 0.6, leaf_dir, 1.6, 0.45, [0.15, 0.46, 0.16, 1.0]);
            }
        }
    }
    (wood, leaves)
}

fn leaf_cluster(
    leaves: &mut MeshBuilder,
    center: Vec3,
    radius: f32,
    count: usize,
    species: Species,
    green: [f32;3],
    rng: &mut Random,
    lod: usize,
) {
    let lobe_size = if matches!(species, Species::Acacia) { radius * 0.85 }
        else if matches!(species, Species::Pine) { radius * 0.72 }
        else { radius * 1.18 };
    leaves.foliage_lobe(
        center, lobe_size.max(0.72),
        [green[0], green[1], green[2], 1.0], rng, lod
    );
    for _ in 0..count {
        let angle = rng.range(0.0,std::f32::consts::TAU);
        let vertical = rng.range(-0.85,0.90);
        let horizontal = (1.0-vertical*vertical).sqrt();
        let normal = Vec3::new(angle.cos()*horizontal,vertical,angle.sin()*horizontal);
        let position = center + normal * radius * rng.range(0.24,1.0);
        let leaf_direction = (normal + Vec3::Y*rng.range(-0.3,0.5)).normalize();
        let (length,width) = match species {
            Species::Pine => (0.65,0.13),
            Species::Acacia => (0.54,0.21),
            Species::Jungle => (1.18,0.47),
            Species::Oak => (0.79,0.32),
        };
        let size = rng.range(0.73,1.30) * if lod == 2 { 3.3 } else if lod==1 {1.6} else {1.0};
        let tint = rng.range(0.71,1.22);
        let color = [
            (green[0]*tint).min(1.0),
            (green[1]*tint).min(1.0),
            (green[2]*tint).min(1.0),
            1.0,
        ];
        leaves.leaf(position,leaf_direction,length*size,width*size,color);
    }
}

fn foliage_cluster_texture() -> Image {
    // Alpha-cutout "leaf cloud": irregular lobed silhouette with small holes.
    // White RGB leaves the species-specific vertex tint in control.
    const SIZE: usize = 96;
    let mut rgba = vec![0_u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let px = (x as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let py = (y as f32 + 0.5) / SIZE as f32 * 2.0 - 1.0;
            let theta = py.atan2(px);
            let radius = (px * px + py * py).sqrt();
            let boundary = 0.79 + 0.11 * (theta * 7.0).sin()
                + 0.075 * (theta * 13.0 + 0.9).cos()
                + 0.035 * (theta * 23.0).sin();
            let irregular = (px * 23.7).sin() * (py * 29.1).cos();
            let perforation = (px * 41.0 + py * 17.0).sin()
                * (py * 36.0 - px * 11.0).cos();
            let solid = radius < boundary + 0.025 * irregular
                && (radius < 0.53 || perforation > -0.81);
            let index = (y * SIZE + x) * 4;
            rgba[index] = 255;
            rgba[index + 1] = 255;
            rgba[index + 2] = 255;
            rgba[index + 3] = if solid { 255 } else { 0 };
        }
    }
    let mut image = Image::new_fill(
        Extent3d { width: SIZE as u32, height: SIZE as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.data = rgba;
    image
}

fn impostor_texture(species: Species) -> Image {
    // Small deterministic RGBA silhouette atlas. No downloaded assets or
    // runtime rendering of textures is needed for the last LOD.
    const SIZE: usize = 128;
    let (bark, green) = species.colors();
    let mut rgba = vec![0u8; SIZE*SIZE*4];
    let mut paint = |x: usize, y: usize, color: [f32;3], alpha: u8| {
        if x>=SIZE || y>=SIZE {return}
        let idx=(y*SIZE+x)*4;
        rgba[idx]= (color[0]*255.0).min(255.0) as u8;
        rgba[idx+1]= (color[1]*255.0).min(255.0) as u8;
        rgba[idx+2]= (color[2]*255.0).min(255.0) as u8;
        rgba[idx+3]=alpha;
    };
    let trunk_width= if matches!(species, Species::Jungle) { 5 } else { 3 };
    for y in 39..120 {
        for x in (64-trunk_width)..=(64+trunk_width) {paint(x,y,bark,255);}
    }
    let mut rng=Random::new((species as u64+1)*79471);
    let (crown_y,spread_x,spread_y) = match species {
        Species::Oak=>(57.0,45.0,34.0),
        Species::Pine=>(63.0,28.0,53.0),
        Species::Acacia=>(42.0,57.0,18.0),
        Species::Jungle=>(47.0,43.0,33.0),
    };
    for _ in 0..150 {
        let angle = rng.range(0.0,std::f32::consts::TAU);
        let r = rng.unit().sqrt();
        let center_x=64.0+angle.cos()*r*spread_x;
        let center_y=crown_y+angle.sin()*r*spread_y;
        let radius=rng.range(5.5,13.0);
        for iy in 0..SIZE {
            for ix in 0..SIZE {
                let distance = ((ix as f32-center_x)/radius).powi(2)
                    + ((iy as f32-center_y)/(radius*0.75)).powi(2);
                if distance<1.0 {
                    let shade=rng.range(0.77,1.12);
                    paint(ix,iy,[green[0]*shade,green[1]*shade,green[2]*shade],255);
                }
            }
        }
    }
    let mut image=Image::new_fill(
        Extent3d{width:SIZE as u32,height:SIZE as u32,depth_or_array_layers:1},
        TextureDimension::D2,
        &[0,0,0,0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.data=rgba;
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_are_deterministic_and_wrap_longitude() {
        let a = nearby_patches(DVec3::new(-0.35249,0.82882,0.43451).normalize());
        let b = nearby_patches(DVec3::new(-0.35249,0.82882,0.43451).normalize());
        assert_eq!(a,b);
        for k in a { assert!(k.col>=0 && k.col<longitude_cells(k.row)); }
    }
    #[test]
    fn lods_are_monotonic() {
        assert_eq!(select_lod(0.0),Some(0));
        assert_eq!(select_lod(49.0),Some(0));
        assert_eq!(select_lod(75.0),Some(1));
        assert_eq!(select_lod(100.0),Some(2));
        assert_eq!(select_lod(150.0),Some(2));
        assert_eq!(select_lod(250.0),Some(3));
        assert_eq!(select_lod(350.0),Some(3));
        assert_eq!(select_lod(650.0),None);
        assert_eq!(select_lod(900.0),None);
    }
    #[test]
    fn forest_view_range_is_shorter_in_pov() {
        assert_eq!(forest_range(true), FIRST_PERSON_RANGE);
        assert_eq!(forest_range(false), THIRD_PERSON_RANGE);
        assert!(forest_range(true) < forest_range(false));
        assert_eq!(select_lod(49.0), Some(0));
        assert_eq!(select_lod(75.0), Some(1));
        assert_eq!(select_lod(120.0), Some(2));
        assert_eq!(select_lod(200.0), Some(3));
    }

    #[test]
    fn spherical_horizon_rejects_the_far_side() {
        let eye = DVec3::X * (RADIUS + 80.0);
        let near = DVec3::X * (RADIUS + 20.0);
        let far = -near;
        assert!(within_horizon(eye, near, 10.0));
        assert!(!within_horizon(eye, far, 10.0));
    }

    #[test]
    fn camera_cone_rejects_trees_behind_the_view() {
        let perspective = Projection::Perspective(PerspectiveProjection {
            fov: std::f32::consts::FRAC_PI_3,
            aspect_ratio: 16.0 / 9.0,
            ..default()
        });
        let eye = DVec3::ZERO;
        assert!(within_camera_cone(
            eye, DVec3::NEG_Z, &perspective, DVec3::NEG_Z * 120.0, 20.0
        ));
        assert!(!within_camera_cone(
            eye, DVec3::NEG_Z, &perspective, DVec3::Z * 120.0, 20.0
        ));
    }

    #[test]
    fn procedural_foliage_atlas_contains_cutout_and_opaque_leaves() {
        let atlas = foliage_cluster_texture();
        let pixels = atlas.data.chunks_exact(4).count();
        let opaque = atlas.data.chunks_exact(4)
            .filter(|pixel| pixel[3] == 255).count();
        assert!(opaque > pixels / 4 && opaque < pixels * 3 / 4);
    }

    #[test]
    fn models_have_reasonable_triangle_counts() {
        for species in Species::ALL {
            let (near,leaves)=generate_tree(species,0,0);
            let (mid,mid_leaves)=generate_tree(species,0,1);
            let (far,far_leaves)=generate_tree(species,0,2);
            let near_count=(near.indices.len()+leaves.indices.len())/3;
            let mid_count=(mid.indices.len()+mid_leaves.indices.len())/3;
            let far_count=(far.indices.len()+far_leaves.indices.len())/3;
            assert!(near_count>mid_count && mid_count>far_count);
            assert!(near_count<6000,"{species:?} has {near_count} triangles");
        }
    }
}