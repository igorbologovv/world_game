//! Bridges canonical world placements and Bevy's 3D pass to Igor_renderer.
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use bevy::{
    camera::{Exposure, MainPassResolutionOverride, Viewport},
    core_pipeline::{core_3d::main_opaque_pass_3d, schedule::Core3d, Core3dSystems},
    math::{DQuat, DVec3},
    prelude::*,
    render::{
        camera::{ExtractedCamera, TemporalJitter},
        diagnostic::RecordDiagnostics,
        render_resource::{RenderPassDescriptor, StoreOp},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        sync_world::RenderEntity,
        view::{ExtractedView, ViewDepthStencilTexture, ViewTarget},
        Extract, ExtractSchedule, RenderApp,
    },
    tasks::{block_on, poll_once, AsyncComputeTaskPool, Task},
};
use igor_renderer::{
    Effects, Model, Models, PipelineKey, Region, RegionDraw, Renderer, Stats, Tree,
};
use world_core::runtime::{GridCell, ReferenceFrames, WorldAssets, WorldCamera};

use super::{
    generate_tree, library,
    placement::{PatchKey, Site},
    streaming::ForestStream,
    textures, MeshBuilder, Species, VegetationSettings, FOLIAGE_CUTOFF,
};

pub(super) fn install(app: &mut App) {
    app.init_resource::<ForestRenderStats>();
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .init_resource::<ForestFrame>()
            .add_systems(ExtractSchedule, extract)
            .add_systems(
                Core3d,
                draw_forest
                    .in_set(Core3dSystems::MainPass)
                    // Bevy composites the atmosphere from the shared depth after opaques.
                    .before(main_opaque_pass_3d),
            );
        app.init_resource::<ModelLoading>()
            .add_systems(Startup, request_models)
            .add_systems(Update, finish_models.before(super::streaming::stream));
    }
}

#[derive(Resource, Default)]
pub(super) struct ModelLoading {
    task: Option<Task<ForestModels>>,
    pub models: Option<Arc<ForestModels>>,
}

pub(super) struct ForestModels {
    gpu: Models,
    families: [Vec<Model>; Species::ALL.len()],
    effects: Effects,
}

fn request_models(mut loading: ResMut<ModelLoading>, assets: Res<WorldAssets>) {
    let root = assets.directory().join("models");
    loading.task = Some(AsyncComputeTaskPool::get().spawn(async move { load_models(&root) }));
}

fn finish_models(mut loading: ResMut<ModelLoading>) {
    if let Some(models) = loading
        .task
        .as_mut()
        .and_then(|task| block_on(poll_once(task)))
    {
        loading.models = Some(Arc::new(models));
        loading.task = None;
    }
}

fn tree_asset(root: &Path, species: Species, variant: usize) -> Option<PathBuf> {
    let path = match (species, variant) {
        (Species::Oak, 0) => return Some(root.join("hero_oak/igor/oak_lod0.glb")),
        (Species::Oak, 1) => "woodland/woodland_tree_00",
        (Species::Oak, 2) => "woodland/woodland_tree_01",
        (Species::Jungle, 1) => "tree_part9/tree",
        (Species::Palm, 0) => "coconut_palm/tree",
        _ => library::asset(species, variant)?.path,
    };
    Some(root.join(format!("vegetation/{path}_lod0.glb")))
}

fn load_models(root: &Path) -> ForestModels {
    let mut gpu = Models::new(Species::ALL.into_iter().map(Species::variants).sum());
    let bark = rgba(textures::bark_texture());
    let families = Species::ALL.map(|species| {
        let foliage = rgba(textures::canopy_texture(species));
        let (wood_color, leaf_color) = species.colors();
        let mut family = Vec::with_capacity(species.variants());
        for variant in 0..species.variants() {
            if let Some(path) = tree_asset(root, species, variant) {
                match gpu.load(&path) {
                    Ok(model) => {
                        family.push(model);
                        continue;
                    }
                    Err(err) => warn!("Igor forest asset {}: {err}", path.display()),
                }
            }
            let stock = species.stock_variants();
            if variant >= stock {
                family.push(family[variant % stock]);
                continue;
            }
            let (wood, leaves) = generate_tree(species, variant as u64);
            let model = gpu
                .add(vec![
                    part(wood, bark.clone(), wood_color, None),
                    part(leaves, foliage.clone(), leaf_color, Some(FOLIAGE_CUTOFF)),
                ])
                .expect("procedural tree geometry fits the Igor atlas");
            family.push(model);
        }
        family
    });
    info!("Igor forest model templates are ready");
    ForestModels {
        gpu,
        families,
        effects: Effects::default(),
    }
}

fn rgba(image: Image) -> image::RgbaImage {
    let (width, height) = (image.width(), image.height());
    let mut pixels = image.data.unwrap();
    pixels.truncate((width * height * 4) as usize);
    image::RgbaImage::from_raw(width, height, pixels).unwrap()
}

