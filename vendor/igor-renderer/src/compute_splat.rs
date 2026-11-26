//! Original GPU implementation, driven by a caller-owned wgpu device/queue.
use bytemuck::{Pod,Zeroable};
use crate::splat::{Selection,Instance,GpuChoice,PointScene,ShadowField};
use crate::{Scene,render_api::FrameUniform};
use crate::render_api::{LANES,JOB_WIDTH,build_jobs,stats_word_offset};
pub use crate::render_api::screen_bytes;
use wgpu::util::DeviceExt;

struct Targets {words:wgpu::Buffer,_image:wgpu::Texture,view:wgpu::TextureView,stats_offset:u64}
impl Targets{
    fn new(device:&wgpu::Device,w:u32,h:u32,max_instances:u64)->Self{
        let words=device.create_buffer(&wgpu::BufferDescriptor{label:Some("atomic depth and owner"),size:screen_bytes(w,h,max_instances),usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_SRC,mapped_at_creation:false});
        let image=device.create_texture(&wgpu::TextureDescriptor{label:Some("compute resolved image"),size:wgpu::Extent3d{width:w,height:h,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8Unorm,usage:wgpu::TextureUsages::STORAGE_BINDING|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]});
        let view=image.create_view(&Default::default());
        Self{words,_image:image,view,stats_offset:stats_word_offset(w,h)*4}
    }
}
struct GpuLod {
    layout:wgpu::BindGroupLayout,pipeline:wgpu::ComputePipeline,
    choices:wgpu::Buffer,bind:wgpu::BindGroup,
}
impl GpuLod{
    fn bind(device:&wgpu::Device,layout:&wgpu::BindGroupLayout,r:&Resources<'_>,jobs:&wgpu::Buffer,choices:&wgpu::Buffer)->wgpu::BindGroup{
        device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("GPU LOD resources"),layout,entries:&[
            wgpu::BindGroupEntry{binding:0,resource:r.uniform.as_entire_binding()},
            wgpu::BindGroupEntry{binding:1,resource:r.instances.as_entire_binding()},
            wgpu::BindGroupEntry{binding:2,resource:choices.as_entire_binding()},
            wgpu::BindGroupEntry{binding:3,resource:jobs.as_entire_binding()},
        ]})
    }
    fn new(device:&wgpu::Device,r:&Resources<'_>,jobs:&wgpu::Buffer)->Self{
        let entry=|binding,ty|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Buffer{ty,has_dynamic_offset:false,min_binding_size:None},count:None};
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("GPU LOD layout"),entries:&[
            entry(0,wgpu::BufferBindingType::Uniform),entry(1,wgpu::BufferBindingType::Storage{read_only:false}),
            entry(2,wgpu::BufferBindingType::Storage{read_only:true}),entry(3,wgpu::BufferBindingType::Storage{read_only:false}),
        ]});
        let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:None,bind_group_layouts:&[&layout],push_constant_ranges:&[]});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("GPU brick LOD and jobs"),source:wgpu::ShaderSource::Wgsl(crate::render_api::GPU_LOD_SHADER.into())});
        let pipeline=device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some("GPU LOD"),layout:Some(&pl),module:&shader,entry_point:"select_cs"});
        let choices=device.create_buffer(&wgpu::BufferDescriptor{label:Some("GPU LOD candidates"),size:r.instances.size()/std::mem::size_of::<Instance>() as u64*std::mem::size_of::<GpuChoice>() as u64,usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let bind=Self::bind(device,&layout,r,jobs,&choices);
        Self{layout,pipeline,choices,bind}
    }
}
#[repr(C)]
#[derive(Clone,Copy,Pod,Zeroable)]
pub struct HistoryCamera{
    pub origin:[f32;4],pub forward:[f32;4],pub right:[f32;4],pub up:[f32;4],
    pub screen:[f32;4],pub valid:[u32;4],
}
struct Temporal{
    layout:wgpu::BindGroupLayout,pipeline:wgpu::ComputePipeline,
    previous:wgpu::Buffer,views:[wgpu::TextureView;2],_textures:[wgpu::Texture;2],
    binds:[wgpu::BindGroup;2],present:[wgpu::BindGroup;2],write:usize,valid:bool,
    last:HistoryCamera,
}
impl Temporal{
    fn new(device:&wgpu::Device,w:u32,h:u32,r:&Resources<'_>,t:&Targets,present_layout:&wgpu::BindGroupLayout)->Self{
        let entry=|binding,ty|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::COMPUTE,ty,count:None};
        let uniform=wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None};
        let sampled=wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false};
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("temporal layout"),entries:&[
            entry(0,uniform.clone()),entry(1,uniform),entry(2,sampled.clone()),entry(3,sampled),
            entry(4,wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba16Float,view_dimension:wgpu::TextureViewDimension::D2}),
            entry(5,wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:false},has_dynamic_offset:false,min_binding_size:None}),
        ]});
        let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:None,bind_group_layouts:&[&layout],push_constant_ranges:&[]});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("temporal reprojection"),source:wgpu::ShaderSource::Wgsl(crate::render_api::TEMPORAL_SHADER.into())});
        let pipeline=device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some("temporal accumulation"),layout:Some(&pl),module:&shader,entry_point:"temporal_cs"});
        let previous=device.create_buffer(&wgpu::BufferDescriptor{label:Some("previous camera"),size:std::mem::size_of::<HistoryCamera>() as u64,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let textures=std::array::from_fn(|_|device.create_texture(&wgpu::TextureDescriptor{label:Some("temporal color/depth history"),size:wgpu::Extent3d{width:w,height:h,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba16Float,usage:wgpu::TextureUsages::STORAGE_BINDING|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]}));
        let views=std::array::from_fn(|i|textures[i].create_view(&Default::default()));
        let binds=std::array::from_fn(|i|device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("temporal ping pong"),layout:&layout,entries:&[
            wgpu::BindGroupEntry{binding:0,resource:r.uniform.as_entire_binding()},
            wgpu::BindGroupEntry{binding:1,resource:previous.as_entire_binding()},
            wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::TextureView(&t.view)},
            wgpu::BindGroupEntry{binding:3,resource:wgpu::BindingResource::TextureView(&views[1-i])},
            wgpu::BindGroupEntry{binding:4,resource:wgpu::BindingResource::TextureView(&views[i])},
            wgpu::BindGroupEntry{binding:5,resource:t.words.as_entire_binding()},
        ]}));
        let present=std::array::from_fn(|i|device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("temporal presentation"),layout:present_layout,entries:&[
            wgpu::BindGroupEntry{binding:0,resource:r.uniform.as_entire_binding()},
            wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&views[i])},
        ]}));
        Self{layout,pipeline,previous,views,_textures:textures,binds,present,write:0,valid:false,last:HistoryCamera::zeroed()}
    }
    fn prepare(&mut self,queue:&wgpu::Queue,current:HistoryCamera,enabled:bool){
        self.last.valid[0]=u32::from(self.valid&&enabled);
        queue.write_buffer(&self.previous,0,bytemuck::bytes_of(&self.last));
        self.last=current;
        self.valid=enabled;
    }
}
pub struct ComputeSplats{
    layout:wgpu::BindGroupLayout,present_layout:wgpu::BindGroupLayout,
    gpu_lod:GpuLod,
    temporal:Temporal,
    clear:wgpu::ComputePipeline,depth:wgpu::ComputePipeline,owner:wgpu::ComputePipeline,resolve:wgpu::ComputePipeline,
    classify:wgpu::ComputePipeline,prepass:wgpu::ComputePipeline,hiz:[wgpu::ComputePipeline;4],cull:wgpu::ComputePipeline,
    readback:wgpu::Buffer,
    present:wgpu::RenderPipeline,hud:wgpu::RenderPipeline,
    jobs:wgpu::Buffer,capacity:usize,projection_cache:wgpu::Buffer,cache_capacity:u64,targets:Targets,
    bind:wgpu::BindGroup,present_bind:wgpu::BindGroup,
}
pub struct Resources<'a>{
    pub uniform:&'a wgpu::Buffer,pub raw:&'a wgpu::Buffer,pub lod:&'a wgpu::Buffer,pub instances:&'a wgpu::Buffer,
    pub patches:&'a wgpu::Buffer,pub materials:&'a wgpu::Buffer,pub atlases:&'a [wgpu::TextureView;2],pub sampler:&'a wgpu::Sampler,pub shadow:&'a wgpu::TextureView,
}
fn binds(device:&wgpu::Device,layout:&wgpu::BindGroupLayout,present_layout:&wgpu::BindGroupLayout,r:&Resources<'_>,jobs:&wgpu::Buffer,projection_cache:&wgpu::Buffer,t:&Targets)->(wgpu::BindGroup,wgpu::BindGroup){
    let bind=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("compute point resources"),layout,entries:&[
        wgpu::BindGroupEntry{binding:0,resource:r.uniform.as_entire_binding()},
        wgpu::BindGroupEntry{binding:1,resource:r.raw.as_entire_binding()},
        wgpu::BindGroupEntry{binding:2,resource:r.lod.as_entire_binding()},
        wgpu::BindGroupEntry{binding:3,resource:r.instances.as_entire_binding()},
        wgpu::BindGroupEntry{binding:4,resource:r.patches.as_entire_binding()},
        wgpu::BindGroupEntry{binding:5,resource:r.materials.as_entire_binding()},
        wgpu::BindGroupEntry{binding:6,resource:wgpu::BindingResource::TextureView(&r.atlases[0])},
        wgpu::BindGroupEntry{binding:7,resource:wgpu::BindingResource::TextureView(&r.atlases[1])},
        wgpu::BindGroupEntry{binding:8,resource:wgpu::BindingResource::Sampler(r.sampler)},
        wgpu::BindGroupEntry{binding:9,resource:jobs.as_entire_binding()},
        wgpu::BindGroupEntry{binding:10,resource:t.words.as_entire_binding()},
        wgpu::BindGroupEntry{binding:11,resource:wgpu::BindingResource::TextureView(&t.view)},
        wgpu::BindGroupEntry{binding:12,resource:projection_cache.as_entire_binding()},
        wgpu::BindGroupEntry{binding:13,resource:wgpu::BindingResource::TextureView(r.shadow)},
    ]});
    let present=device.create_bind_group(&wgpu::BindGroupDescriptor{label:Some("resolved frame"),layout:present_layout,entries:&[
        wgpu::BindGroupEntry{binding:0,resource:r.uniform.as_entire_binding()},
        wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&t.view)},
    ]});
    (bind,present)
}
impl ComputeSplats{
    pub fn new(device:&wgpu::Device,format:wgpu::TextureFormat,w:u32,h:u32,r:&Resources<'_>)->Self{
        let storage=|binding,read_only|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only},has_dynamic_offset:false,min_binding_size:None},count:None};
        let uniform=|visibility|wgpu::BindGroupLayoutEntry{binding:0,visibility,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None};
        let texture=|binding|wgpu::BindGroupLayoutEntry{binding,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None};
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("compute layout"),entries:&[
            uniform(wgpu::ShaderStages::COMPUTE),storage(1,true),storage(2,true),storage(3,true),storage(4,true),storage(5,true),
            texture(6),texture(7),wgpu::BindGroupLayoutEntry{binding:8,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None},
            storage(9,true),storage(10,false),storage(12,false),
            wgpu::BindGroupLayoutEntry{binding:13,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
            wgpu::BindGroupLayoutEntry{binding:11,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba8Unorm,view_dimension:wgpu::TextureViewDimension::D2},count:None},
        ]});
        let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:None,bind_group_layouts:&[&layout],push_constant_ranges:&[]});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("compute splat projection"),source:wgpu::ShaderSource::Wgsl(crate::render_api::COMPUTE_SHADER.into())});
        let pipe=|entry|device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor{label:Some(entry),layout:Some(&pl),module:&shader,entry_point:entry});
        let clear=pipe("clear_cs");let depth=pipe("depth_cs");let owner=pipe("owner_cs");let resolve=pipe("resolve_cs");
        let classify=pipe("classify_cs");let prepass=pipe("prepass_cs");let hiz=[pipe("hiz1_cs"),pipe("hiz2_cs"),pipe("hiz3_cs"),pipe("hiz4_cs")];let cull=pipe("cull_cs");
        let readback=device.create_buffer(&wgpu::BufferDescriptor{label:Some("occlusion stats readback"),size:16,usage:wgpu::BufferUsages::MAP_READ|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let present_layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor{label:Some("present layout"),entries:&[
            uniform(wgpu::ShaderStages::FRAGMENT),
            wgpu::BindGroupLayoutEntry{binding:1,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:false},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
        ]});
        let ppl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor{label:None,bind_group_layouts:&[&present_layout],push_constant_ranges:&[]});
        let ps=device.create_shader_module(wgpu::ShaderModuleDescriptor{label:Some("compute presentation"),source:wgpu::ShaderSource::Wgsl(crate::render_api::PRESENT_SHADER.into())});
        let display=|entry|device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{
            label:Some(entry),layout:Some(&ppl),vertex:wgpu::VertexState{module:&ps,entry_point:"fullscreen_vs",buffers:&[]},
            fragment:Some(wgpu::FragmentState{module:&ps,entry_point:entry,targets:&[Some(wgpu::ColorTargetState{format,blend:None,write_mask:wgpu::ColorWrites::ALL})]}),
            primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview:None,
        });
        let present=display("present_fs");let hud=display("hud_fs");
        let capacity=1;
        let jobs=device.create_buffer_init(&wgpu::util::BufferInitDescriptor{label:Some("point jobs"),contents:&[0u8;16],usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST});
        let projection_cache=device.create_buffer(&wgpu::BufferDescriptor{label:Some("projection cache"),size:8,usage:wgpu::BufferUsages::STORAGE,mapped_at_creation:false});
        let cache_capacity=8;
        let targets=Targets::new(device,w,h,r.instances.size()/std::mem::size_of::<Instance>() as u64);
        let (bind,present_bind)=binds(device,&layout,&present_layout,r,&jobs,&projection_cache,&targets);
        let gpu_lod=GpuLod::new(device,r,&jobs);
        let temporal=Temporal::new(device,w,h,r,&targets,&present_layout);
        Self{layout,present_layout,gpu_lod,temporal,clear,depth,owner,resolve,classify,prepass,hiz,cull,readback,present,hud,jobs,capacity,projection_cache,cache_capacity,targets,bind,present_bind}
    }
    fn rebind(&mut self,device:&wgpu::Device,r:&Resources<'_>){
        (self.bind,self.present_bind)=binds(device,&self.layout,&self.present_layout,r,&self.jobs,&self.projection_cache,&self.targets);
        self.gpu_lod.bind=GpuLod::bind(device,&self.gpu_lod.layout,r,&self.jobs,&self.gpu_lod.choices);
    }
    pub fn resize(&mut self,device:&wgpu::Device,w:u32,h:u32,r:&Resources<'_>){
        self.targets=Targets::new(device,w,h,r.instances.size()/std::mem::size_of::<Instance>() as u64);self.rebind(device,r);
        self.temporal=Temporal::new(device,w,h,r,&self.targets,&self.present_layout);
    }
    pub fn prepare(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,r:&Resources<'_>,selection:&Selection,cache_requested:bool,cache_budget:u64,gpu_lod:bool)->Result<(u32,bool),String>{
        let job_count:usize=selection.instances.iter().map(|i|i.pad[1].div_ceil(LANES) as usize).sum();
        if job_count>((u32::MAX/64) as usize)||job_count as u64>u64::from(device.limits().max_compute_workgroups_per_dimension)*u64::from(JOB_WIDTH){
            return Err("compute job dispatch exceeds limit".into());
        }
        if job_count>self.capacity{
            let capacity=job_count.next_power_of_two();
            let bytes=capacity as u64*16;
            let limits=device.limits();
            if bytes>u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size){return Err("compute job buffer exceeds limit".into());}
            self.jobs=device.create_buffer(&wgpu::BufferDescriptor{label:Some("point jobs"),size:bytes,usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
            self.capacity=capacity;self.rebind(device,r);
        }
        if gpu_lod{
            let mut choices=selection.gpu_choices.clone();
            let mut first=0u32;
            for (choice,inst) in choices.iter_mut().zip(&selection.instances){
                choice.metric[3]=f32::from_bits(first);
                first+=inst.pad[1].div_ceil(LANES);
            }
            if !choices.is_empty(){queue.write_buffer(&self.gpu_lod.choices,0,bytemuck::cast_slice(&choices));}
        }else{
            let jobs=build_jobs(selection);
            if !jobs.is_empty(){queue.write_buffer(&self.jobs,0,bytemuck::cast_slice(&jobs));}
        }
        let bytes=(job_count as u64*64*8).max(8);
        let limits=device.limits();
        let cache_ok=cache_requested && bytes<=cache_budget && bytes<=u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
        if cache_ok && bytes>self.cache_capacity{
            // Exact rounded allocation: avoid doubling a large transient buffer.
            let cap=bytes.div_ceil(1048576)*1048576;
            let cap=if cap<=cache_budget && cap<=u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size){cap}else{bytes};
            self.projection_cache=device.create_buffer(&wgpu::BufferDescriptor{label:Some("projection cache"),size:cap,usage:wgpu::BufferUsages::STORAGE,mapped_at_creation:false});
            self.cache_capacity=cap;self.rebind(device,r);
        }else if !cache_ok && self.cache_capacity>8{
            self.projection_cache=device.create_buffer(&wgpu::BufferDescriptor{label:Some("disabled projection cache"),size:8,usage:wgpu::BufferUsages::STORAGE,mapped_at_creation:false});
            self.cache_capacity=8;self.rebind(device,r);
        }
        Ok((job_count as u32,cache_ok))
    }
    pub fn reset_history(&mut self){self.temporal.valid=false;}
    pub fn prepare_history(&mut self,queue:&wgpu::Queue,current:HistoryCamera,enabled:bool){self.temporal.prepare(queue,current,enabled);}
    pub fn read_stats(&self,device:&wgpu::Device)->Option<[u32;4]>{
        let slice=self.readback.slice(..);
        let (tx,rx)=std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read,move|r|{let _=tx.send(r);});
        // Called only after this frame's queue-completion notification.
        device.poll(wgpu::Maintain::Wait);
        if rx.recv().ok()?.is_err(){return None;}
        let result={
            let data=slice.get_mapped_range();
            let mut values=[0u32;4];
            for (i,v) in values.iter_mut().enumerate(){*v=u32::from_le_bytes(data[i*4..i*4+4].try_into().unwrap());}
            values
        };
        self.readback.unmap();
        Some(result)
    }
    /// Record the original compute passes into a host-owned command encoder.
    /// No submission, window or render pass is performed here. The resolved
    /// image and depth/owner buffer remain owned by this renderer.
    pub fn encode_passes(&mut self,encoder:&mut wgpu::CommandEncoder,w:u32,h:u32,job_count:u32,instance_count:u32,occlusion:bool,gpu_lod:bool,taa:bool){
        let xy=(w.div_ceil(8),h.div_ceil(8));
        let job_xy=(job_count.min(JOB_WIDTH),job_count.div_ceil(JOB_WIDTH));
        let groups=instance_count.div_ceil(LANES);
        let inst_xy=(groups.min(JOB_WIDTH),groups.div_ceil(JOB_WIDTH));
        if gpu_lod && groups>0{
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("GPU select and generate point jobs"),timestamp_writes:None});
            pass.set_pipeline(&self.gpu_lod.pipeline);pass.set_bind_group(0,&self.gpu_lod.bind,&[]);
            pass.dispatch_workgroups(inst_xy.0,inst_xy.1,1);
        }
        {
            let mut dispatch=|pipeline:&wgpu::ComputePipeline,label:&str,groups:(u32,u32)|{
                if groups.0==0||groups.1==0{return;}
                let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some(label),timestamp_writes:None});
                pass.set_pipeline(pipeline);pass.set_bind_group(0,&self.bind,&[]);pass.dispatch_workgroups(groups.0,groups.1,1);
            };
            dispatch(&self.clear,"clear depth and owner",xy);
            if occlusion {
                dispatch(&self.classify,"classify nearby occluders",inst_xy);
                dispatch(&self.prepass,"near occluder depth",job_xy);
                for (i,s) in [4u32,16,64,256].into_iter().enumerate(){
                    dispatch(&self.hiz[i],"max depth pyramid",(w.div_ceil(s).div_ceil(8),h.div_ceil(s).div_ceil(8)));
                }
                dispatch(&self.cull,"conservative instance occlusion",inst_xy);
            }
            dispatch(&self.depth,"remaining closest depth",job_xy);
            dispatch(&self.owner,"atomic winner ID",job_xy);
            dispatch(&self.resolve,"shade winners",xy);
        }
        if occlusion {encoder.copy_buffer_to_buffer(&self.targets.words,self.targets.stats_offset,&self.readback,0,16);}
        if taa{
            let i=self.temporal.write;
            {
                let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("temporal reprojection"),timestamp_writes:None});
                pass.set_pipeline(&self.temporal.pipeline);pass.set_bind_group(0,&self.temporal.binds[i],&[]);
                pass.dispatch_workgroups(xy.0,xy.1,1);
            }
            self.temporal.write=1-i;
        }
    }
    /// Original resolved color (or most recently written temporal output).
    pub fn resolved_view(&self,taa:bool)->&wgpu::TextureView{
        if taa{&self.temporal.views[1-self.temporal.write]}else{&self.targets.view}
    }
    /// Original screen layout. Depth is positive view-space float bits;
    /// owner IDs are in the next width*height words. Not hardware depth.
    pub fn screen_words(&self)->&wgpu::Buffer{&self.targets.words}
    pub fn encode(&mut self,encoder:&mut wgpu::CommandEncoder,color:&wgpu::TextureView,w:u32,h:u32,job_count:u32,instance_count:u32,occlusion:bool,gpu_lod:bool,taa:bool){
        self.encode_passes(encoder,w,h,job_count,instance_count,occlusion,gpu_lod,taa);
        let present_bind=if taa{&self.temporal.present[1-self.temporal.write]}else{&self.present_bind};
        // The only triangles in this mode are screen presentation/HUD,
        // never tree or ground geometry.
        let mut pass=encoder.begin_render_pass(&wgpu::RenderPassDescriptor{label:Some("present computed pixels"),color_attachments:&[Some(wgpu::RenderPassColorAttachment{view:color,resolve_target:None,ops:wgpu::Operations{load:wgpu::LoadOp::Clear(wgpu::Color::BLACK),store:wgpu::StoreOp::Store}})],depth_stencil_attachment:None,occlusion_query_set:None,timestamp_writes:None});
        pass.set_bind_group(0,present_bind,&[]);
        pass.set_pipeline(&self.present);pass.draw(0..3,0..1);
        pass.set_pipeline(&self.hud);pass.draw(0..3,0..1);
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    use crate::splat::Draw;
    #[test]fn pyramid_layout(){assert_eq!(stats_word_offset(5,3),34);assert_eq!(screen_bytes(5,3,7),180);}
    #[test]fn jobs_cover_each_point_once(){
        let inst=Instance{origin_scale:[0.0,0.0,0.0,1.0],offset:0,turn:0,pad:[0;2],bound:[0.0;4]};
        let selection=Selection{gpu_choices:vec![Default::default();3],instances:vec![inst;3],draws:vec![Draw{point_count:65,start:0,end:2},Draw{point_count:1,start:2,end:3}],points:131,raw_points:0,visited:0,world_splats:0,trees:3,threshold:1.0,budget_met:true};
        let jobs=build_jobs(&selection);
        assert_eq!(jobs.len(),5);
        assert_eq!(jobs.iter().map(|j|u64::from(j.count)).sum::<u64>(),131);
        assert_eq!((jobs[1].instance_id,jobs[1].first,jobs[1].count),(0,64,1));
        assert_eq!((jobs[4].instance_id,jobs[4].first,jobs[4].count),(2,0,1));
    }
}

