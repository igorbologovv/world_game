//! Shared vegetation models, material integration, and biome placement rules.
pub(crate) mod canopy;
mod debug;
mod fern;
mod ground_cover;
mod hero_oak;
mod impostor;
mod lod;
mod palm;
mod placement;
mod proxy;
mod streaming;
mod textures;
mod tree_assets;

use bevy::{
    asset::RenderAssetUsages,
    camera::{
        primitives::{Aabb, MeshAabb},
        visibility::VisibilitySystems,
    },
    math::DVec3,
    pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin},
    prelude::*,
    reflect::TypePath,
    render::{
        mesh::Indices,
        render_resource::{AsBindGroup, PrimitiveTopology},
    },
    shader::ShaderRef,
};
use world_core::{
    planet::{LEGACY_BAKED_RADIUS, RADIUS, SEA_LEVEL},
    runtime::WorldMovement,
    SurfaceHeights,
};

pub use fern::FernSettings;
pub use lod::VegetationSettings;

const VARIANTS: usize = 3;
const FOLIAGE_CUTOFF: f32 = 0.39;

type FoliageMaterial = ExtendedMaterial<StandardMaterial, Wind>;

#[derive(Asset, AsBindGroup, TypePath, Clone)]
struct Wind {
    #[uniform(100)]
    coverage: Vec4,
}

impl Default for Wind {
    fn default() -> Self {
        Self {
            coverage: Vec4::new(0.0, 1.0, 0.0, 0.0),
        }
    }
}

