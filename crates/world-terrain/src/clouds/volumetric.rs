//! Reduced-resolution cloud march, temporal resolve, and HDR composition.

use bevy::{
    camera::Exposure,
    core_pipeline::{schedule::Core3d, Core3dSystems, FullscreenShader},
    diagnostic::FrameCount,
    math::DVec3,
    pbr::resources::AtmosphereTextures,
    prelude::*,
    render::{
        diagnostic::RecordDiagnostics,
        render_asset::RenderAssets,
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        sync_world::RenderEntity,
        texture::GpuImage,
        view::{ExtractedView, ViewDepthStencilTexture, ViewTarget},
        Extract, ExtractSchedule, RenderApp, RenderStartup,
    },
};
use world_core::{
    planet::{RADIUS, WATER_SURFACE_ELEVATION},
    runtime::{GridCell, PlanetRoot, ReferenceFrame, WorldCamera},
};

use super::{CloudFields, CloudMode, CloudVisibility, BASE, TOP};

const LOWER_TOP: f32 = 4_650.0;
const UPPER_BASE: f32 = 4_950.0;
const WIND: Vec3 = Vec3::new(4.8, -1.1, -3.2);
const RADIANCE: TextureFormat = TextureFormat::Rgba16Float;
const DISTANCES: TextureFormat = TextureFormat::Rg32Float;

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, enable_depth_sampling);
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app
            .add_systems(ExtractSchedule, extract_camera)
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(
                Core3d,
                render_clouds
                    .after(Core3dSystems::MainPass)
                    .before(Core3dSystems::PostProcess),
            );
    }
}

fn enable_depth_sampling(mut views: Query<&mut Camera3d, (With<WorldCamera>, Added<Camera3d>)>) {
    for mut camera in &mut views {
        let usage = TextureUsages::from(camera.depth_texture_usages);
        camera.depth_texture_usages = (usage | TextureUsages::TEXTURE_BINDING).into();
    }
}

#[derive(Component)]
struct CloudCamera {
    eye: DVec3,
    rotation: Quat,
    projection: Mat4,
    sun: Vec3,
    sunlight: Vec3,
    elapsed: f64,
    weather: Handle<Image>,
    shape: Handle<Image>,
}

fn extract_camera(
    mut commands: Commands,
    root: Extract<Option<Res<PlanetRoot>>>,
    frames: Extract<Query<&ReferenceFrame>>,
    cameras: Extract<
        Query<(RenderEntity, &GridCell, &Transform, &Exposure, &Projection), With<WorldCamera>>,
    >,
    sun: Extract<Query<(&DirectionalLight, &Transform), With<bevy::light::SunDisk>>>,
    visibility: Extract<Query<&Visibility, With<CloudVisibility>>>,
    mode: Extract<Res<CloudMode>>,
    fields: Extract<Option<Res<CloudFields>>>,
    time: Extract<Res<Time>>,
) {
    for (entity, cell, transform, exposure, projection) in &*cameras {
        let active = **mode == CloudMode::Volumetric
            && visibility.single().is_ok_and(|v| *v != Visibility::Hidden);
        let frame = root
            .as_ref()
            .and_then(|root| frames.get(root.entity()).ok());
        let (true, Some(frame), Some(fields), Ok((light, sun)), Projection::Perspective(_)) =
            (active, frame, fields.as_ref(), sun.single(), projection)
        else {
            commands.entity(entity).remove::<CloudCamera>();
            continue;
        };

        commands.entity(entity).insert(CloudCamera {
            eye: frame.grid_position_double(cell, transform),
            rotation: transform.rotation,
            projection: projection.get_clip_from_view(),
            sun: sun.back().as_vec3(),
            sunlight: light.color.to_linear().to_vec3() * light.illuminance * exposure.exposure(),
            elapsed: time.elapsed_secs_f64(),
            weather: fields.weather.clone(),
            shape: fields.shape.clone(),
        });
    }
}

// The current camera origin stays at zero for projection and reprojection.
// The CPU subtracts the two planet positions in f64 before uploading the delta.
#[derive(Clone, Copy, Default, ShaderType)]
struct CloudUniform {
    planet_from_view: Mat4,
    view_from_clip: Mat4,
    previous_clip_from_current: Mat4,
    eye: Vec4,
    sun: Vec4,
    sunlight: Vec4,
    wind_offset: Vec4,
    radii: Vec4,
    layers: Vec4,
    viewport: UVec4,
    frame: UVec4,
}

