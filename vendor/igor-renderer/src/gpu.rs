use crate::scene_canopy::Selector;
use crate::{Camera, Instance, Models, Region, Stats, View};
use bytemuck::{Pod, Zeroable};
use std::{collections::HashMap, mem::size_of, ops::Range};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    origin: [f32; 4],
    params: [u32; 4],
    effects: [f32; 4],
    shadow_transform: [f32; 4],
    shadow_projection: [f32; 4],
    sun: [f32; 4],
    forest_band: [f32; 4],
    clip_from_local: [[f32; 4]; 4],
}
fn upload(d: &wgpu::Device, label: &str, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: if data.is_empty() { &[0u8; 16] } else { data },
        usage,
    })
}
struct BatchDraw {
    indices: Range<u32>,
    instances: Range<u32>,
}
fn add_batch(
    dst: &mut Vec<Instance>,
    draws: &mut Vec<BatchDraw>,
    indices: Range<u32>,
    src: &[Instance],
) -> u64 {
    if src.is_empty() || indices.is_empty() {
        return 0;
    }
    let start = dst.len() as u32;
    dst.extend_from_slice(src);
    let end = dst.len() as u32;
    let tris = u64::from(indices.end - indices.start) / 3 * u64::from(end - start);
    draws.push(BatchDraw {
        indices,
        instances: start..end,
    });
    tris
}

/// Host attachment formats and sample count. Depth uses Bevy's reverse-Z convention.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct PipelineKey {
    /// Format of the shared color attachment.
    pub color: wgpu::TextureFormat,
    /// Format of the shared depth attachment.
    pub depth: wgpu::TextureFormat,
    /// Sample count of both attachments.
    pub samples: u32,
}

/// Shared Igor GPU pipelines and tree templates on the host's wgpu device.
pub struct Renderer {
    full_v: wgpu::Buffer,
    indices: wgpu::Buffer,
    mid_v: wgpu::Buffer,
    mid_i: wgpu::Buffer,
    materials: wgpu::Buffer,
    atlases: [wgpu::TextureView; 2],
    sampler: wgpu::Sampler,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    pipelines: HashMap<PipelineKey, [wgpu::RenderPipeline; 2]>,
}

/// GPU buffers and LOD history for a single resident region of the forest.
pub struct RegionDraw {
    selection: Selector,
    packed: Vec<Instance>,
    draws: [Vec<BatchDraw>; 3],
    far_v: wgpu::Buffer,
    far_i: wgpu::Buffer,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
}

impl Renderer {
    /// Upload the shared model buffers and atlas to the existing device and queue.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, models: &Models) -> Self {
        let layout = create_layout(device);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Igor hybrid pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Igor hybrid scene shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene_hybrid.wgsl").into()),
        });
        Self {
            full_v: upload(
                device,
                "Igor LOD0 mesh vertices",
                bytemuck::cast_slice(&models.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            indices: upload(
                device,
                "Igor original LOD0 indices",
                bytemuck::cast_slice(&models.indices),
                wgpu::BufferUsages::INDEX,
            ),
            mid_v: upload(
                device,
                "Igor middle canopy vertices",
                bytemuck::cast_slice(&models.proxies.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            mid_i: upload(
                device,
                "Igor middle canopy indices",
                bytemuck::cast_slice(&models.proxies.indices),
                wgpu::BufferUsages::INDEX,
            ),
            materials: upload(
                device,
                "Igor materials",
                bytemuck::cast_slice(&models.geometry.materials),
                wgpu::BufferUsages::STORAGE,
            ),
            atlases: models.geometry.upload_textures(device, queue),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            layout,
            pipeline_layout,
            shader,
            pipelines: HashMap::new(),
        }
    }

    /// Create Igor's two triangle pipelines for the view's attachment configuration.
    pub fn prepare_pipeline(&mut self, device: &wgpu::Device, key: PipelineKey) {
        self.pipelines
            .entry(key)
            .or_insert_with(|| create_pipelines(device, &self.pipeline_layout, &self.shader, key));
    }

    /// Upload the original shadow field and allocate this region's instance buffers.
    pub fn add_region(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        region: &Region,
    ) -> RegionDraw {
        let shadow = &region.shadow;
        let shadow_extent = wgpu::Extent3d {
            width: shadow.width,
            height: shadow.height,
            depth_or_array_layers: 1,
        };
        let shadow_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Igor static canopy shadows"),
            size: shadow_extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &shadow_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &shadow.pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(shadow.width * 4),
                rows_per_image: Some(shadow.height),
            },
            shadow_extent,
        );
        let shadow_view = shadow_texture.create_view(&Default::default());

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Igor scene camera"),
            size: size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Igor visible tree instances"),
            size: (region.placed.len() * 3 * size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.materials.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.atlases[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.atlases[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
            ],
        });

        RegionDraw {
            selection: Selector::new(&region.canopy, region.placed.len(), region.mid.clone()),
            packed: Vec::with_capacity(region.placed.len()),
            draws: std::array::from_fn(|_| Vec::new()),
            far_v: upload(
                device,
                "Igor far canopy vertices",
                bytemuck::cast_slice(&region.canopy.geometry.vertices),
                wgpu::BufferUsages::VERTEX,
            ),
            far_i: upload(
                device,
                "Igor far canopy indices",
                bytemuck::cast_slice(&region.canopy.geometry.indices),
                wgpu::BufferUsages::INDEX,
            ),
            group,
            uniform,
            instances,
        }
    }

    /// Submit the forest into a pass opened by the host with its shared color and depth.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, key: PipelineKey, regions: &[&RegionDraw]) {
        let [full_pipe, proxy_pipe] = &self.pipelines[&key];
        for (rank, pipeline) in [full_pipe, proxy_pipe, proxy_pipe].into_iter().enumerate() {
            if regions.iter().all(|region| region.draws[rank].is_empty()) {
                continue;
            }
            pass.set_pipeline(pipeline);
            if rank < 2 {
                let (vertices, indices) = if rank == 0 {
                    (&self.full_v, &self.indices)
                } else {
                    (&self.mid_v, &self.mid_i)
                };
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            }
            for region in regions
                .iter()
                .filter(|region| !region.draws[rank].is_empty())
            {
                if rank == 2 {
                    pass.set_vertex_buffer(0, region.far_v.slice(..));
                    pass.set_index_buffer(region.far_i.slice(..), wgpu::IndexFormat::Uint32);
                }
                pass.set_bind_group(0, &region.group, &[]);
                pass.set_vertex_buffer(1, region.instances.slice(..));
                for draw in &region.draws[rank] {
                    pass.draw_indexed(draw.indices.clone(), 0, draw.instances.clone());
                }
            }
        }
    }
}