impl MaterialExtension for Wind {
    fn vertex_shader() -> ShaderRef {
        "shaders/foliage.wesl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/vegetation_fade.wesl".into()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
enum Species {
    Oak = 0,
    Pine = 1,
    Acacia = 2,
    Jungle = 3,
    Palm = 4,
}

impl Species {
    const ALL: [Self; 5] = [
        Self::Oak,
        Self::Pine,
        Self::Acacia,
        Self::Jungle,
        Self::Palm,
    ];

    const fn variants(self) -> usize {
        match self {
            Self::Palm => 1,
            _ => VARIANTS,
        }
    }

    fn colors(self) -> ([f32; 3], [f32; 3]) {
        match self {
            Self::Oak => ([0.35, 0.22, 0.12], [0.23, 0.53, 0.18]),
            Self::Pine => ([0.30, 0.20, 0.13], [0.13, 0.34, 0.23]),
            Self::Acacia => ([0.34, 0.25, 0.15], [0.42, 0.55, 0.19]),
            Self::Jungle => ([0.29, 0.20, 0.14], [0.14, 0.48, 0.17]),
            Self::Palm => ([0.39, 0.30, 0.22], [0.22, 0.49, 0.13]),
        }
    }
}

struct WoodLod {
    mesh: Handle<Mesh>,
    bounds: Aabb,
}

struct ModelLod {
    wood: Option<WoodLod>,
    foliage: Handle<Mesh>,
    foliage_material: Handle<FoliageMaterial>,
    foliage_bounds: Aabb,
}

impl ModelLod {
    fn new(
        wood: Option<Mesh>,
        foliage: Mesh,
        foliage_material: Handle<FoliageMaterial>,
        meshes: &mut Assets<Mesh>,
    ) -> Self {
        let wood = wood.map(|mesh| WoodLod {
            bounds: mesh.get_aabb().expect("trunks contain vertices"),
            mesh: meshes.add(mesh),
        });
        let foliage_bounds = foliage.get_aabb().expect("canopies contain vertices");
        Self::from_parts(wood, meshes.add(foliage), foliage_material, foliage_bounds)
    }

    fn from_parts(
        wood: Option<WoodLod>,
        foliage: Handle<Mesh>,
        foliage_material: Handle<FoliageMaterial>,
        mut foliage_bounds: Aabb,
    ) -> Self {
        foliage_bounds.half_extents += 0.22;
        Self {
            wood,
            foliage,
            foliage_material,
            foliage_bounds,
        }
    }
}

struct VariantModels {
    lods: Vec<ModelLod>,
    bark: Option<Handle<StandardMaterial>>,
}

#[derive(Resource)]
struct ForestModels {
    variants: Vec<Vec<VariantModels>>,
    bark_material: Handle<StandardMaterial>,
    hero_oak: Option<hero_oak::HeroOak>,
    portraits: impostor::Portraits,
}

impl ForestModels {
    fn model(
        &self,
        species: Species,
        variant: usize,
        level: usize,
        use_hero: bool,
    ) -> (&ModelLod, &Handle<StandardMaterial>) {
        if use_hero {
            let hero = self
                .hero_oak
                .as_ref()
                .expect("hero models stay resident after loading");
            return (&hero.lods[level], &hero.bark);
        }
        let model = &self.variants[species as usize][variant];
        (
            &model.lods[level],
            model.bark.as_ref().unwrap_or(&self.bark_material),
        )
    }
}

/// Reserve space for an authored scene among the procedurally placed trees.
///
/// Insert before the forest starts streaming. All generated tree LODs honor it.
#[derive(Clone, Copy, Resource)]
pub struct ForestClearing {
    /// Terrain-aligned unit radial direction at the clearing center.
    pub direction: DVec3,
    /// Radius on the reference sphere in meters.
    pub radius: f64,
}

impl ForestClearing {
    fn contains(&self, tree: DVec3) -> bool {
        let radial = tree.normalize();
        radial.distance_squared(self.direction) * RADIUS.powi(2) < self.radius.powi(2)
    }
}

/// The root of a resident, individually streamed coconut palm.
#[derive(Component)]
pub struct CoconutPalm;

/// Stream deterministic foliage around the world camera.
///
/// Install with the shared world space plugin. Tune ranges through [`VegetationSettings`].
/// Press F8 to toggle the vegetation debug overlay.
pub struct ProceduralForestPlugin;

impl Plugin for ProceduralForestPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<FoliageMaterial> { ..default() },
            MaterialPlugin::<lod::BarkMaterial> { ..default() },
            MaterialPlugin::<proxy::ProxyMaterial> { ..default() },
        ))
        .add_plugins(fern::FernPlugin)
        .init_resource::<VegetationSettings>()
        .init_resource::<streaming::ForestStream>()
        .init_resource::<lod::FadeMaterials>()
        .init_resource::<debug::LodDiagnostics>()
        .add_systems(
            Startup,
            (
                build_models,
                hero_oak::request_assets,
                tree_assets::request_assets,
                debug::spawn,
            ),
        )
        .add_systems(
            Update,
            (
                hero_oak::finish_loading,
                tree_assets::finish_loading,
                streaming::stream,
            )
                .chain()
                .after(WorldMovement),
        )
        .add_systems(Update, debug::update.after(fern::FernStreaming))
        .add_systems(
            PostUpdate,
            lod::update_lods
                .after(VisibilitySystems::UpdateFrusta)
                .before(VisibilitySystems::VisibilityPropagate),
        );
        ground_cover::install(app);
        canopy::install(app);
    }
}

fn build_models(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut foliage_materials: ResMut<Assets<FoliageMaterial>>,
) {
    let bark_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: Some(images.add(textures::bark_texture())),
        perceptual_roughness: 0.94,
        ..default()
    });
    let mut variants = Vec::with_capacity(Species::ALL.len());
    for species in Species::ALL {
        let foliage_image = images.add(textures::canopy_texture(species));
        let foliage_material = foliage_materials.add(cutout_material(foliage_image));
        let mut species_variants = Vec::with_capacity(species.variants());
        for variant in 0..species.variants() {
            let mut lods = Vec::with_capacity(3);
            for level in 0..3 {
                let (wood, leaves) = generate_tree(species, variant as u64, level);
                lods.push(ModelLod::new(
                    Some(wood.finish()),
                    leaves.finish_canopy(variant as f32),
                    foliage_material.clone(),
                    &mut meshes,
                ));
            }
            species_variants.push(VariantModels { lods, bark: None });
        }
        variants.push(species_variants);
    }
    commands.insert_resource(ForestModels {
        variants,
        bark_material,
        hero_oak: None,
        portraits: default(),
    });
}

