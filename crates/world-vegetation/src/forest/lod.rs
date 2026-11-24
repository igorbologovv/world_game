//! Screen-space detail selection and shared, complementary dither materials.
use std::collections::HashMap;

use bevy::{
    asset::AssetId,
    camera::primitives::Aabb,
    light::{NotShadowCaster, NotShadowReceiver},
    pbr::{ExtendedMaterial, MaterialExtension},
    prelude::*,
    reflect::TypePath,
    render::render_resource::AsBindGroup,
    shader::ShaderRef,
};
use world_core::runtime::{
    GridTransform, PlanetRoot, ReferenceFrame, ReferenceFrames, WorldCamera,
};

use super::{
    placement::{PatchKey, Site},
    streaming::ForestStream,
    CoconutPalm, FoliageMaterial, ForestModels, Species,
};

const STEPS: f32 = 12.0;
const REFERENCE_FOCAL: f32 = 935.3074;
const REFERENCE_HEIGHT: f32 = 15.0;

/// Vegetation quality controls. Distances describe a 1080p, 60-degree reference view.
#[derive(Clone, Copy, Resource)]
pub struct VegetationSettings {
    /// Reference distances from detailed to simplified and from simplified to coarse meshes.
    pub mesh_distances: [f32; 2],
    /// Reference distances across which individual meshes give way to batched impostors.
    pub impostor_blend: [f32; 2],
    /// Approximate range of individual silhouettes before the parent forest mesh takes over.
    pub impostor_range: f32,
    /// Outer distance for the dithered forest cutoff, in meters.
    pub forest_range: f32,
    /// Extra radial distance to request chunks before they enter their display range.
    pub preload_margin: f32,
    /// Fractional hysteresis for the projected size thresholds.
    pub hysteresis: f32,
    /// Time in seconds to blend between resident representations.
    pub transition_seconds: f32,
}

impl Default for VegetationSettings {
    fn default() -> Self {
        Self {
            mesh_distances: [30.0, 62.0],
            impostor_blend: [84.0, 116.0],
            impostor_range: 400.0,
            forest_range: 2_000.0,
            preload_margin: 260.0,
            hysteresis: 0.10,
            transition_seconds: 0.30,
        }
    }
}

impl VegetationSettings {
    pub(super) fn proxy_range(&self) -> Vec2 {
        let impostor = self.impostor_range.clamp(200.0, 600.0);
        let end = self.forest_range.clamp(impostor + 220.0, 3_000.0);
        Vec2::new(0.79 * end, end)
    }
}

pub(super) struct View {
    pub focal: f32,
    pub orthographic: bool,
}

impl View {
    pub fn new(camera: &Camera, projection: &Projection) -> Self {
        let height = camera
            .physical_viewport_size()
            .map_or(1080.0, |size| size.y.max(1) as f32);
        let (focal, orthographic) = match projection {
            Projection::Perspective(view) => (height / (2.0 * (view.fov * 0.5).tan()), false),
            Projection::Orthographic(view) => (height / view.area.height().max(0.01), true),
            Projection::Custom(_) => (height, false),
        };
        Self {
            focal,
            orthographic,
        }
    }

    fn pixels(&self, site: Site, distance: f32) -> f32 {
        let size = site.height() * self.focal;
        if self.orthographic {
            size
        } else {
            size / distance.max(1.0)
        }
    }

    pub fn impostor_band(&self, settings: &VegetationSettings) -> Vec2 {
        let scale = if self.orthographic {
            1.0
        } else {
            (self.focal / REFERENCE_FOCAL).clamp(0.48, 1.55)
        };
        let start = settings.impostor_blend[0].clamp(15.0, 230.0);
        let end = settings.impostor_blend[1].clamp(start + 4.0, 260.0);
        Vec2::new(start, end) * scale
    }
}

