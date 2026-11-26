use bevy::{
    math::DVec3,
    prelude::*,
    render::{
        render_resource::{binding_types::*, *},
        renderer::{RenderDevice, RenderQueue},
    },
};

#[derive(Default, ShaderType)]
struct HistoryUniform {
    origin: Vec4,
    forward: Vec4,
    right: Vec4,
    up: Vec4,
    screen: Vec4,
    valid: UVec4,
}

pub(super) struct Pipeline {
    layout: BindGroupLayoutDescriptor,
    pub(super) id: CachedComputePipelineId,
}

impl Pipeline {
    pub(super) fn new(cache: &PipelineCache, shader: Handle<Shader>) -> Self {
        let layout = BindGroupLayoutDescriptor::new(
            "Igor history",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer::<super::FrameUniform>(false),
                    uniform_buffer::<HistoryUniform>(false),
                    storage_buffer_read_only_sized(false, None),
                    texture_2d(TextureSampleType::Float { filterable: false }),
                    texture_storage_2d(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly),
                ),
            ),
        );
        let id = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("Igor history reprojection".into()),
            layout: vec![layout.clone()],
            shader,
            entry_point: Some("temporal_cs".into()),
            ..default()
        });
        Self { layout, id }
    }
}

#[derive(Clone, Copy)]
pub(super) struct View {
    pub(super) entity: Entity,
    pub(super) eye: DVec3,
    pub(super) forward: Vec3,
    pub(super) right: Vec3,
    pub(super) up: Vec3,
    pub(super) screen: Vec4,
    pub(super) time: f32,
}

pub(super) struct History {
    size: UVec2,
    views: [TextureView; 2],
    last: Option<View>,
    read: usize,
    uniform: UniformBuffer<HistoryUniform>,
}

impl History {
    fn new(device: &RenderDevice, size: UVec2) -> Self {
        let views = std::array::from_fn(|_| {
            device
                .create_texture(&TextureDescriptor {
                    label: Some("Igor HDR point history"),
                    size: Extent3d {
                        width: size.x,
                        height: size.y,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: TextureFormat::Rgba16Float,
                    usage: TextureUsages::TEXTURE_BINDING | TextureUsages::STORAGE_BINDING,
                    view_formats: &[],
                })
                .create_view(&TextureViewDescriptor::default())
        });
        Self {
            size,
            views,
            last: None,
            read: 0,
            uniform: UniformBuffer::default(),
        }
    }

    pub(super) fn for_size<'a>(
        slot: &'a mut Option<Self>,
        device: &RenderDevice,
        size: UVec2,
    ) -> &'a mut Self {
        if slot.as_ref().is_none_or(|history| history.size != size) {
            *slot = Some(Self::new(device, size));
        }
        slot.as_mut().unwrap()
    }

    pub(super) fn encode(
        &mut self,
        encoder: &mut CommandEncoder,
        device: &RenderDevice,
        queue: &RenderQueue,
        cache: &PipelineCache,
        pipeline: &Pipeline,
        camera: BindingResource<'_>,
        screen: &Buffer,
        view: View,
    ) -> &TextureView {
        let previous = self.last.filter(|old| {
            old.entity == view.entity
                && old.screen == view.screen
                && view.time >= old.time
                && view.time - old.time <= 0.15
                && view.eye.distance_squared(old.eye) <= 64.0
                && old.forward.dot(view.forward) > 0.9
        });
        self.uniform.set(
            previous.map_or_else(HistoryUniform::default, |old| HistoryUniform {
                origin: (view.eye - old.eye).as_vec3().extend(0.0),
                forward: old.forward.extend(0.0),
                right: old.right.extend(0.0),
                up: old.up.extend(0.0),
                screen: old.screen,
                valid: UVec4::X,
            }),
        );
        self.uniform.write_buffer(device, queue);
        let write = 1 - self.read;
        let bind = device.create_bind_group(
            "Igor history",
            &cache.get_bind_group_layout(&pipeline.layout),
            &BindGroupEntries::sequential((
                camera,
                self.uniform.binding().unwrap(),
                screen.as_entire_binding(),
                &self.views[self.read],
                &self.views[write],
            )),
        );
        let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
            label: Some("Igor history reprojection"),
            ..default()
        });
        pass.set_pipeline(cache.get_compute_pipeline(pipeline.id).unwrap());
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(self.size.x.div_ceil(8), self.size.y.div_ceil(8), 1);
        self.last = Some(view);
        self.read = write;
        &self.views[write]
    }
}