fn cutout_material(texture: Handle<Image>) -> FoliageMaterial {
    FoliageMaterial {
        base: StandardMaterial {
            base_color_texture: Some(texture),
            alpha_mode: AlphaMode::Mask(FOLIAGE_CUTOFF),
            perceptual_roughness: 0.80,
            reflectance: 0.16,
            diffuse_transmission: 0.34,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: Wind::default(),
    }
}

// Test the ray to the crown against the planet's sea-level sphere.
// A tree above the observer on a nearby slope can still be visible.
fn within_horizon(eye: DVec3, position: DVec3, extra_height: f64) -> bool {
    let crown = position + position.normalize_or_zero() * extra_height;
    let ray = crown - eye;
    let closest = (-eye.dot(ray) / ray.length_squared().max(1.0)).clamp(0.0, 1.0);
    (eye + ray * closest).length_squared() >= RADIUS * RADIUS
}

fn tangent_basis(up: DVec3) -> (DVec3, DVec3) {
    let reference = if up.y.abs() > 0.94 {
        DVec3::X
    } else {
        DVec3::Y
    };
    let east = up.cross(reference).normalize();
    (east, east.cross(up).normalize())
}

// Mirrors the WGSL macro_noise and its climate inputs, to avoid trees in deserts
// or tundra when the terrain shader draws a different material there.
fn hash_shader(p: Vec3) -> f32 {
    let scaled = p * Vec3::new(0.1031, 0.1030, 0.0973);
    let mut h = scaled - scaled.floor();
    let other = Vec3::new(h.y, h.x, h.z) + Vec3::splat(33.33);
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

fn tree_site(
    heights: &SurfaceHeights,
    direction: DVec3,
    rng: &mut Random,
) -> Option<(f64, Species)> {
    let ground = heights.sample(direction)?;
    if !(3.5..=2600.0).contains(&ground) || heights.is_river(direction) {
        return None;
    }
    let species = select_species(direction, ground, rng, || {
        let (east, north) = tangent_basis(direction);
        near_sea(direction, east, north, |up| heights.sample_base(up))
    })?;
    let (east, north) = tangent_basis(direction);
    let offset = 4.0 / RADIUS;
    let rise = |axis: DVec3| {
        heights
            .sample((direction + axis * offset).normalize())
            .unwrap_or(ground)
            - ground
    };
    (rise(east).hypot(rise(north)) <= 4.0 * 0.58).then_some((ground, species))
}

fn near_sea(
    direction: DVec3,
    east: DVec3,
    north: DVec3,
    sample: impl Fn(DVec3) -> Option<f64>,
) -> bool {
    let diagonal = std::f64::consts::FRAC_1_SQRT_2;
    let east_north = (east + north) * diagonal;
    let east_south = (east - north) * diagonal;
    [
        east,
        -east,
        north,
        -north,
        east_north,
        -east_north,
        east_south,
        -east_south,
    ]
    .into_iter()
    .any(|axis| {
        // The baked coastline occupies the same angular region at either radius.
        let coastal_angle = 220.0 / LEGACY_BAKED_RADIUS;
        sample((direction + axis * coastal_angle).normalize())
            .is_some_and(|height| height < SEA_LEVEL)
    })
}

fn select_species(
    direction: DVec3,
    height: f64,
    random: &mut Random,
    coastal: impl FnOnce() -> bool,
) -> Option<Species> {
    let dir = direction.as_vec3();
    let latitude = dir.y.abs();
    let weather = noise3(dir * 5.8 + Vec3::new(17.3, -4.7, 11.8));
    let patches = noise3(dir * 17.0 + Vec3::new(-2.1, 9.4, 4.6));
    let moisture = (0.50 + 0.96 * weather + 0.32 * patches).clamp(0.0, 1.0);
    let temp = (1.07 - 0.78 * latitude - (height.max(0.0) as f32) / 16_000.0
        + 0.13 * noise3(dir * 4.1 + Vec3::new(3.2, 18.6, -8.4)))
    .clamp(0.0, 1.0);
    let density = grove_density(dir);
    species_for_climate(latitude, temp, moisture, height, density, random, coastal)
}

// A 260 m grove field shared by tree placement and understory habitat.
fn grove_density(dir: Vec3) -> f32 {
    let grove = noise3(dir * (RADIUS as f32 / 260.0) + Vec3::new(7.4, -12.8, 3.7));
    (0.82 + grove * 0.90).clamp(0.25, 1.0)
}

fn species_for_climate(
    latitude: f32,
    temp: f32,
    moisture: f32,
    height: f64,
    density: f32,
    random: &mut Random,
    coastal: impl FnOnce() -> bool,
) -> Option<Species> {
    let roll = random.unit();
    if height > 2_600.0 || latitude > 0.96 || temp < 0.13 {
        return None;
    }
    let tropical_palm = temp > 0.78 && moisture > 0.50 && height <= 420.0 && roll < density * 0.22;
    let coastal_palm =
        || temp > 0.70 && moisture > 0.25 && height <= 45.0 && roll < density * 0.58 && coastal();
    if tropical_palm || coastal_palm() {
        return Some(Species::Palm);
    }
    if temp > 0.68 && moisture > 0.49 && roll < density * 0.98 {
        return Some(Species::Jungle);
    }
    if temp > 0.66 && (0.19..0.61).contains(&moisture) && roll < density * 0.34 {
        return Some(Species::Acacia);
    }
    if (0.29..=0.68).contains(&temp) && moisture > 0.41 && roll < density * 0.94 {
        return if random.unit() < 0.77 {
            Some(Species::Oak)
        } else {
            Some(Species::Pine)
        };
    }
    if temp <= 0.46 && moisture > 0.27 && roll < density * 0.87 {
        return Some(Species::Pine);
    }
    None
}

struct Random(u64);
impl Random {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e3779b97f4a7c15)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(2685821657736338717) >> 32) as u32
    }
    fn unit(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
    fn unit64(&mut self) -> f64 {
        f64::from(self.next_u32()) / f64::from(u32::MAX)
    }
    fn range(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.unit()
    }
}