struct HistoryFrame {
    radiance: TextureView,
    distances: TextureView,
}

struct History {
    entity: Entity,
    viewport: UVec4,
    target: TextureId,
    raw: TextureView,
    frames: [HistoryFrame; 2],
    write: usize,
    previous: Option<Pose>,
    uniform: UniformBuffer<CloudUniform>,
}

struct Pose {
    eye: DVec3,
    rotation: Quat,
    projection: Mat4,
    elapsed: f64,
    sun: Vec3,
    sunlight: Vec3,
    frame: u32,
}

impl History {
    fn new(device: &RenderDevice, entity: Entity, viewport: UVec4, target: TextureId) -> Self {
        let make_texture = |label, format| {
            device
                .create_texture(&TextureDescriptor {
                    label: Some(label),
                    size: Extent3d {
                        width: viewport.z.div_ceil(2),
                        height: viewport.w.div_ceil(2),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&TextureViewDescriptor::default())
        };
        Self {
            entity,
            viewport,
            target,
            raw: make_texture("cloud march radiance", RADIANCE),
            frames: std::array::from_fn(|_| HistoryFrame {
                radiance: make_texture("cloud history radiance", RADIANCE),
                distances: make_texture("cloud history distances", DISTANCES),
            }),
            write: 0,
            previous: None,
            uniform: UniformBuffer::default(),
        }
    }

    fn update(
        &mut self,
        camera: &CloudCamera,
        frame: u32,
        device: &RenderDevice,
        queue: &RenderQueue,
    ) {
        let previous = self.previous.as_ref().filter(|last| {
            let dt = camera.elapsed - last.elapsed;
            let height = (camera.eye.length() - RADIUS).max(0.0);
            let cut_distance = (height * 0.04).clamp(1_600.0, 60_000.0);
            last.frame.wrapping_add(1) == frame
                && (0.0..=0.25).contains(&dt)
                && last.rotation.dot(camera.rotation).abs() > 0.88
                && last.eye.distance(camera.eye) < cut_distance
                && last.sun.dot(camera.sun) > 0.999
                && last.sunlight.distance(camera.sunlight)
                    < 0.05 * camera.sunlight.length().max(0.1)
                && (last.projection.x_axis.x - camera.projection.x_axis.x).abs() < 0.1
                && (last.projection.y_axis.y - camera.projection.y_axis.y).abs() < 0.1
        });
        let previous_projection = previous.map_or(Mat4::IDENTITY, |last| {
            let wind_motion = WIND.as_dvec3() * (camera.elapsed - last.elapsed);
            let delta = (camera.eye - last.eye - wind_motion).as_vec3();
            let rotation = last.rotation.conjugate();
            last.projection * Mat4::from_rotation_translation(rotation, rotation * delta)
        });
        self.uniform.set(CloudUniform {
            planet_from_view: Mat4::from_quat(camera.rotation),
            view_from_clip: camera.projection.inverse(),
            previous_clip_from_current: previous_projection,
            eye: camera.eye.as_vec3().extend(camera.eye.length() as f32),
            sun: camera.sun.extend(0.0),
            sunlight: camera.sunlight.extend(0.0),
            wind_offset: (WIND.as_dvec3() * camera.elapsed).as_vec3().extend(0.0),
            radii: Vec4::new(
                RADIUS as f32,
                (RADIUS + WATER_SURFACE_ELEVATION) as f32,
                RADIUS as f32 + 60_000.0,
                0.0,
            ),
            layers: Vec4::new(BASE, LOWER_TOP, UPPER_BASE, TOP) + Vec4::splat(RADIUS as f32),
            viewport: self.viewport,
            frame: UVec4::new(frame, u32::from(previous.is_some()), 0, 0),
        });
        self.uniform.write_buffer(device, queue);
        self.previous = Some(Pose {
            eye: camera.eye,
            rotation: camera.rotation,
            projection: camera.projection,
            elapsed: camera.elapsed,
            sun: camera.sun,
            sunlight: camera.sunlight,
            frame,
        });
    }
}

#[derive(Resource)]
struct CloudPipelines {
    common: [BindGroupLayoutDescriptor; 2],
    resolve_uniform: BindGroupLayoutDescriptor,
    march: BindGroupLayoutDescriptor,
    temporal: BindGroupLayoutDescriptor,
    upscale: BindGroupLayoutDescriptor,
    march_ids: [CachedRenderPipelineId; 2],
    temporal_id: CachedRenderPipelineId,
    upscale_ids: [CachedRenderPipelineId; 2],
    atmosphere_sampler: Sampler,
}

fn init_pipelines(
    mut commands: Commands,
    assets: Res<AssetServer>,
    cache: Res<PipelineCache>,
    device: Res<RenderDevice>,
    fullscreen: Res<FullscreenShader>,
) {
    let layout =
        |label, entries: &[BindGroupLayoutEntry]| BindGroupLayoutDescriptor::new(label, entries);
    let stages = ShaderStages::FRAGMENT;
    let common = [
        layout(
            "cloud uniform and depth",
            &BindGroupLayoutEntries::sequential(
                stages,
                (uniform_buffer::<CloudUniform>(false), texture_depth_2d()),
            ),
        ),
        layout(
            "cloud uniform and multisample depth",
            &BindGroupLayoutEntries::sequential(
                stages,
                (
                    uniform_buffer::<CloudUniform>(false),
                    texture_depth_2d_multisampled(),
                ),
            ),
        ),
    ];
    let sample = TextureSampleType::Float { filterable: true };
    let filtering = sampler(SamplerBindingType::Filtering);
    let image = || texture_2d(TextureSampleType::Float { filterable: false });
    let march = layout(
        "cloud density and atmosphere",
        &BindGroupLayoutEntries::sequential(
            stages,
            (
                texture_3d(sample),
                filtering,
                texture_cube(sample),
                filtering,
                texture_2d(sample),
                filtering,
            ),
        ),
    );
    let temporal = layout(
        "cloud temporal inputs",
        &BindGroupLayoutEntries::sequential(stages, (image(), image(), image(), image())),
    );
    let upscale = layout(
        "cloud upscale inputs",
        &BindGroupLayoutEntries::sequential(stages, (image(), image())),
    );
    let resolve_uniform = layout(
        "cloud resolve uniform",
        &BindGroupLayoutEntries::sequential(stages, (uniform_buffer::<CloudUniform>(false),)),
    );
    let pipeline = |entry: &'static str,
                    path,
                    variant,
                    primary: &BindGroupLayoutDescriptor,
                    bindings: &BindGroupLayoutDescriptor,
                    formats: &[TextureFormat],
                    blend| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(entry.into()),
            layout: vec![primary.clone(), bindings.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: assets.load(path),
                entry_point: Some(entry.into()),
                constants: vec![],
                shader_defs: if variant == 1 {
                    vec!["CLOUD_MSAA".into()]
                } else {
                    vec![]
                },
                targets: formats
                    .iter()
                    .map(|&format| {
                        Some(ColorTargetState {
                            format,
                            blend,
                            write_mask: ColorWrites::ALL,
                        })
                    })
                    .collect(),
            }),
            ..default()
        })
    };
    let march_ids = [0, 1].map(|msaa| {
        pipeline(
            "march_clouds",
            "shaders/volumetric_clouds.wesl",
            msaa,
            &common[msaa],
            &march,
            &[RADIANCE, DISTANCES],
            None,
        )
    });
    let temporal_id = pipeline(
        "resolve_clouds",
        "shaders/cloud_temporal.wesl",
        0,
        &resolve_uniform,
        &temporal,
        &[RADIANCE],
        None,
    );
    let upscale_ids = [0, 1].map(|msaa| {
        pipeline(
            "upscale_clouds",
            "shaders/cloud_composite.wesl",
            msaa,
            &common[msaa],
            &upscale,
            &[RADIANCE],
            Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        )
    });
    commands.insert_resource(CloudPipelines {
        common,
        resolve_uniform,
        march,
        temporal,
        upscale,
        march_ids,
        temporal_id,
        upscale_ids,
        atmosphere_sampler: device.create_sampler(&SamplerDescriptor {
            min_filter: FilterMode::Linear,
            mag_filter: FilterMode::Linear,
            ..default()
        }),
    });
}