fn pick_lod(settings: &VegetationSettings, pixels: f32, previous: Option<usize>) -> usize {
    let near = settings.mesh_distances[0].clamp(2.0, 250.0);
    let thresholds = [near, settings.mesh_distances[1].clamp(near + 1.0, 300.0)]
        .map(|distance| REFERENCE_HEIGHT * REFERENCE_FOCAL / distance);
    let Some(mut level) = previous else {
        return thresholds
            .iter()
            .position(|&threshold| pixels >= threshold)
            .unwrap_or(2);
    };
    let h = settings.hysteresis.clamp(0.0, 0.35);
    while level < 2 && pixels < thresholds[level] * (1.0 - h) {
        level += 1;
    }
    while level > 0 && pixels > thresholds[level - 1] * (1.0 + h) {
        level -= 1;
    }
    level
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelChoice {
    level: usize,
    hero: bool,
}

#[derive(Clone, Copy)]
struct Blend<T> {
    target: T,
    progress: f32,
}

#[derive(Clone, Copy)]
pub(super) struct LodState<T> {
    pub desired: Option<T>,
    pub current: Option<T>,
    blend: Option<Blend<T>>,
}

impl<T> Default for LodState<T> {
    fn default() -> Self {
        Self {
            desired: None,
            current: None,
            blend: None,
        }
    }
}

impl<T: Copy + Eq> LodState<T> {
    pub fn update(&mut self, target: T, step: f32) {
        self.desired = Some(target);
        let Some(current) = self.current else {
            self.current = Some(target);
            return;
        };
        if let Some(blend) = &mut self.blend {
            // Reverse a transition without losing its current coverage. A third target
            // waits until one slot is free.
            blend.progress += if target == current { -step } else { step };
            if blend.progress <= 0.0 {
                self.blend = None;
            } else if blend.progress >= 1.0 {
                self.current = Some(blend.target);
                self.blend = None;
            }
        } else if current != target {
            self.blend = Some(Blend {
                target,
                progress: step.min(1.0),
            });
        }
    }
    pub fn slots(&self, upper: u8) -> Option<[(T, u8, u8); 2]> {
        let active = self.current?;
        let split = self.blend.map_or(upper, |blend| {
            (f32::from(upper) * (1.0 - blend.progress)).round() as u8
        });
        let incoming = self.blend.map_or(active, |blend| blend.target);
        Some([(active, 0, split), (incoming, split, upper)])
    }

    pub fn is_blending(&self) -> bool {
        self.blend.is_some()
    }
}

pub(super) fn quantize(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * STEPS + 0.5).floor() as u8
}

pub(super) fn coverage(value: u8) -> f32 {
    f32::from(value) / STEPS
}

pub(super) fn smoothstep(start: f32, end: f32, value: f32) -> f32 {
    let v = ((value - start) / (end - start)).clamp(0.0, 1.0);
    v * v * (3.0 - 2.0 * v)
}

pub(super) fn fade_step(time: &Time, settings: &VegetationSettings) -> f32 {
    time.delta_secs().min(0.075) / settings.transition_seconds.clamp(0.10, 2.0)
}

#[derive(Asset, AsBindGroup, TypePath, Clone)]
pub(super) struct BarkFade {
    #[uniform(100)]
    coverage: Vec4,
}

impl MaterialExtension for BarkFade {
    fn fragment_shader() -> ShaderRef {
        "shaders/vegetation_fade.wesl".into()
    }
}

pub(super) type BarkMaterial = ExtendedMaterial<StandardMaterial, BarkFade>;

/// A shared PBR material variant for an opaque or alpha-clipped dither interval.
pub(super) fn dither_material(base: StandardMaterial, low: u8, high: u8) -> BarkMaterial {
    BarkMaterial {
        base,
        extension: BarkFade {
            coverage: Vec4::new(coverage(low), coverage(high), 0.0, 0.0),
        },
    }
}

type Palette<T, U> = HashMap<(AssetId<T>, u8, u8), Handle<U>>;

#[derive(Resource, Default)]
pub(super) struct FadeMaterials {
    leaves: Palette<FoliageMaterial, FoliageMaterial>,
    bark: Palette<StandardMaterial, BarkMaterial>,
}

#[derive(Component)]
pub(super) struct DetailTree {
    patch: PatchKey,
    site: Site,
    lod: LodState<ModelChoice>,
    slots: [(Entity, Entity); 2],
    presented: [Option<(ModelChoice, u8, u8)>; 2],
}

#[derive(Component)]
pub(super) struct Trunk;

#[derive(Component)]
pub(super) struct Leaves;