#[derive(Clone, Default)]
struct MeshBuilder {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 4]>,
    wind: Vec<[f32; 2]>,
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
    fn triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend([a, b, c]);
    }
    fn tube(
        &mut self,
        start: Vec3,
        end: Vec3,
        radius_a: f32,
        radius_b: f32,
        sides: usize,
        color: [f32; 4],
    ) {
        self.tube_path(&[(start, radius_a), (end, radius_b)], sides, color);
    }

    fn tube_path(&mut self, nodes: &[(Vec3, f32)], sides: usize, color: [f32; 4]) {
        let last = nodes.len() - 1;
        let axis = (nodes[last].0 - nodes[0].0).normalize_or_zero();
        if axis == Vec3::ZERO {
            return;
        }
        let reference = if axis.y.abs() > 0.87 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let base = self.positions.len() as u32;
        let stride = sides as u32 + 1;
        let mut grain = nodes[0].0.dot(Vec3::new(0.31, 0.58, 0.29));
        for (row, &(center, radius)) in nodes.iter().enumerate() {
            let (before, wide) = nodes[row.saturating_sub(1)];
            let (after, narrow) = nodes[(row + 1).min(last)];
            let delta = after - before;
            let length = delta.length().max(0.0001);
            let along = delta / length;
            let tangent = along.cross(reference).normalize_or_zero();
            let bitangent = along.cross(tangent);
            if row > 0 {
                grain += center.distance(before) * 0.58;
            }
            for side in 0..=sides {
                let u = side as f32 / sides as f32;
                let angle = std::f32::consts::TAU * u;
                let (sin, cos) = angle.sin_cos();
                let radial = tangent * cos + bitangent * sin;
                let normal = radial + along * ((wide - narrow) / length);
                let rib = (angle * 3.0).cos();
                let tint = 0.92 + 0.08 * rib;
                let shade = [color[0] * tint, color[1] * tint, color[2] * tint, 1.0];
                let edge = center + radial * radius * (1.0 + 0.055 * rib);
                self.vertex(edge, normal, Vec2::new(u, grain), shade);
            }
            if row > 0 {
                for side in 0..sides as u32 {
                    let a = base + (row - 1) as u32 * stride + side;
                    let b = a + stride;
                    self.triangle(a, a + 1, b);
                    self.triangle(a + 1, b + 1, b);
                }
            }
        }
    }

    fn leaf(&mut self, center: Vec3, direction: Vec3, length: f32, width: f32, color: [f32; 4]) {
        let forward = direction.normalize_or_zero();
        if forward.length_squared() < 0.5 {
            return;
        }
        let reference = if forward.y.abs() > 0.93 {
            Vec3::X
        } else {
            Vec3::Y
        };
        let sideways = forward.cross(reference).normalize();
        let normal = sideways.cross(forward).normalize();
        let base = center - forward * (length * 0.43);
        let tip = center + forward * (length * 0.57);
        let lifted = center + normal * length * 0.12;
        let uv = |u, v| textures::canopy_uv(Vec2::new(u, v), true);
        let a = self.vertex(base, normal, uv(0.5, 1.0), color);
        let b = self.vertex(lifted - sideways * width, normal, uv(0.0, 0.52), color);
        let c = self.vertex(tip, normal, uv(0.5, 0.0), color);
        let d = self.vertex(lifted + sideways * width, normal, uv(1.0, 0.52), color);
        self.triangle(a, c, b);
        self.triangle(a, d, c);
    }
    // Layered perpendicular cutout cards give each branch a leafy 3D crown.
    // Far cheaper than representing every tiny leaf as an independent polygon.
    fn foliage_lobe(
        &mut self,
        center: Vec3,
        extent: f32,
        aspect: f32,
        color: [f32; 4],
        rng: &mut Random,
    ) {
        let angle = rng.range(0.0, std::f32::consts::TAU);
        for axis in 0..2 {
            let turn = angle + axis as f32 * std::f32::consts::FRAC_PI_2;
            let across = Vec3::new(turn.cos(), 0.0, turn.sin());
            let rise = Vec3::Y + across.cross(Vec3::Y) * rng.range(-0.47, 0.47);
            let normal = across.cross(rise).normalize();
            let half_width = extent * if axis == 0 { 1.0 } else { 0.85 };
            let half_height = extent * aspect;
            let tint = 0.88 + rng.range(0.0, 0.22);
            let shaded = [
                (color[0] * tint).min(1.0),
                (color[1] * tint).min(1.0),
                (color[2] * tint).min(1.0),
                1.0,
            ];
            let dx = across * half_width;
            let dy = rise * half_height;
            let positions = [
                center - dx - dy,
                center + dx - dy,
                center + dx + dy,
                center - dx + dy,
            ];
            let base = self.positions.len() as u32;
            for (point, uv) in positions.into_iter().zip([
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 0.0),
            ]) {
                let light = 0.79 + (1.0 - uv.y) * 0.21;
                let shaded = [shaded[0] * light, shaded[1] * light, shaded[2] * light, 1.0];
                self.vertex(point, normal, textures::canopy_uv(uv, false), shaded);
            }
            self.triangle(base, base + 1, base + 2);
            self.triangle(base, base + 2, base + 3);
        }
    }

    fn finish_canopy(mut self, variant: f32) -> Mesh {
        self.wind = self
            .positions
            .iter()
            .map(|&[x, y, z]| {
                let weight = ((y - 2.5) * 0.009).clamp(0.0, 0.21);
                [weight, variant * 1.8 + x * 0.21 + z * 0.17 + y * 0.09]
            })
            .collect();
        self.finish()
    }

    fn finish(self) -> Mesh {
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, self.positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, self.normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, self.uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, self.colors);
        if !self.wind.is_empty() {
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, self.wind);
        }
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
        Species::Palm => 12.0,
    }
}