fn part(
    mesh: MeshBuilder,
    color: image::RgbaImage,
    tint: [f32; 3],
    alpha_cutoff: Option<f32>,
) -> igor_renderer::MeshPart {
    igor_renderer::MeshPart {
        positions: mesh.positions,
        normals: mesh.normals,
        uvs: mesh.uvs,
        indices: mesh.indices,
        color,
        factor: [tint[0], tint[1], tint[2], 1.0],
        alpha_cutoff,
    }
}

pub(super) struct RenderChunk {
    origin: DVec3,
    rotation: DQuat,
    region: Region,
}

impl RenderChunk {
    pub fn build(
        models: &ForestModels,
        anchor: DVec3,
        sites: &[Site],
        sun: Option<Vec3>,
    ) -> Option<Self> {
        let up = anchor.normalize();
        let rotation = DQuat::from_rotation_arc(DVec3::Y, up);
        let inverse = rotation.inverse();
        let base = sites
            .iter()
            .map(|site| (site.position - anchor).dot(up))
            .reduce(f64::min)?;
        let origin = anchor + up * base;
        let trees: Vec<_> = sites
            .iter()
            .map(|site| Tree {
                model: models.families[site.species as usize][site.variant],
                root: (inverse * (site.position - origin))
                    .as_vec3()
                    .to_array()
                    .into(),
                yaw: site.yaw,
                scale: site.scale,
            })
            .collect();
        let region = Region::new(&trees, &models.gpu, models.effects, local_sun(inverse, sun))?;
        Some(Self {
            origin,
            rotation,
            region,
        })
    }

    fn camera(
        &self,
        frame: &ForestFrame,
        view: &ExtractedView,
        clip: Mat4,
        size: UVec2,
        exposure_scale: f32,
    ) -> igor_renderer::Camera {
        let inverse = self.rotation.inverse();
        let origin = (inverse * (frame.eye - self.origin)).as_vec3();
        let rotation = inverse.as_quat() * view.world_from_view.rotation();
        let view_from_local = Mat4::from_rotation_translation(rotation, origin).inverse();
        igor_renderer::Camera {
            origin: origin.to_array().into(),
            forward: (rotation * Vec3::NEG_Z).to_array().into(),
            right: (rotation * Vec3::X).to_array().into(),
            up: (rotation * Vec3::Y).to_array().into(),
            width: size.x,
            height: size.y,
            clip_from_local: igor_renderer::glam::Mat4::from_cols_array_2d(
                &(clip * view_from_local).to_cols_array_2d(),
            ),
            tan_half_fov: [
                view.clip_from_view.x_axis.x.recip(),
                view.clip_from_view.y_axis.y.recip(),
            ],
            sun_direction: local_sun(inverse, frame.sun),
            forest_band: frame.band.to_array(),
            exposure_scale,
        }
    }
}

pub(super) fn sun_direction(lights: &Query<(&DirectionalLight, &GlobalTransform)>) -> Option<Vec3> {
    lights
        .iter()
        .filter(|(light, _)| light.illuminance > 0.0)
        .max_by(|(a, _), (b, _)| a.illuminance.total_cmp(&b.illuminance))
        .map(|(_, transform)| transform.back().as_vec3())
}

fn local_sun(inverse: DQuat, sun: Option<Vec3>) -> igor_renderer::glam::Vec3 {
    let direction = sun.map_or(Vec3::new(-0.5, 0.8, -0.4).normalize(), |direction| {
        inverse.as_quat() * direction
    });
    direction.to_array().into()
}

/// Tree counters for the most recently submitted forest pass.
#[derive(Resource, Clone, Default)]
pub struct ForestRenderStats(Arc<Mutex<Stats>>);

impl ForestRenderStats {
    /// Number of trees selected at each Igor LOD. Fading trees can count in multiple levels.
    pub fn levels(&self) -> [usize; 3] {
        self.snapshot().levels
    }

    /// Number of trees blending between full and middle LODs and cells blending to far LOD.
    pub fn transitions(&self) -> [usize; 2] {
        self.snapshot().transitions
    }

    /// Number of triangles submitted by the Igor forest pass.
    pub fn triangles(&self) -> u64 {
        self.snapshot().triangles
    }

    pub(super) fn snapshot(&self) -> Stats {
        *self.0.lock().unwrap()
    }
}

#[derive(Resource, Default)]
struct ForestFrame {
    camera: Option<Entity>,
    eye: DVec3,
    elapsed: f32,
    time: f32,
    band: Vec2,
    sun: Option<Vec3>,
    models: Option<Arc<ForestModels>>,
    chunks: BTreeMap<PatchKey, Arc<RenderChunk>>,
    stats: ForestRenderStats,
}

