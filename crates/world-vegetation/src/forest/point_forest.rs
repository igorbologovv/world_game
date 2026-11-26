//! Experimental bridge for the standalone Igor direct-compute renderer.
//! No second device/window: use Bevy's RenderDevice and real scene depth.
use std::{collections::{BTreeMap, HashMap}, sync::{Arc, atomic::{AtomicBool, AtomicU32, Ordering}}};
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
const WIDTHS: [f32;22] = [0.0,0.0625,0.09375,0.125,0.1875,0.25,0.375,0.5,0.625,0.75,0.875,1.0,1.25,1.5,1.75,2.0,2.5,3.0,3.5,4.0,8.0,16.0];
pub(super) fn enabled()->bool {std::env::var("IGOR_POINT_FOREST").as_deref()==Ok("1")}
const MAX_TEMPLATE_RECORDS:usize=MAX_RECORDS/(super::library::ASSETS.len()+13);

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
#[derive(Clone, Copy)]
struct GpuTree { origin_scale:Vec4, rotation:Vec4, range:UVec4 }
impl GpuTree {
    // Exact WGSL std430 layout: two vec4<f32> followed by vec4<u32>.
    // Use raw words rather than runtime-array ShaderType derive.
    fn words(self) -> [u32;12] {
        let a=self.origin_scale.to_array().map(f32::to_bits);
        let b=self.rotation.to_array().map(f32::to_bits);
        let c=self.range.to_array();
        [a[0],a[1],a[2],a[3],b[0],b[1],b[2],b[3],c[0],c[1],c[2],c[3]]
    }
}
#[derive(Resource)]
pub(super) struct PointForest {
    geometry:Arc<Vec<UVec4>>,
    templates:HashMap<usize,[Range;WIDTHS.len()]>,
    ready:Arc<AtomicBool>,
    status:Arc<AtomicU32>,
    submitted:Arc<AtomicU32>,
    rejected:HashMap<usize, ()>,
    pub candidates:u32,
    pub eligible:u32,
    trees:Vec<GpuTree>,
    jobs:Vec<UVec4>,
    selected:u32,
    baked_this_frame:bool,
    budget:u32,
}
impl Default for PointForest {
    fn default() -> Self {
        Self{geometry:Arc::new(Vec::new()),templates:HashMap::new(),ready:Arc::new(AtomicBool::new(false)),
            status:Arc::new(AtomicU32::new(0)),submitted:Arc::new(AtomicU32::new(0)),rejected:HashMap::new(),candidates:0,eligible:0,
            trees:Vec::new(),jobs:Vec::new(),selected:0,baked_this_frame:false,
            budget:std::env::var("IGOR_POINT_BUDGET").ok().and_then(|v|v.parse().ok()).unwrap_or(BUDGET).clamp(1,8_000_000)}
    }
}
impl PointForest {
    pub(super) fn begin_frame(&mut self) {
        self.trees.clear();self.jobs.clear();self.selected=0;self.baked_this_frame=false;self.candidates=0;self.eligible=0;
    }
    pub(super) fn add_tree(&mut self, portraits:&Portraits, site:Site, eye:DVec3, distance:f32, focal:f32, upper:u8) -> bool {
        if !self.ready.load(Ordering::Acquire) { return false; }
        let variant=portraits.point_variant(site.species,site.variant);
        let tile=impostor::tile(site.species,variant);
        if !self.templates.contains_key(&tile) {
            if self.baked_this_frame || self.rejected.contains_key(&tile) { return false; }
            let Some(samples)=portraits.point_samples(site.species,variant) else {return false;};
            self.baked_this_frame=true;
            if samples.is_empty() || samples.iter().any(|s|s.position.abs().max_element()>=127.0) {
                self.rejected.insert(tile,()); warn!("Point forest: invalid sample template {tile}");return false;
            }
            // Preserve full-detail samples, then allocate coarse ranges first.
            // Fine ranges may alias a resident coarser range to bound VRAM.
            let raw:Vec<_>=samples.iter().copied().map(packed).collect();
            let cap=MAX_TEMPLATE_RECORDS.min(MAX_RECORDS.saturating_sub(self.geometry.len()));
            if raw.len()>=cap {self.rejected.insert(tile,());warn!("Point forest: template {tile} exceeds resident budget");return false;}
            let storage=Arc::make_mut(&mut self.geometry);
            let start=storage.len();
            let mut ranges=[Range::default();WIDTHS.len()];
            let raw_range=Range{start:start as u32,count:raw.len() as u32};
            storage.extend(raw);
            let mut next=Range::default();
            for i in (1..WIDTHS.len()).rev(){
                let level=aggregate(&samples,WIDTHS[i]);
                let fits=storage.len()-start+level.len()<=cap;
                let useful=next.count==0 || level.len() as u32>next.count.saturating_mul(11)/10;
                if !level.is_empty() && level.len()<raw_range.count as usize && fits && useful{
                    next=Range{start:storage.len() as u32,count:level.len() as u32};
                    storage.extend(level);
                }
                ranges[i]=if next.count==0{raw_range}else{next};
            }
            ranges[0]=raw_range;
            let count=storage.len()-start;
            self.templates.insert(tile,ranges);
            info!("Point forest: baked template {tile}, {count} shared records, total {}",storage.len());
        }
        let levels=&self.templates[&tile];
        let target_width=2.2*(distance-site.height()*0.35).max(0.1)/(focal.max(1.0)*site.scale.max(0.1));
        let preferred=WIDTHS.iter().rposition(|&w|w<=target_width).unwrap_or(0);
        // Keep the true sample level when its immediate neighbour was
        // aliased far down the hierarchy.
        let preferred=if distance<25.0{0}else{preferred};
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
            self.jobs.push(UVec4::new(index,first,(range.count-first).min(64),u32::from(upper)));
        }
        self.selected+=range.count;
        true
    }
}
pub(super) fn install(app:&mut App) {
    if !enabled(){return;}
    info!("Igor point forest enabled; counters at lower right (IGOR_POINT_DEBUG=1 highlights compute pixels)");
    app.init_resource::<PointForest>()
        .add_systems(Startup,spawn_overlay)
        .add_systems(PostUpdate,update_overlay.after(super::lod::update_lods));
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
#[derive(Component)]
struct PointOverlay;
fn spawn_overlay(mut commands:Commands){
    commands.spawn((PointOverlay,Text::new("IGOR | initializing"),TextFont{font_size:FontSize::Px(15.0),..default()},
        TextColor(Color::srgb(0.9,1.0,0.8)),BackgroundColor(Color::srgba(0.02,0.03,0.02,0.85)),
        Node{position_type:PositionType::Absolute,right:Val::Px(14.0),bottom:Val::Px(14.0),
            padding:UiRect::all(Val::Px(6.0)),..default()}));
}
fn update_overlay(forest:Res<PointForest>,time:Res<Time>,mut elapsed:Local<f32>,mut overlay:Query<&mut Text,With<PointOverlay>>){
    *elapsed+=time.delta_secs();if *elapsed<0.5{return;}*elapsed=0.0;
    let status=match forest.status.load(Ordering::Acquire){
        0=>"waiting for perspective render view",1=>"compiling pipelines",2=>"pipeline error (see terminal)",
        3=>"unsupported view/limits",4=>"ready",_=>"unknown",
    };
    let msg=format!("IGOR | {status} | {} trees / {} points | {} eligible / {} detailed | {} templates | {} GPU trees submitted",
        forest.trees.len(),forest.selected,forest.eligible,forest.candidates,forest.templates.len(),forest.submitted.load(Ordering::Acquire));
    if let Ok(mut text)=overlay.single_mut(){text.0=msg;}
}
fn single_sample_camera(mut cameras:Query<&mut Msaa,With<WorldCamera>>) {
    for mut msaa in &mut cameras {if *msaa!=Msaa::Off{*msaa=Msaa::Off;}}
}

#[derive(Resource)]
struct ExtractedForest {
    geometry:Arc<Vec<UVec4>>, ready:Arc<AtomicBool>,status:Arc<AtomicU32>,submitted:Arc<AtomicU32>,
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
    commands.insert_resource(ExtractedForest{geometry:forest.geometry.clone(),ready:forest.ready.clone(),status:forest.status.clone(),submitted:forest.submitted.clone(),
        trees:forest.trees.clone(),jobs:forest.jobs.clone()});
    let sun=lights.iter().max_by(|a,b|a.0.illuminance.total_cmp(&b.0.illuminance));
    let mut perspective=false;
    for (entity,transform,projection,exposure) in &*cameras {
        if !matches!(projection,Projection::Perspective(_)) {
            commands.entity(entity).remove::<PointCamera>();continue;
        }
        perspective=true;
        let (direction,light)=sun.map_or((Vec3::Y,Vec3::ONE),|(light,transform)|
            (transform.back().as_vec3(),light.color.to_linear().to_vec3()*light.illuminance*exposure.exposure()));
        commands.entity(entity).insert(PointCamera{
            clip:projection.get_clip_from_view()*Mat4::from_quat(transform.rotation.conjugate()),
            sun:direction,sunlight:light,elapsed:time.elapsed_secs(),
        });
    }
    if !perspective {forest.ready.store(false,Ordering::Release);forest.status.store(0,Ordering::Release);forest.submitted.store(0,Ordering::Release);}
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
            storage_buffer_read_only_sized(false,None),storage_buffer_sized(false,None),storage_buffer_sized(false,None))));
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
struct GpuFrame {
    uploaded:Option<Arc<Vec<UVec4>>>,
    points:RawBufferVec<[u32;4]>,trees:RawBufferVec<[u32;12]>,jobs:RawBufferVec<[u32;4]>,
    uniform:UniformBuffer<FrameUniform>,screen:Option<(u64,Buffer)>,projection:Option<(u64,Buffer)>,
}
impl Default for GpuFrame {
    fn default() -> Self {
        Self{uploaded:None,
            points:RawBufferVec::new(BufferUsages::STORAGE),
            trees:RawBufferVec::new(BufferUsages::STORAGE),
            jobs:RawBufferVec::new(BufferUsages::STORAGE),
            uniform:UniformBuffer::default(),screen:None,projection:None}
    }
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
        cache.get_render_pipeline(pipelines.draw)) else {
        forest.ready.store(false,Ordering::Release);
        forest.submitted.store(0,Ordering::Release);
        let states=[cache.get_compute_pipeline_state(pipelines.depth),cache.get_compute_pipeline_state(pipelines.owner),
            cache.get_compute_pipeline_state(pipelines.resolve),cache.get_render_pipeline_state(pipelines.draw)];
        let failed=states.iter().any(|s|matches!(s,CachedPipelineState::Err(_)));
        let status=if failed{2}else{1};
        if forest.status.swap(status,Ordering::AcqRel)!=status && failed{
            for s in states {if let CachedPipelineState::Err(err)=s {error!("Point forest pipeline: {err:?}");}}
        }
        return;
    };
    let v=view.viewport;
    let bytes=u64::from(v.z)*u64::from(v.w)*12;
    if msaa.samples()!=1||target.main_texture_format()!=TextureFormat::Rgba16Float||
        v.z==0||v.w==0||bytes>u64::from(device.limits().max_storage_buffer_binding_size) {
        forest.ready.store(false,Ordering::Release);forest.submitted.store(0,Ordering::Release);
        if forest.status.swap(3,Ordering::AcqRel)!=3 {
            warn!("Point forest unsupported view: format={:?}, MSAA={}, viewport={v:?}, screen bytes={bytes}, storage limit={}",target.main_texture_format(),msaa.samples(),device.limits().max_storage_buffer_binding_size);
        }
        return;
    }
    forest.ready.store(true,Ordering::Release);
    if forest.status.swap(4,Ordering::AcqRel)!=4 {info!("Point forest GPU pipelines ready");}
    if forest.jobs.is_empty(){forest.submitted.store(0,Ordering::Release);return;}
    if state.uploaded.as_ref().is_none_or(|old|!Arc::ptr_eq(old,&forest.geometry)){
        state.points.clear();
        state.points.extend(forest.geometry.iter().map(|p|p.to_array()));
        state.points.write_buffer(&device,&queue);
        state.uploaded=Some(forest.geometry.clone());
    }
    state.trees.clear();
    state.trees.extend(forest.trees.iter().map(|tree|tree.words()));
    state.trees.write_buffer(&device,&queue);
    state.jobs.clear();
    state.jobs.extend(forest.jobs.iter().map(|job|job.to_array()));
    state.jobs.write_buffer(&device,&queue);
    let count=forest.jobs.len() as u32;
    state.uniform.set(FrameUniform{clip:camera.clip,viewport:v,params:UVec4::new(count,JOB_WIDTH,u32::from(std::env::var("IGOR_POINT_DEBUG").as_deref()==Ok("1")),0),
        sun_time:camera.sun.extend(camera.elapsed),sunlight:camera.sunlight.extend(0.0)});
    state.uniform.write_buffer(&device,&queue);
    if state.screen.as_ref().is_none_or(|(size,_)|*size!=bytes){
        state.screen=Some((bytes,device.create_buffer(&BufferDescriptor{label:Some("point depth owner color"),
            size:bytes,usage:BufferUsages::STORAGE|BufferUsages::COPY_DST,mapped_at_creation:false})));
    }
    // Two u32 per dispatched point slot, including workgroup padding.
    let projection_bytes=u64::from(count)*64*8;
    if projection_bytes>u64::from(device.limits().max_storage_buffer_binding_size) {
        forest.ready.store(false,Ordering::Release);forest.submitted.store(0,Ordering::Release);return;
    }
    if state.projection.as_ref().is_none_or(|(size,_)|*size<projection_bytes){
        let size=projection_bytes.next_power_of_two().min(u64::from(device.limits().max_storage_buffer_binding_size));
        state.projection=Some((size,device.create_buffer(&BufferDescriptor{label:Some("point projection cache"),size,
            usage:BufferUsages::STORAGE,mapped_at_creation:false})));
    }
    let projection=&state.projection.as_ref().unwrap().1;
    let screen=&state.screen.as_ref().unwrap().1;
    let uniform=state.uniform.binding().unwrap();
    let group=device.create_bind_group("point forest compute",&cache.get_bind_group_layout(&pipelines.compute),
        &BindGroupEntries::sequential((uniform.clone(),state.points.binding().unwrap(),state.trees.binding().unwrap(),
            state.jobs.binding().unwrap(),screen.as_entire_binding(),projection.as_entire_binding())));
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
    forest.submitted.store(forest.trees.len() as u32,Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn tree_words_match_wgsl_layout() {
        let t=GpuTree{origin_scale:Vec4::new(1.0,2.0,3.0,4.0),rotation:Vec4::W,range:UVec4::new(5,6,7,8)};
        let words=t.words();
        assert_eq!(std::mem::size_of_val(&words),48);
        assert_eq!(&words[..4], &[1.0f32.to_bits(),2.0f32.to_bits(),3.0f32.to_bits(),4.0f32.to_bits()]);
        assert_eq!(&words[4..8], &[0,0,0,1.0f32.to_bits()]);
        assert_eq!(&words[8..], &[5,6,7,8]);
    }
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