fn generate_tree(species: Species, variant: u64, lod: usize) -> (MeshBuilder, MeshBuilder) {
    if species == Species::Palm {
        return palm::procedural(lod);
    }
    let mut wood = MeshBuilder::default();
    let mut leaves = MeshBuilder::default();
    let (bark, green) = species.colors();
    let bark = [bark[0], bark[1], bark[2], 1.0];
    let height = tree_height(species, variant);
    let seed = 0xb5ee2dc653b6f7d9 ^ ((species as u64) << 20) ^ variant;
    let mut rng = Random::new(seed);
    let sides = [10, 7, 4][lod];
    let trunk_radius = match species {
        Species::Jungle => 0.75,
        Species::Oak => 0.65,
        Species::Pine => 0.44,
        Species::Acacia => 0.52,
        Species::Palm => unreachable!(),
    };
    let trunk_length = match species {
        Species::Pine => height * 0.92,
        Species::Jungle => height * 0.78,
        Species::Oak => height * 0.72,
        Species::Acacia => height * 0.57,
        Species::Palm => unreachable!(),
    };
    let trunk_segments = [7, 5, 3][lod];
    let lean = match species {
        Species::Pine => 0.42,
        Species::Jungle => 0.82,
        Species::Oak => 1.11,
        Species::Acacia => 1.75,
        Species::Palm => unreachable!(),
    };
    let turn = rng.range(0.0, std::f32::consts::TAU);
    let trunk = |t: f32| {
        let spiral = turn + t * 1.65;
        Vec3::new(
            spiral.cos() * lean * t,
            t * trunk_length,
            spiral.sin() * lean * t,
        )
    };
    // One connected tube avoids seams at bends and keeps the base sunk into the terrain.
    let mut trunk_nodes = Vec::with_capacity(trunk_segments + 2);
    trunk_nodes.push((Vec3::Y * -2.6, trunk_radius * 1.42));
    trunk_nodes.push((trunk((1.18 / trunk_length).min(0.12)), trunk_radius * 1.21));
    for segment in 1..=trunk_segments {
        let t = segment as f32 / trunk_segments as f32;
        trunk_nodes.push((trunk(t), trunk_radius * (1.0 - 0.88 * t)));
    }
    wood.tube_path(&trunk_nodes, sides, bark);
    let roots = if matches!(species, Species::Jungle) {
        6
    } else {
        4
    };
    let radius = if matches!(species, Species::Jungle) {
        2.8
    } else {
        1.56
    };
    for i in 0..roots {
        let angle = i as f32 * std::f32::consts::TAU / roots as f32 + turn + rng.range(-0.21, 0.21);
        let spread = Vec3::new(angle.cos(), 0.0, angle.sin()) * radius * rng.range(0.65, 1.15);
        // Consume the same root draws so crown randomness stays fixed at every LOD.
        if lod != 0 {
            continue;
        }
        let knee = Vec3::Y * 1.12 + spread * 0.50;
        let toe = spread + Vec3::Y * 0.48;
        let root_top = trunk((2.4 + radius * 0.42) / trunk_length);
        wood.tube_path(
            &[
                (toe, 0.07),
                (knee, trunk_radius * 0.34),
                (root_top, trunk_radius * 0.66),
            ],
            5,
            bark,
        );
    }

    let levels = match species {
        Species::Pine => 9,
        Species::Acacia => 3,
        _ => 5,
    };
    // Keep every crown tier and major lobe in its canonical position.
    let twigs = if species == Species::Pine { 2 } else { 3 };
    let leaves_per_cluster = match species {
        Species::Pine => [8, 5, 3][lod],
        _ => [13, 7, 3][lod],
    };
    for tier in 0..levels {
        let t = (tier as f32 + 1.0) / (levels as f32 + 1.0);
        let (branch_y, length, branches) = match species {
            Species::Oak => (
                height * (0.36 + 0.45 * t),
                (height * 0.40) * (1.0 - 0.53 * t),
                5,
            ),
            Species::Pine => (
                height * (0.18 + 0.67 * t),
                height * 0.29 * (1.0 - 0.75 * t),
                6,
            ),
            Species::Acacia => (
                height * (0.45 + 0.17 * t),
                height * 0.53 * (1.0 - 0.21 * t),
                5,
            ),
            Species::Jungle => (
                height * (0.58 + 0.32 * t),
                height * 0.34 * (1.0 - 0.39 * t),
                5,
            ),
            Species::Palm => unreachable!(),
        };
        for branch in 0..branches {
            let mut rng = Random::new(seed ^ (((tier * 8 + branch + 1) as u64) * 331_792_583));
            let angle = std::f32::consts::TAU * (branch as f32 / branches as f32 + t * 0.37)
                + turn
                + rng.range(-0.27, 0.27);
            let horizontal = Vec3::new(angle.cos(), 0.0, angle.sin());
            let length = length * rng.range(0.75, 1.21);
            let base =
                trunk(((branch_y + height * rng.range(-0.021, 0.021)) / trunk_length).min(1.0));
            let lift = match species {
                Species::Acacia => 0.06,
                Species::Pine => -0.10,
                _ => 0.24,
            };
            let middle = base + horizontal * length * 0.51 + Vec3::Y * length * lift;
            let endpoint = base + horizontal * length + Vec3::Y * length * (lift + 0.16);
            let radius = match species {
                Species::Acacia => 0.22,
                Species::Pine => 0.16,
                _ => 0.21,
            } * (1.0 - 0.38 * t);
            wood.tube_path(
                &[
                    (base, radius),
                    (middle, radius * 0.67),
                    (endpoint, radius * 0.25),
                ],
                sides.min(6),
                bark,
            );
            for twig in 0..twigs {
                let offset = [0.0, -0.61, 0.64][twig];
                let twig_dir = Vec3::new(
                    (angle + offset).cos(),
                    0.12 + rng.range(-0.1, 0.4),
                    (angle + offset).sin(),
                )
                .normalize();
                let root = middle.lerp(endpoint, 0.49 + twig as f32 * 0.09);
                let tip = root + twig_dir * (length * rng.range(0.18, 0.39));
                if lod < 2 {
                    wood.tube(root, tip, radius * 0.29, 0.012, sides.min(5), bark);
                }
                let cluster_radius = match species {
                    Species::Pine => length * 0.20,
                    Species::Acacia => length * 0.27,
                    _ => length * 0.28,
                };
                let mut foliage_random = Random::new(u64::from(rng.next_u32()));
                leaf_cluster(
                    &mut leaves,
                    tip,
                    cluster_radius,
                    leaves_per_cluster,
                    species,
                    green,
                    &mut foliage_random,
                );
            }
        }
    }
    let tip = trunk(1.0);
    let crown_radius = if matches!(species, Species::Pine) {
        height * 0.047
    } else {
        height * 0.10
    };
    leaf_cluster(
        &mut leaves,
        tip,
        crown_radius,
        leaves_per_cluster,
        species,
        green,
        &mut rng,
    );
    (wood, leaves)
}