fn extract(
    mut frame: ResMut<ForestFrame>,
    cameras: Extract<Query<(Entity, RenderEntity, &GridCell, &Transform), With<WorldCamera>>>,
    frames: Extract<ReferenceFrames>,
    loaded: Extract<Res<ModelLoading>>,
    forest: Extract<Res<ForestStream>>,
    settings: Extract<Res<VegetationSettings>>,
    time: Extract<Res<Time>>,
    stats: Extract<Res<ForestRenderStats>>,
    lights: Extract<Query<(&DirectionalLight, &GlobalTransform)>>,
) {
    frame.camera = None;
    frame.models.clone_from(&loaded.models);
    frame.chunks.clear();
    let Ok((entity, render_entity, cell, transform)) = cameras.single() else {
        return;
    };
    let Some(grid) = frames.parent_frame(entity) else {
        return;
    };
    frame.camera = Some(render_entity);
    frame.eye = grid.grid_position_double(cell, transform);
    frame.elapsed = time.delta_secs();
    frame.time = time.elapsed_secs();
    frame.band = settings.forest_band();
    frame.sun = sun_direction(&lights);
    frame.chunks.extend(forest.chunks());
    frame.stats = stats.clone();
}

#[derive(Default)]
struct DrawState {
    renderer: Option<Renderer>,
    draws: HashMap<PatchKey, (Arc<RenderChunk>, RegionDraw)>,
}

fn draw_forest(
    view: ViewQuery<(
        &ExtractedCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &Msaa,
        Option<&TemporalJitter>,
        Option<&MainPassResolutionOverride>,
    )>,
    mut state: Local<DrawState>,
    mut ctx: RenderContext,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    frame: Res<ForestFrame>,
) {
    if Some(view.entity()) != frame.camera {
        return;
    }
    let (camera, view, color, depth, msaa, jitter, resolution) = view.into_inner();
    let viewport = Viewport::from_viewport_and_override(camera.viewport.as_ref(), resolution);
    let size = viewport
        .as_ref()
        .map_or(view.viewport.zw(), |v| v.physical_size);
    if size.min_element() == 0 {
        return;
    }

    let state = &mut *state;
    state.draws.retain(|key, (loaded, _)| {
        frame
            .chunks
            .get(key)
            .is_some_and(|chunk| Arc::ptr_eq(chunk, loaded))
    });
    let Some(models) = &frame.models else {
        return;
    };
    let gpu = device.wgpu_device();
    let mut clip = view.clip_from_view;
    if let Some(jitter) = jitter {
        jitter.jitter_projection(&mut clip, size.as_vec2());
    }
    // Igor's authored lighting is calibrated at the world camera's original EV100.
    let exposure_scale = camera.exposure / (Exposure { ev100: 13.0 }).exposure();
    let visible: Vec<_> = frame
        .chunks
        .iter()
        .filter_map(|(&key, chunk)| {
            let camera = chunk.camera(&frame, view, clip, size, exposure_scale);
            if chunk.region.visible(camera, frame.band.y) {
                Some((key, chunk, camera))
            } else {
                if let Some((_, draw)) = state.draws.get_mut(&key) {
                    draw.hide();
                }
                None
            }
        })
        .collect();
    if visible.is_empty() {
        *frame.stats.0.lock().unwrap() = Stats::default();
        return;
    }
    let renderer = state
        .renderer
        .get_or_insert_with(|| Renderer::new(gpu, &queue, &models.gpu));
    let key = PipelineKey {
        color: color.main_texture_format(),
        depth: depth.texture().format(),
        samples: msaa.samples(),
    };
    renderer.prepare_pipeline(gpu, key);

    let mut stats = Stats::default();
    for (key, chunk, camera) in &visible {
        let (_, draw) = state.draws.entry(*key).or_insert_with(|| {
            (
                Arc::clone(chunk),
                renderer.add_region(gpu, &queue, &chunk.region),
            )
        });
        let region = draw.prepare(
            &queue,
            &models.gpu,
            &chunk.region,
            *camera,
            msaa.samples(),
            frame.elapsed,
            frame.time,
        );
        for (total, count) in stats.levels.iter_mut().zip(region.levels) {
            *total += count;
        }
        for (total, count) in stats.transitions.iter_mut().zip(region.transitions) {
            *total += count;
        }
        stats.triangles += region.triangles;
    }
    *frame.stats.0.lock().unwrap() = stats;
    if stats.triangles == 0 {
        return;
    }

    let draws: Vec<_> = visible
        .iter()
        .map(|(key, _, _)| &state.draws[key].1)
        .collect();
    let recorder = ctx.diagnostic_recorder();
    let diagnostics = recorder.as_deref();
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some("igor_hybrid_forest"),
            color_attachments: &[Some(color.get_color_attachment())],
            depth_stencil_attachment: Some(depth.get_attachment(StoreOp::Store)),
            ..default()
        });
    if let Some(viewport) = viewport {
        let pos = viewport.physical_position.as_vec2();
        let size = viewport.physical_size.as_vec2();
        pass.set_viewport(
            pos.x,
            pos.y,
            size.x,
            size.y,
            viewport.depth.start,
            viewport.depth.end,
        );
    }
    let timer = diagnostics.pass_span(&mut pass, "igor_hybrid_forest");
    renderer.draw(&mut pass, key, &draws);
    timer.end(&mut pass);
}
