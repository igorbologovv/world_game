//! Experimental bridge for the standalone Igor direct-compute renderer.
//! No second device/window: use Bevy's RenderDevice and real scene depth.
use std::{collections::{BTreeMap, HashMap}, sync::{Arc, atomic::{AtomicBool, Ordering}}};
use bevy::{
    camera::Exposure,
    core_pipeline::{core_3d::{main_opaque_pass_3d, main_transparent_pass_3d, CORE_3D_DEPTH_FORMAT}, schedule::Core3d, Core3dSystems, FullscreenShader},
    math::DVec3,
    prelude::*,
    render::{
        render_resource::{binding_types::*, *},
        renderer::{RenderContext, RenderDevice, RenderQueue, ViewQuery},
        sync_world::RenderEntity,
        view::{ExtractedView, Msaa, ViewDepthStencilTexture, ViewTarget},
        Extract, ExtractSchedule, RenderApp, RenderStartup,
    },
};
use world_core::runtime::WorldCamera;
use super::{impostor::{self, Portraits}, placement::Site};

const BUDGET: u32 = 3_500_000;
const MAX_RECORDS: usize = 5_000_000;
const JOB_WIDTH: u32 = 1024;
const WIDTHS: [f32; 5] = [0.0, 0.10, 0.16, 0.25, 0.40];

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub position: Vec3,
    pub radius: f32,
    pub color: Vec3,
    pub normal: Vec3,
    pub leaf: bool,
}
fn packed(s: Sample) -> UVec4 {
    let q = |v: f32| ((v*256.0).round().clamp(-32768.0,32767.0) as i16 as u16) as u32;
    let r = (s.radius*4096.0).ceil().clamp(1.0,65535.0) as u32;
    let color = (s.color.clamp(Vec3::ZERO,Vec3::ONE)*255.0).round().as_uvec3();
    let [nx,ny] = impostor::encode_normal(s.normal);
    UVec4::new(q(s.position.x)|(q(s.position.y)<<16),q(s.position.z)|(r<<16),
        color.x|(color.y<<8)|(color.z<<16)|0xff000000,
        u32::from(nx)|(u32::from(ny)<<8)|(u32::from(s.leaf)<<16))
}
#[derive(Default)]
struct Aggregate { position:Vec3, color:Vec3, normal:Vec3, area:f32 }
fn aggregate(samples: &[Sample], width: f32) -> Vec<UVec4> {
    let mut cells: BTreeMap<(i32,i32,i32,bool),Aggregate> = BTreeMap::new();
    for s in samples {
        let c = (s.position/width).floor().as_ivec3();
        let cell = cells.entry((c.x,c.y,c.z,s.leaf)).or_default();
        let a = s.radius*s.radius;
        cell.position += s.position*a; cell.color += s.color*a;
        cell.normal += s.normal*a; cell.area += a;
    }
    cells.into_iter().filter_map(|((_,_,_,leaf),a)| {
        (a.area>0.0).then(||packed(Sample {
            position:a.position/a.area,color:a.color/a.area,
            normal:a.normal.normalize_or_zero(),
            radius:a.area.sqrt().min(width*0.86),leaf,
        }))
    }).collect()
}
#[derive(Clone, Copy, Default)]
struct Range { start:u32, count:u32 }
#[derive(Clone, Copy, ShaderType)]
struct GpuTree { origin_scale:Vec4, rotation:Vec4, range:UVec4 }
#[derive(Clone, Default, ShaderType)]
struct PointArray { #[size(runtime)] data:Vec<UVec4> }
#[derive(Clone, Default, ShaderType)]
struct TreeArray { #[size(runtime)] data:Vec<GpuTree> }
#[derive(Clone, Default, ShaderType)]
struct JobArray { #[size(runtime)] data:Vec<UVec4> }
#[derive(Resource)]
pub(super) struct PointForest {
    geometry:Arc<Vec<UVec4>>,
    templates:HashMap<usize,[Range;WIDTHS.len()]>,
    ready:Arc<AtomicBool>,
    trees:Vec<GpuTree>,
    jobs:Vec<UVec4>,
    selected:u32,
    baked_this_frame:bool,
    budget:u32,
}
impl Default for PointForest {
    fn default() -> Self {
        Self{geometry:Arc::new(Vec::new()),templates:HashMap::new(),ready:Arc::new(AtomicBool::new(false)),
            trees:Vec::new(),jobs:Vec::new(),selected:0,baked_this_frame:false,
            budget:std::env::var("IGOR_POINT_BUDGET").ok().and_then(|v|v.parse().ok()).unwrap_or(BUDGET).clamp(1,8_000_000)}
    }
}
impl PointForest {
    pub(super) fn begin_frame(&mut self) {
        self.trees.clear();self.jobs.clear();self.selected=0;self.baked_this_frame=false;
    }
    pub(super) fn add_tree(&mut self, portraits:&Portraits, site:Site, eye:DVec3, distance:f32, mesh_level:usize) -> bool {
        if !self.ready.load(Ordering::Acquire) { return false; }
        let tile=impostor::tile(site.species,site.variant);
        if !self.templates.contains_key(&tile) {
            if self.baked_this_frame { return false; }
            self.baked_this_frame=true;
            let Some(samples)=portraits.point_samples(site.species,site.variant) else {return false;};
            if samples.is_empty() || samples.iter().any(|s|s.position.abs().max_element()>=127.0) {return false;}
            let mut levels:[Vec<UVec4>;WIDTHS.len()]=std::array::from_fn(|i|
                if i==0 {samples.iter().copied().map(packed).collect()} else {aggregate(&samples,WIDTHS[i])});
            // Avoid count inversions between independently sampled grids.
            for i in (0..WIDTHS.len()-1).rev() {
                if levels[i].len()<levels[i+1].len(){levels[i+1]=levels[i].clone();}
            }
            let count:usize=levels.iter().map(Vec::len).sum();
            if self.geometry.len()+count>MAX_RECORDS {return false;}
            let storage=Arc::make_mut(&mut self.geometry);
            let ranges=std::array::from_fn(|i| {
                let start=storage.len() as u32;
                storage.append(&mut levels[i]);
                Range{start,count:storage.len() as u32-start}
            });
            self.templates.insert(tile,ranges);
            info!("Point forest: baked template {tile}, {count} shared records, total {}",storage.len());
        }
        let levels=&self.templates[&tile];
        let preferred=match mesh_level {0=>usize::from(distance>20.0),1=>2, _=>if distance>65.0{4}else{3}};
        let Some(range)=levels[preferred..].iter().copied().find(|r|r.count>0 && self.selected.saturating_add(r.count)<=self.budget) else {return false;};
        let rotation=site.rotation();
        let index=self.trees.len() as u32;
        // Subtract in f64 before converting to the GPU's local coordinates.
        let relative=(site.position-eye).as_vec3();
        self.trees.push(GpuTree{
            origin_scale:relative.extend(site.scale),
            rotation:Vec4::from_array(rotation.to_array()),
            range:UVec4::new(range.start,range.count,site.yaw.to_bits(),site.height().to_bits()),
        });
        for first in (0..range.count).step_by(64) {
            self.jobs.push(UVec4::new(index,first,(range.count-first).min(64),0));
        }
        self.selected+=range.count;
        true
    }
}
pub(super) fn install(app:&mut App) {
    if std::env::var("IGOR_POINT_FOREST").as_deref()!=Ok("1"){return;}
    app.init_resource::<PointForest>();
    // First integration uses a single-sample depth buffer. Existing
    // renderer (and its MSAA setting) is untouched without the flag.
    app.add_systems(PostUpdate, single_sample_camera);
    if let Some(render)=app.get_sub_app_mut(RenderApp) {
        render.add_systems(ExtractSchedule,extract_frame)
            .add_systems(RenderStartup,init_pipelines)
            .add_systems(Core3d,render_points.in_set(Core3dSystems::MainPass)
                .after(main_opaque_pass_3d).before(main_transparent_pass_3d));
    }
}
fn single_sample_camera(mut cameras:Query<&mut Msaa,With<WorldCamera>>) {
    for mut msaa in &mut cameras {if *msaa!=Msaa::Off{*msaa=Msaa::Off;}}
}

#[derive(Resource)]
struct ExtractedForest {
    geometry:Arc<Vec<UVec4>>, ready:Arc<AtomicBool>,
    trees:Vec<GpuTree>,jobs:Vec<UVec4>,
}
#[derive(Component)]
struct PointCamera { clip:Mat4, sun:Vec3, sunlight:Vec3, elapsed:f32 }
fn extract_frame(
    mut commands:Commands,
    forest:Extract<Res<PointForest>>,
    cameras:Extract<Query<(RenderEntity,&Transform,&Projection,&Exposure),With<WorldCamera>>>,
    lights:Extract<Query<(&DirectionalLight,&Transform)>>,
    time:Extract<Res<Time>>,
) {
    commands.insert_resource(ExtractedForest{geometry:forest.geometry.clone(),ready:forest.ready.clone(),
        trees:forest.trees.clone(),jobs:forest.jobs.clone()});
    let sun=lights.iter().max_by(|a,b|a.0.illuminance.total_cmp(&b.0.illuminance));
    for (entity,transform,projection,exposure) in &*cameras {
        if !matches!(projection,Projection::Perspective(_)) {
            commands.entity(entity).remove::<PointCamera>();continue;
        }
        let (direction,light)=sun.map_or((Vec3::Y,Vec3::ONE),|(light,transform)|
            (transform.back().as_vec3(),light.color.to_linear().to_vec3()*light.illuminance*exposure.exposure()));
        commands.entity(entity).insert(PointCamera{
            clip:projection.get_clip_from_view()*Mat4::from_quat(transform.rotation.conjugate()),
            sun:direction,sunlight:light,elapsed:time.elapsed_secs(),
        });
    }
}
#[derive(Clone,Default,ShaderType)]
struct FrameUniform { clip:Mat4, viewport:UVec4, params:UVec4, sun_time:Vec4, sunlight:Vec4 }
#[derive(Resource)]
struct Pipelines { compute:BindGroupLayoutDescriptor, composite:BindGroupLayoutDescriptor,
    depth:CachedComputePipelineId,owner:CachedComputePipelineId,resolve:CachedComputePipelineId,
    draw:CachedRenderPipelineId }
fn init_pipelines(mut commands:Commands, assets:Res<AssetServer>,cache:Res<PipelineCache>,fullscreen:Res<FullscreenShader>){
    let compute=BindGroupLayoutDescriptor::new("point forest compute",&BindGroupLayoutEntries::sequential(
        ShaderStages::COMPUTE,(uniform_buffer::<FrameUniform>(false),
            storage_buffer_read_only_sized(false,None),storage_buffer_read_only_sized(false,None),
            storage_buffer_read_only_sized(false,None),storage_buffer_sized(false,None))));
    let composite=BindGroupLayoutDescriptor::new("point forest composite",&BindGroupLayoutEntries::sequential(
        ShaderStages::FRAGMENT,(uniform_buffer::<FrameUniform>(false),storage_buffer_read_only_sized(false,None))));
    let shader=assets.load("shaders/point_forest.wgsl");
    let pipeline=|entry:&'static str|cache.queue_compute_pipeline(ComputePipelineDescriptor{
        label:Some(entry.into()),layout:vec![compute.clone()],shader:shader.clone(),entry_point:Some(entry.into()),
        zero_initialize_workgroup_memory:true,..default()});
    let depth=pipeline("depth_cs");let owner=pipeline("owner_cs");let resolve=pipeline("resolve_cs");
    let draw=cache.queue_render_pipeline(RenderPipelineDescriptor{
        label:Some("point forest depth composite".into()),layout:vec![composite.clone()],
        vertex:fullscreen.to_vertex_state(),
        fragment:Some(FragmentState{shader:assets.load("shaders/point_forest_composite.wgsl"),entry_point:Some("fragment".into()),
            shader_defs:vec![],constants:vec![],targets:vec![Some(ColorTargetState{format:TextureFormat::Rgba16Float,blend:None,write_mask:ColorWrites::ALL})]}),
        depth_stencil:Some(DepthStencilState{format:CORE_3D_DEPTH_FORMAT,depth_write_enabled:Some(true),
            depth_compare:Some(CompareFunction::GreaterEqual),stencil:default(),bias:default()}),
        ..default()
    });
    commands.insert_resource(Pipelines{compute,composite,depth,owner,resolve,draw});
}
#[derive(Default)]
struct GpuFrame {
    uploaded:Option<Arc<Vec<UVec4>>>,
    points:StorageBuffer<PointArray>,trees:StorageBuffer<TreeArray>,jobs:StorageBuffer<JobArray>,
    uniform:UniformBuffer<FrameUniform>,screen:Option<(u64,Buffer)>,
}
fn render_points(
    view:ViewQuery<(&PointCamera,&ExtractedView,&ViewTarget,&ViewDepthStencilTexture,&Msaa)>,
    forest:Option<Res<ExtractedForest>>,pipelines:Res<Pipelines>,cache:Res<PipelineCache>,
    device:Res<RenderDevice>,queue:Res<RenderQueue>,mut state:Local<GpuFrame>,mut ctx:RenderContext,
) {
    let Some(forest)=forest else{return;};
    let (camera,view,target,depth_texture,msaa)=view.into_inner();
    let (Some(depth),Some(owner),Some(resolve),Some(draw))=(cache.get_compute_pipeline(pipelines.depth),
        cache.get_compute_pipeline(pipelines.owner),cache.get_compute_pipeline(pipelines.resolve),
        cache.get_render_pipeline(pipelines.draw)) else {forest.ready.store(false,Ordering::Release);return;};
    let v=view.viewport;
    let bytes=u64::from(v.z)*u64::from(v.w)*12;
    if msaa.samples()!=1||target.main_texture_format()!=TextureFormat::Rgba16Float||
        v.z==0||v.w==0||bytes>u64::from(device.limits().max_storage_buffer_binding_size) {
        forest.ready.store(false,Ordering::Release);return;
    }
    forest.ready.store(true,Ordering::Release);
    if forest.jobs.is_empty(){return;}
    if state.uploaded.as_ref().is_none_or(|old|!Arc::ptr_eq(old,&forest.geometry)){
        state.points.set(PointArray{data:(*forest.geometry).clone()});
        state.points.write_buffer(&device,&queue);
        state.uploaded=Some(forest.geometry.clone());
    }
    state.trees.set(TreeArray{data:forest.trees.clone()});state.trees.write_buffer(&device,&queue);
    state.jobs.set(JobArray{data:forest.jobs.clone()});state.jobs.write_buffer(&device,&queue);
    let count=forest.jobs.len() as u32;
    state.uniform.set(FrameUniform{clip:camera.clip,viewport:v,params:UVec4::new(count,JOB_WIDTH,0,0),
        sun_time:camera.sun.extend(camera.elapsed),sunlight:camera.sunlight.extend(0.0)});
    state.uniform.write_buffer(&device,&queue);
    if state.screen.as_ref().is_none_or(|(size,_)|*size!=bytes){
        state.screen=Some((bytes,device.create_buffer(&BufferDescriptor{label:Some("point depth owner color"),
            size:bytes,usage:BufferUsages::STORAGE|BufferUsages::COPY_DST,mapped_at_creation:false})));
    }
    let screen=&state.screen.as_ref().unwrap().1;
    let uniform=state.uniform.binding().unwrap();
    let group=device.create_bind_group("point forest compute",&cache.get_bind_group_layout(&pipelines.compute),
        &BindGroupEntries::sequential((uniform.clone(),state.points.binding().unwrap(),state.trees.binding().unwrap(),
            state.jobs.binding().unwrap(),screen.as_entire_binding())));
    let composite=device.create_bind_group("point forest composite",&cache.get_bind_group_layout(&pipelines.composite),
        &BindGroupEntries::sequential((uniform,screen.as_entire_binding())));
    let encoder=ctx.command_encoder();
    encoder.clear_buffer(screen,0,None);
    for (label,pipeline,x,y) in [
        ("point depth",depth,count.min(JOB_WIDTH),count.div_ceil(JOB_WIDTH)),
        ("point owner",owner,count.min(JOB_WIDTH),count.div_ceil(JOB_WIDTH)),
        ("point resolve",resolve,v.z.div_ceil(8),v.w.div_ceil(8)),
    ] {
        let mut pass=encoder.begin_compute_pass(&ComputePassDescriptor{label:Some(label),..default()});
        pass.set_pipeline(pipeline);pass.set_bind_group(0,&group,&[]);pass.dispatch_workgroups(x,y,1);
    }
    let attachments=[Some(target.get_color_attachment())];
    let mut pass=encoder.begin_render_pass(&RenderPassDescriptor{label:Some("point forest depth composite"),
        color_attachments:&attachments,depth_stencil_attachment:Some(depth_texture.get_attachment(StoreOp::Store)),..default()});
    pass.set_viewport(v.x as f32,v.y as f32,v.z as f32,v.w as f32,0.0,1.0);
    pass.set_pipeline(draw);pass.set_bind_group(0,&composite,&[]);pass.draw(0..3,0..1);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn packed_records_are_16_bytes_and_signed() {
        let s=Sample{position:Vec3::new(-1.0,2.0,3.0),radius:0.1,color:Vec3::ONE,normal:Vec3::Y,leaf:true};
        let p=packed(s);
        assert_eq!(std::mem::size_of::<UVec4>(),16);
        assert_eq!(p.x as u16 as i16,-256);
        assert_eq!((p.x>>16) as i16,512);
        assert_eq!(p.w>>16,1);
        assert_eq!(aggregate(&[s,s],0.25).len(),1);
    }
}