fn leaf_cluster(
    leaves: &mut MeshBuilder,
    center: Vec3,
    radius: f32,
    count: usize,
    species: Species,
    green: [f32; 3],
    rng: &mut Random,
) {
    let lobe_size = if matches!(species, Species::Acacia) {
        radius * 0.85
    } else if matches!(species, Species::Pine) {
        radius * 0.72
    } else {
        radius * 1.18
    };
    leaves.foliage_lobe(
        center,
        lobe_size.max(0.72),
        match species {
            Species::Acacia => 0.49,
            Species::Pine => 0.90,
            _ => 0.73,
        },
        [green[0], green[1], green[2], 1.0],
        rng,
    );
    for _ in 0..count {
        let angle = rng.range(0.0, std::f32::consts::TAU);
        let vertical = rng.range(-0.85, 0.90);
        let horizontal = (1.0 - vertical * vertical).sqrt();
        let normal = Vec3::new(angle.cos() * horizontal, vertical, angle.sin() * horizontal);
        let position = center + normal * radius * rng.range(0.24, 1.0);
        let leaf_direction = (normal + Vec3::Y * rng.range(-0.3, 0.5)).normalize();
        let (length, width) = match species {
            Species::Pine => (0.65, 0.13),
            Species::Acacia => (0.54, 0.21),
            Species::Jungle => (1.18, 0.47),
            Species::Oak => (0.79, 0.32),
            Species::Palm => unreachable!(),
        };
        let size = rng.range(0.73, 1.30);
        let tint = rng.range(0.71, 1.22);
        let color = [
            (green[0] * tint).min(1.0),
            (green[1] * tint).min(1.0),
            (green[2] * tint).min(1.0),
            1.0,
        ];
        leaves.leaf(position, leaf_direction, length * size, width * size, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palms_favor_tropical_lowlands_and_warm_coasts() {
        // This seed produces a site within even the sparsest eligible groves.
        let pick = |temp, moisture, height, coastal| {
            species_for_climate(
                0.2,
                temp,
                moisture,
                height,
                1.0,
                &mut Random::new(0),
                || coastal,
            )
        };
        assert_eq!(pick(0.88, 0.72, 200.0, false), Some(Species::Palm));
        assert_eq!(pick(0.75, 0.31, 12.0, true), Some(Species::Palm));
        assert_eq!(pick(0.75, 0.31, 12.0, false), Some(Species::Acacia));
        assert_eq!(pick(0.88, 0.72, 900.0, false), Some(Species::Jungle));
        assert_eq!(pick(0.40, 0.65, 12.0, true), Some(Species::Oak));
        assert_eq!(pick(0.26, 0.42, 12.0, true), Some(Species::Pine));
        assert_eq!(pick(0.86, 0.10, 12.0, true), None);
    }

    #[test]
    fn shore_probe_detects_sea_level_water_on_the_diagonals() {
        let (east, north) = tangent_basis(DVec3::Y);
        assert!(near_sea(DVec3::Y, east, north, |up| Some(
            if up.dot(east) > 0.001 && up.dot(north) > 0.001 {
                -5.0
            } else {
                8.0
            }
        )));
        assert!(!near_sea(DVec3::Y, east, north, |_| Some(8.0)));
        assert!(!near_sea(DVec3::Y, east, north, |_| None));
    }

    #[test]
    fn spherical_horizon_rejects_the_far_side() {
        let eye = DVec3::X * (RADIUS + 80.0);
        let near = DVec3::X * (RADIUS + 20.0);
        let far = -near;
        assert!(within_horizon(eye, near, 10.0));
        assert!(within_horizon(
            eye,
            near + DVec3::new(350.0, 500.0, 0.0),
            10.0
        ));
        assert!(!within_horizon(eye, far, 10.0));
    }

    #[test]
    fn models_have_reasonable_triangle_counts() {
        for species in Species::ALL {
            let (near, leaves) = generate_tree(species, 0, 0);
            let (mid, mid_leaves) = generate_tree(species, 0, 1);
            let (far, far_leaves) = generate_tree(species, 0, 2);
            let near_count = (near.indices.len() + leaves.indices.len()) / 3;
            let mid_count = (mid.indices.len() + mid_leaves.indices.len()) / 3;
            let far_count = (far.indices.len() + far_leaves.indices.len()) / 3;
            assert!(near_count > mid_count && mid_count > far_count);
            assert!(near_count < 6000, "{species:?} has {near_count} triangles");
        }
    }
}