pub(super) fn spawn_tree(
    commands: &mut Commands,
    root: &PlanetRoot,
    frame: &ReferenceFrame,
    patch: PatchKey,
    site: Site,
) -> Entity {
    let (cell, translation) = frame.translation_to_grid(site.position);
    let mut tree = root.spawn(
        commands,
        (
            Transform::from_translation(translation)
                .with_rotation(site.rotation())
                .with_scale(Vec3::splat(site.scale)),
            Visibility::Inherited,
        ),
    );
    tree.insert(cell);
    if site.species == Species::Palm && site.variant == 0 {
        tree.insert(CoconutPalm);
    }
    let name =
        super::library::asset(site.species, site.variant).map_or("Forest tree", |asset| asset.path);
    tree.insert(Name::new(name));
    let mut slots = [(Entity::PLACEHOLDER, Entity::PLACEHOLDER); 2];
    tree.with_children(|children| {
        for slot in &mut slots {
            let trunk = children
                .spawn((
                    Trunk,
                    NotShadowCaster,
                    NotShadowReceiver,
                    Aabb::default(),
                    Mesh3d::default(),
                    MeshMaterial3d::<BarkMaterial>::default(),
                    Visibility::Hidden,
                ))
                .id();
            let leaves = children
                .spawn((
                    Leaves,
                    NotShadowCaster,
                    NotShadowReceiver,
                    Aabb::default(),
                    Mesh3d::default(),
                    MeshMaterial3d::<FoliageMaterial>::default(),
                    Visibility::Hidden,
                ))
                .id();
            *slot = (trunk, leaves);
        }
    });
    tree.insert(DetailTree {
        patch,
        site,
        lod: LodState::default(),
        slots,
        presented: [None; 2],
    });
    tree.id()
}

type RenderParts<'a, M> = (
    &'a mut Mesh3d,
    &'a mut MeshMaterial3d<M>,
    &'a mut Aabb,
    &'a mut Visibility,
);

pub(super) fn update_lods(
    models: Res<ForestModels>,
    forest: Res<ForestStream>,
    settings: Res<VegetationSettings>,
    time: Res<Time>,
    cameras: Query<(Entity, GridTransform, &Camera, &Projection), With<WorldCamera>>,
    frames: ReferenceFrames,
    mut palette: ResMut<FadeMaterials>,
    standard: Res<Assets<StandardMaterial>>,
    mut leaf_materials: ResMut<Assets<FoliageMaterial>>,
    mut bark_materials: ResMut<Assets<BarkMaterial>>,
    mut stats: ResMut<super::debug::LodDiagnostics>,
    mut trees: Query<&mut DetailTree>,
    mut trunks: Query<RenderParts<BarkMaterial>, With<Trunk>>,
    mut leaves: Query<RenderParts<FoliageMaterial>, (With<Leaves>, Without<Trunk>)>,
) {
    let Ok((entity, grid, camera, projection)) = cameras.single() else {
        return;
    };
    let Some(frame) = frames.parent_frame(entity) else {
        return;
    };
    let eye = frame.grid_position_double(&grid.cell, &grid.transform);
    let view = View::new(camera, projection);
    let step = fade_step(&time, &settings);
    let band = view.impostor_band(&settings);
    stats.levels = [0; 3];
    stats.blends = 0;
    let models_changed = models.is_changed();
    for mut tree in &mut trees {
        let distance = eye.distance(tree.site.position) as f32;
        let (impostor, readiness) = forest.coverage(tree.patch);
        let range = settings.impostor_range.clamp(200.0, 600.0);
        let impostor = coverage(quantize(
            impostor * (1.0 - smoothstep(range * 0.78, range, distance)),
        ));
        let upper = quantize(impostor * readiness * (1.0 - smoothstep(band.x, band.y, distance)));
        if upper == 0 {
            if tree.lod.current.is_some() {
                tree.lod = LodState::default();
                for (index, slot) in tree.slots.into_iter().enumerate() {
                    if tree.presented[index].take().is_some() {
                        hide(slot, &mut trunks, &mut leaves);
                    }
                }
            }
            continue;
        }
        let level = pick_lod(
            &settings,
            view.pixels(tree.site, distance),
            tree.lod.desired.map(|lod| lod.level),
        );
        let hero = tree.site.species == Species::Oak
            && tree.site.variant == 0
            && models.hero_oak.is_some();
        tree.lod.update(ModelChoice { level, hero }, step);
        let active = tree.lod.current.unwrap();
        stats.levels[active.level] += 1;
        stats.blends += usize::from(tree.lod.is_blending());
        let slots = tree
            .lod
            .slots(upper)
            .expect("an active tree is assigned a model");
        for (index, ((trunk, canopy), (choice, low, high))) in
            tree.slots.into_iter().zip(slots).enumerate()
        {
            let desired = (low != high).then_some((choice, low, high));
            // Refresh visible handles when a shared model has finished loading.
            let refresh_model = models_changed && desired.is_some();
            if tree.presented[index] == desired && !refresh_model {
                continue;
            }
            tree.presented[index] = desired;
            if low == high {
                hide((trunk, canopy), &mut trunks, &mut leaves);
                continue;
            }
            let (model, bark) = models.model(
                tree.site.species,
                tree.site.variant,
                choice.level,
                choice.hero,
            );
            let dither = Vec4::new(coverage(low), coverage(high), 0.0, 0.0);
            if let Ok((mut mesh, mut material, mut bounds, mut visible)) = trunks.get_mut(trunk) {
                if let Some(wood) = &model.wood {
                    let handle = palette
                        .bark
                        .entry((bark.id(), low, high))
                        .or_insert_with(|| {
                            bark_materials.add(dither_material(
                                standard
                                    .get(bark)
                                    .expect("forest owns its bark materials")
                                    .clone(),
                                low,
                                high,
                            ))
                        });
                    mesh.set_if_neq(Mesh3d(wood.mesh.clone()));
                    material.set_if_neq(MeshMaterial3d(handle.clone()));
                    bounds.set_if_neq(wood.bounds);
                    visible.set_if_neq(Visibility::Inherited);
                } else {
                    visible.set_if_neq(Visibility::Hidden);
                }
            }
            if let Ok((mut mesh, mut material, mut bounds, mut visible)) = leaves.get_mut(canopy) {
                let handle = if low == 0 && high == STEPS as u8 {
                    model.foliage_material.clone()
                } else {
                    palette
                        .leaves
                        .entry((model.foliage_material.id(), low, high))
                        .or_insert_with(|| {
                            let mut fade = leaf_materials
                                .get(&model.foliage_material)
                                .expect("forest owns its canopy materials")
                                .clone();
                            fade.extension.coverage = dither;
                            leaf_materials.add(fade)
                        })
                        .clone()
                };
                mesh.set_if_neq(Mesh3d(model.foliage.clone()));
                material.set_if_neq(MeshMaterial3d(handle));
                bounds.set_if_neq(model.foliage_bounds);
                visible.set_if_neq(Visibility::Inherited);
            }
        }
    }
}