/// Persistent scene data. The demo uses this same uploader; embedders may
/// alternatively supply equivalent Resources from their own allocations.
pub struct SceneBuffers{
    pub uniform:wgpu::Buffer,pub raw:wgpu::Buffer,pub lod:wgpu::Buffer,pub instances:wgpu::Buffer,
    pub patches:wgpu::Buffer,pub materials:wgpu::Buffer,pub atlases:[wgpu::TextureView;2],
    pub sampler:wgpu::Sampler,pub shadow_view:wgpu::TextureView,
}
impl SceneBuffers{
    pub fn upload(device:&wgpu::Device,queue:&wgpu::Queue,scene:&Scene,points:&PointScene,shadow_field:&ShadowField)->Self{
        let instance_bytes=points.max_instances().max(1) as u64*std::mem::size_of::<Instance>() as u64;
    let uniform=device.create_buffer(&wgpu::BufferDescriptor{label:Some("camera"),size:std::mem::size_of::<FrameUniform>() as u64,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
    let zero=[0u8;16];
    let upload=|label:&str,data:&[u8]|device.create_buffer_init(&wgpu::util::BufferInitDescriptor{label:Some(label),contents:if data.is_empty(){&zero}else{data},usage:wgpu::BufferUsages::STORAGE});
    let raw=upload("raw points",bytemuck::cast_slice(&points.raw));
    let lod=upload("flat LOD points and world metadata",bytemuck::cast_slice(&points.lod));
    let patches=upload("source surface coordinates",bytemuck::cast_slice(&scene.geometry.patches));
    let materials=upload("source materials",bytemuck::cast_slice(&scene.geometry.materials));
    let atlases=scene.geometry.upload_textures(device,queue);
    let sampler=device.create_sampler(&wgpu::SamplerDescriptor{label:Some("source material sampling"),mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,..Default::default()});
    let shadow_extent=wgpu::Extent3d{width:shadow_field.width,height:shadow_field.height,depth_or_array_layers:1};
    let shadow_texture=device.create_texture(&wgpu::TextureDescriptor{label:Some("static canopy transmittance"),size:shadow_extent,mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8Unorm,usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,view_formats:&[]});
    queue.write_texture(wgpu::ImageCopyTexture{texture:&shadow_texture,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},&shadow_field.pixels,wgpu::ImageDataLayout{offset:0,bytes_per_row:Some(shadow_field.width*4),rows_per_image:Some(shadow_field.height)},shadow_extent);
    let shadow_view=shadow_texture.create_view(&Default::default());
    let instances=device.create_buffer(&wgpu::BufferDescriptor{label:Some("selected LOD instances"),size:instance_bytes,usage:wgpu::BufferUsages::STORAGE|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});

        Self{uniform,raw,lod,instances,patches,materials,atlases,sampler,shadow_view}
    }
    pub fn resources(&self)->Resources<'_>{
        Resources{uniform:&self.uniform,raw:&self.raw,lod:&self.lod,instances:&self.instances,
            patches:&self.patches,materials:&self.materials,atlases:&self.atlases,sampler:&self.sampler,shadow:&self.shadow_view}
    }
    pub fn write_frame(&self,queue:&wgpu::Queue,frame:&FrameUniform){
        queue.write_buffer(&self.uniform,0,bytemuck::bytes_of(frame));
    }
    pub fn write_instances(&self,queue:&wgpu::Queue,selection:&Selection)->Result<(),String>{
        let data=bytemuck::cast_slice(&selection.instances);
        if data.len() as u64>self.instances.size(){return Err("selection exceeds instance buffer".into());}
        if !data.is_empty(){queue.write_buffer(&self.instances,0,data);}
        Ok(())
    }
}