impl RegionDraw {
    /// Reset the original LOD state when a whole region leaves the view.
    pub fn hide(&mut self) {
        self.selection.reset();
    }

    /// Advance the standalone selector and upload the original mask ranges.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        models: &Models,
        region: &Region,
        camera: Camera,
        samples: u32,
        elapsed: f32,
        time: f32,
    ) -> Stats {
        let selected = self.selection.select(
            &region.canopy,
            View::new(camera),
            &region.placed,
            true,
            true,
            elapsed.min(0.1),
        );
        self.packed.clear();
        for group in &mut self.draws {
            group.clear();
        }
        let mut triangles = 0;
        for (i, list) in selected.full.iter().enumerate() {
            let range = models.draws[region.models[i]].clone();
            triangles += add_batch(&mut self.packed, &mut self.draws[0], range, list);
        }
        for (i, list) in selected.mid.iter().enumerate() {
            let draw = region.mid[i];
            let range = draw.first_index..draw.first_index + draw.index_count;
            triangles += add_batch(&mut self.packed, &mut self.draws[1], range, list);
        }
        for (draw, instance) in selected.far {
            let range = draw.first_index..draw.first_index + draw.index_count;
            triangles += add_batch(&mut self.packed, &mut self.draws[2], range, &[instance]);
        }
        if !self.packed.is_empty() {
            queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&self.packed));
            let ubo = Uniform {
                origin: camera.origin.extend(0.0).to_array(),
                params: [
                    samples,
                    region.effects.coverage.to_bits(),
                    region.effects.grain.to_bits(),
                    region.effects.flags,
                ],
                effects: [
                    time,
                    region.effects.wind_strength,
                    region.effects.shadow_strength
                        * shadow_alignment(camera.sun_direction.dot(region.shadow.sun_direction)),
                    camera.exposure_scale,
                ],
                shadow_transform: region.shadow.transform,
                shadow_projection: [
                    region.shadow.projection.x,
                    region.shadow.projection.y,
                    region.shadow.band_base,
                    region.shadow.band_height.recip(),
                ],
                sun: camera.sun_direction.extend(0.0).to_array(),
                forest_band: [
                    camera.forest_band[0],
                    camera.forest_band[1],
                    camera.forest_band[0].powi(2),
                    0.0,
                ],
                clip_from_local: camera.clip_from_local.to_cols_array_2d(),
            };
            queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&ubo));
        }
        Stats {
            levels: [selected.full_count, selected.mid_count, selected.far_count],
            transitions: selected.fades,
            triangles,
        }
    }
}

fn shadow_alignment(dot: f32) -> f32 {
    // Fade baked shadows as the real sun moves away from the bake direction.
    let t = ((dot - 0.94) / (0.99 - 0.94)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn create_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let texture = |binding, dimension| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            texture(2, wgpu::TextureViewDimension::D2Array),
            texture(3, wgpu::TextureViewDimension::D2Array),
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            texture(5, wgpu::TextureViewDimension::D2),
        ],
    })
}

fn create_pipelines(
    device: &wgpu::Device,
    pl: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    key: PipelineKey,
) -> [wgpu::RenderPipeline; 2] {
    const VA: [wgpu::VertexAttribute; 5] =
        wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x2,4=>Uint32];
    const PA: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0=>Float32x3,1=>Unorm16x4,2=>Float32,3=>Snorm8x4];
    const IA: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![5=>Float32x4,6=>Float32x4];
    let inst_layout = || {
        Some(wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &IA,
        })
    };
    let full_layout = [
        Some(wgpu::VertexBufferLayout {
            array_stride: 44,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VA,
        }),
        inst_layout(),
    ];
    let proxy_layout = [
        Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<crate::scene_canopy::Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &PA,
        }),
        inst_layout(),
    ];
    let pipe = |vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout<'_>>]| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(fs),
            layout: Some(pl),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: key.color,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: key.depth,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: key.samples,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let full_pipe = pipe("mesh_vs", "mesh_fs", &full_layout);
    let proxy_pipe = pipe("proxy_vs", "proxy_fs", &proxy_layout);
    [full_pipe, proxy_pipe]
}