fn hide(
    (trunk, canopy): (Entity, Entity),
    trunks: &mut Query<RenderParts<BarkMaterial>, With<Trunk>>,
    leaves: &mut Query<RenderParts<FoliageMaterial>, (With<Leaves>, Without<Trunk>)>,
) {
    if let Ok((_, _, _, mut visible)) = trunks.get_mut(trunk) {
        visible.set_if_neq(Visibility::Hidden);
    }
    if let Ok((_, _, _, mut visible)) = leaves.get_mut(canopy) {
        visible.set_if_neq(Visibility::Hidden);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_projected_size_changes_keep_the_resident_lod() {
        let settings = VegetationSettings::default();
        let boundary = REFERENCE_HEIGHT * REFERENCE_FOCAL / settings.mesh_distances[0];
        assert_eq!(pick_lod(&settings, boundary * 0.95, Some(0)), 0);
        assert_eq!(pick_lod(&settings, boundary * 0.88, Some(0)), 1);
        assert_eq!(pick_lod(&settings, boundary * 1.05, Some(1)), 1);
        assert_eq!(pick_lod(&settings, boundary * 1.12, Some(1)), 0);
    }

    #[test]
    fn dither_fade_reverses_without_replacing_either_mesh() {
        let from = ModelChoice {
            level: 0,
            hero: true,
        };
        let to = ModelChoice {
            level: 1,
            hero: true,
        };
        let mut state = LodState::default();
        state.update(from, 0.25);
        state.update(to, 0.25);
        state.update(to, 0.25);
        state.update(from, 0.25);
        let blend = state.blend.unwrap();
        assert_eq!(blend.target, to);
        assert_eq!(blend.progress, 0.25);
        state.update(from, 0.25);
        assert!(state.blend.is_none());
        assert_eq!(state.current, Some(from));
    }

    #[test]
    fn representation_masks_partition_every_dither_bucket() {
        for p in 0..=STEPS as u8 {
            for ready in 0..=STEPS as u8 {
                let cutoff = quantize(coverage(p) * coverage(ready) * 0.7);
                let split = (f32::from(cutoff) * 0.37).round() as u8;
                for pixel in 0..STEPS as u8 {
                    assert_eq!(
                        usize::from(pixel < split)
                            + usize::from(pixel >= split && pixel < cutoff)
                            + usize::from(pixel >= cutoff && pixel < p)
                            + usize::from(pixel >= p),
                        1
                    );
                }
            }
        }
    }
}
