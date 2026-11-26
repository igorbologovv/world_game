use bevy::{
    prelude::*,
    render::{
        render_resource::{binding_types::*, *},
        renderer::RenderDevice,
    },
};

pub(super) struct Pipeline {
    pub(super) id: CachedComputePipelineId,
    layout: BindGroupLayoutDescriptor,
}

impl Pipeline {
    pub(super) fn new(cache: &PipelineCache, shader: Handle<Shader>, multisampled: bool) -> Self {
        let depth = BindingType::Texture {
            sample_type: TextureSampleType::Depth,
            view_dimension: TextureViewDimension::D2,
            multisampled,
        };
        let layout = BindGroupLayoutDescriptor::new(
            "Igor opaque depth",
            &BindGroupLayoutEntries::sequential(
                ShaderStages::COMPUTE,
                (
                    uniform_buffer::<super::FrameUniform>(false),
                    depth,
                    storage_buffer_sized(false, None),
                ),
            ),
        );
        let id = cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some("Igor seed opaque depth".into()),
            layout: vec![layout.clone()],
            shader,
            entry_point: Some("seed".into()),
            ..default()
        });
        Self { id, layout }
    }

    pub(super) fn bind(
        &self,
        device: &RenderDevice,
        cache: &PipelineCache,
        uniform: BindingResource<'_>,
        depth: &TextureView,
        screen: &Buffer,
    ) -> BindGroup {
        device.create_bind_group(
            "Igor opaque depth",
            &cache.get_bind_group_layout(&self.layout),
            &BindGroupEntries::sequential((uniform, depth, screen.as_entire_binding())),
        )
    }
}

pub(super) fn shader(multisampled: bool) -> String {
    let depth = if multisampled {
        r#"@group(0) @binding(1) var opaque: texture_depth_multisampled_2d;
fn opaque_depth(pixel: vec2<i32>) -> f32 {
    // A splat can still cover an open sample at a terrain or mesh edge.
    var farthest = 1.0;
    for (var sample = 0u; sample < textureNumSamples(opaque); sample++) {
        farthest = min(farthest, textureLoad(opaque, pixel, i32(sample)));
    }
    return farthest;
}"#
    } else {
        r#"@group(0) @binding(1) var opaque: texture_depth_2d;
fn opaque_depth(pixel: vec2<i32>) -> f32 {
    return textureLoad(opaque, pixel, 0);
}"#
    };
    [depth, include_str!("occlusion.wgsl")].concat()
}