fn render_clouds(
    view: ViewQuery<(
        &CloudCamera,
        &ExtractedView,
        &ViewTarget,
        &ViewDepthStencilTexture,
        &AtmosphereTextures,
        &Msaa,
    )>,
    mut history: Local<Option<History>>,
    mut ctx: RenderContext,
    gpu: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    frame: Res<FrameCount>,
    cache: Res<PipelineCache>,
    pipelines: Res<CloudPipelines>,
    images: Res<RenderAssets<GpuImage>>,
) {
    let entity = view.entity();
    let (camera, view, target, depths, atmosphere, msaa) = view.into_inner();
    let variant = usize::from(msaa.samples() > 1);
    let (Some(march), Some(resolve), Some(upscale), Some(shape), Some(weather)) = (
        cache.get_render_pipeline(pipelines.march_ids[variant]),
        cache.get_render_pipeline(pipelines.temporal_id),
        cache.get_render_pipeline(pipelines.upscale_ids[variant]),
        images.get(&camera.shape),
        images.get(&camera.weather),
    ) else {
        return;
    };
    let viewport = view.viewport;
    if viewport.z == 0 || viewport.w == 0 {
        return;
    }
    let target_id = target
        .main_texture()
        .id()
        .min(target.main_texture_other().id());
    if history.as_ref().is_none_or(|history| {
        (history.entity, history.viewport, history.target) != (entity, viewport, target_id)
    }) {
        *history = Some(History::new(&gpu, entity, viewport, target_id));
    }
    let history = history.as_mut().unwrap();
    history.update(camera, frame.0, &gpu, &queue);

    let make_group = |layout: &BindGroupLayoutDescriptor, entries: &[BindGroupEntry]| {
        gpu.create_bind_group(
            layout.label.as_ref(),
            &cache.get_bind_group_layout(layout),
            entries,
        )
    };
    let depth = &depths.attachment.texture.default_view;
    let uniform = history.uniform.binding().unwrap();
    let common = make_group(
        &pipelines.common[variant],
        &BindGroupEntries::sequential((uniform.clone(), depth)),
    );
    let resolve_layout = make_group(
        &pipelines.resolve_uniform,
        &BindGroupEntries::sequential((uniform,)),
    );
    let density = make_group(
        &pipelines.march,
        &BindGroupEntries::sequential((
            &shape.texture_view,
            &shape.sampler,
            &weather.texture_view,
            &weather.sampler,
            &atmosphere.transmittance_lut.default_view,
            &pipelines.atmosphere_sampler,
        )),
    );
    let current = &history.frames[history.write];
    let previous = &history.frames[1 - history.write];
    let temporal = make_group(
        &pipelines.temporal,
        &BindGroupEntries::sequential((
            &history.raw,
            &current.distances,
            &previous.radiance,
            &previous.distances,
        )),
    );
    let upsample = make_group(
        &pipelines.upscale,
        &BindGroupEntries::sequential((&current.radiance, &current.distances)),
    );

    draw(
        &mut ctx,
        "cloud_ray_march",
        march,
        &common,
        &density,
        &[
            attachment(&history.raw, true),
            attachment(&current.distances, true),
        ],
        None,
    );
    draw(
        &mut ctx,
        "cloud_temporal_resolve",
        resolve,
        &resolve_layout,
        &temporal,
        &[attachment(&current.radiance, true)],
        None,
    );
    draw(
        &mut ctx,
        "cloud_depth_aware_composite",
        upscale,
        &common,
        &upsample,
        &[attachment(target.main_texture_view(), false)],
        Some(viewport),
    );
    history.write = 1 - history.write;
}

fn attachment(view: &TextureView, clear: bool) -> Option<RenderPassColorAttachment<'_>> {
    Some(RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: Operations {
            load: if clear {
                LoadOp::Clear(default())
            } else {
                LoadOp::Load
            },
            store: StoreOp::Store,
        },
    })
}

fn draw(
    ctx: &mut RenderContext,
    label: &'static str,
    pipeline: &RenderPipeline,
    common: &BindGroup,
    textures: &BindGroup,
    attachments: &[Option<RenderPassColorAttachment>],
    viewport: Option<UVec4>,
) {
    let diagnostics = ctx.diagnostic_recorder();
    let mut pass = ctx
        .command_encoder()
        .begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
            color_attachments: attachments,
            ..default()
        });
    let diagnostics = diagnostics.as_deref();
    let span = diagnostics.pass_span(&mut pass, label);
    if let Some(rect) = viewport {
        pass.set_viewport(
            rect.x as f32,
            rect.y as f32,
            rect.z as f32,
            rect.w as f32,
            0.0,
            1.0,
        );
    }
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, common, &[]);
    pass.set_bind_group(1, textures, &[]);
    pass.draw(0..3, 0..1);
    span.end(&mut pass);
}
