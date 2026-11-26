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
use world_core::runtime::{WorldCamera, GridTransform, ReferenceFrames};
use super::{impostor::{self, Portraits}, placement::Site, streaming::ForestStream,
    ForestModels, lod::{VegetationSettings, View}, hero_oak::HeroOakAssets, tree_assets::TreeAssets};

const BUDGET: u32 = 3_500_000;
const MAX_RECORDS: usize = 7_500_000;
const MAX_PATCHES:usize=900_000;
pub(super) const ATLAS_SIDE:u32=512;
const MAX_LAYERS:usize=256;
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
    pub patch:u32,
    pub covered:bool,
}
// Each source patch is 8 vec4s (128 bytes), identical to the WGSL layout.
pub(super) struct PointBake {pub samples:Vec<Sample>,pub patches:Vec<[u32;32]>,pub layers:Vec<Arc<Vec<u8>>>}
pub(super) fn raw_patch(positions:[Vec3;3],normals:[Vec3;3],uvs:[Vec2;3],tint:Vec4,layer:u32,normal_layer:Option<u32>,repeat:bool,cutoff:f32,tangent:Vec4)->Option<[u32;32]>{
    let e=positions[1]-positions[0];let f=positions[2]-positions[0];
    let cross=e.cross(f);let det=cross.length_squared();if det<1e-16{return None;}
    let n=cross.normalize();let gu=f.cross(cross)/det;let gv=cross.cross(e)/det;
    let du=uvs[1]-uvs[0];let dv=uvs[2]-uvs[0];
    let enc=|v:Vec3|{let [x,y]=impostor::encode_normal(v.normalize_or_zero());u32::from(x)|(u32::from(y)<<8)};
    let flags=u32::from(repeat)|(((cutoff.clamp(0.0,1.0)*255.0).round() as u32)<<8)|(normal_layer.map_or(0,|x|x+1)<<16);
    let a=n.extend(n.dot(positions[0])).to_array().map(f32::to_bits);
    let b=positions[0].to_array().map(f32::to_bits);let c=gu.to_array().map(f32::to_bits);let d=gv.to_array().map(f32::to_bits);
    let t=tint.to_array().map(f32::to_bits);let k=tangent.to_array().map(f32::to_bits);
    Some([a[0],a[1],a[2],a[3],b[0],b[1],b[2],layer,c[0],c[1],c[2],enc(normals[0]),d[0],d[1],d[2],enc(normals[1]),
        uvs[0].x.to_bits(),uvs[0].y.to_bits(),du.x.to_bits(),du.y.to_bits(),dv.x.to_bits(),dv.y.to_bits(),enc(normals[2]),flags,
        t[0],t[1],t[2],t[3],k[0],k[1],k[2],k[3]])
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
        if !s.covered {continue;}
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
            radius:a.area.sqrt().min(width*0.86),leaf,patch:0,covered:true,
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
    patches:Arc<Vec<[u32;32]>>,
    layers:Arc<Vec<Arc<Vec<u8>>>>,
    textured:u32,
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
    threshold:f32,
}
impl Default for PointForest {
    fn default() -> Self {
        Self{geometry:Arc::new(Vec::new()),patches:Arc::new(Vec::new()),layers:Arc::new(Vec::new()),textured:0,templates:HashMap::new(),ready:Arc::new(AtomicBool::new(false)),
            status:Arc::new(AtomicU32::new(0)),submitted:Arc::new(AtomicU32::new(0)),rejected:HashMap::new(),candidates:0,eligible:0,
            trees:Vec::new(),jobs:Vec::new(),selected:0,baked_this_frame:false,
            budget:std::env::var("IGOR_POINT_BUDGET").ok().and_then(|v|v.parse().ok()).unwrap_or(BUDGET).clamp(1,8_000_000),threshold:2.2}
    }
}
impl PointForest {
    pub(super) fn begin_frame(&mut self) {
        self.trees.clear();self.jobs.clear();self.selected=0;self.textured=0;self.baked_this_frame=false;self.candidates=0;self.eligible=0;
    }
    fn ensure_template(&mut self, portraits:&Portraits, site:Site) -> Option<usize> {
        let variant=portraits.point_variant(site.species,site.variant);
        let tile=impostor::tile(site.species,variant);
        if !self.templates.contains_key(&tile) {
            if self.baked_this_frame || self.rejected.contains_key(&tile) { return None; }
            let Some(bake)=portraits.point_samples(site.species,variant) else {return None;};
            let PointBake{samples,mut patches,layers}=bake;
            self.baked_this_frame=true;
            if samples.is_empty() || samples.iter().any(|s|s.position.abs().max_element()>=127.0) {
                self.rejected.insert(tile,()); warn!("Point forest: invalid sample template {tile}");return None;
            }
            // Preserve full-detail samples, then allocate coarse ranges first.
            // Fine ranges may alias a resident coarser range to bound VRAM.
            let detailed=self.patches.len()+patches.len()<=MAX_PATCHES && self.layers.len()+layers.len()<=MAX_LAYERS;
            let patch_base=self.patches.len() as u32;
            let layer_base=self.layers.len() as u32;
            let raw:Vec<_>=samples.iter().copied().filter(|s|detailed||s.covered).map(|s|{
                let mut p=packed(s);
                if detailed {p.z=patch_base+s.patch;p.w|=1<<17;}
                p
            }).collect();
            let cap=MAX_TEMPLATE_RECORDS.min(MAX_RECORDS.saturating_sub(self.geometry.len()));
            if raw.len()>=cap {self.rejected.insert(tile,());warn!("Point forest: template {tile} exceeds resident budget");return None;}
            if detailed {
                for patch in &mut patches {
                    patch[7]+=layer_base;
                    let normal=patch[23]>>16;
                    if normal!=0{patch[23]=(patch[23]&65535)|((normal+layer_base)<<16);}
                }
                Arc::make_mut(&mut self.patches).extend(patches);
                Arc::make_mut(&mut self.layers).extend(layers);
            }
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
        Some(tile)
    }
    fn pick_range(&self,tile:usize,site:Site,distance:f32,focal:f32,threshold:f32)->Range {
        let levels=&self.templates[&tile];
        // Keep close texture-bearing geometry; use increasingly coarse
        // samples farther away to reserve the global budget for the foreground.
        let far=1.0+2.0*super::lod::smoothstep(70.0,450.0,distance);
        let near=0.52+0.48*super::lod::smoothstep(18.0,65.0,distance);
        let target=threshold*far*near*(distance-site.height()*0.35).max(0.1)/(focal.max(1.0)*site.scale.max(0.1));
        let level=WIDTHS.iter().rposition(|&w|w<=target).unwrap_or(0);
        levels[level]
    }
    fn push_tree(&mut self,site:Site,eye:DVec3,range:Range,upper:u8)->bool {
        if range.count==0 || self.selected.saturating_add(range.count)>self.budget{return false;}
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
        if self.geometry[range.start as usize].w&(1<<17)!=0{self.textured+=range.count;}
        true
    }
    // Kept for the disabled legacy LOD code to compile; pure mode instead
    // selects globally across the complete visible site set.
    pub(super) fn add_tree(&mut self,portraits:&Portraits,site:Site,eye:DVec3,distance:f32,focal:f32,upper:u8)->bool{
        if !self.ready.load(Ordering::Acquire){return false;}
        let Some(tile)=self.ensure_template(portraits,site) else{return false;};
        let range=self.pick_range(tile,site,distance,focal,self.threshold);
        self.push_tree(site,eye,range,upper)
    }
}
fn select_points(
    mut points:ResMut<PointForest>,forest:Res<ForestStream>,models:Res<ForestModels>,
    settings:Res<VegetationSettings>,hero:Res<HeroOakAssets>,assets:Res<TreeAssets>,
    cameras:Query<(Entity,GridTransform,&Camera,&Projection),With<WorldCamera>>,
    frames:ReferenceFrames,
) {
    points.begin_frame();
    if !points.ready.load(Ordering::Acquire)||hero.is_pending()||assets.is_pending(){return;}
    let Ok((entity,grid,camera,projection))=cameras.single() else{return;};
    let Some(frame)=frames.parent_frame(entity) else{return;};
    let eye=frame.grid_position_double(&grid.cell,&grid.transform);
    let view=View::new(camera,projection);
    if view.orthographic{return;}
    let Projection::Perspective(perspective)=projection else{return;};
    let aspect=camera.physical_viewport_size().map_or(1.78,|s|s.x as f32/s.y.max(1) as f32);
    let ty=(perspective.fov*0.5).tan();
    let tx=ty*aspect;
    let inverse=grid.transform.rotation.conjugate();
    let reach=settings.proxy_range().y;
    let mut visible=Vec::new();
    for site in forest.point_sites(){
        points.candidates+=1;
        let delta=(site.position-eye).as_vec3();
        let distance=delta.length();
        let radius=site.height().max(5.0)*1.25;
        if distance>reach+radius {continue;}
        // Conservative tree bounds in camera space. Never cull a visible
        // crown just because its base lies outside the viewport.
        let v=inverse*delta;
        let z=-v.z;
        if z+radius<0.05 || v.x.abs()>z.max(0.0)*tx+radius*(1.0+tx)
            || v.y.abs()>z.max(0.0)*ty+radius*(1.0+ty) {continue;}
        if !super::within_horizon(eye,site.position,f64::from(radius)){continue;}
        visible.push((distance,site));
    }
    visible.sort_unstable_by(|a,b|a.0.total_cmp(&b.0)
        .then_with(||a.1.position.x.total_cmp(&b.1.position.x))
        .then_with(||a.1.position.y.total_cmp(&b.1.position.y))
        .then_with(||a.1.position.z.total_cmp(&b.1.position.z)));
    points.eligible=visible.len() as u32;
    let mut choices=Vec::with_capacity(visible.len());
    for (distance,site) in visible{
        if let Some(tile)=points.ensure_template(&models.portraits,site){choices.push((distance,site,tile));}
    }
    // Choose a global screen-space threshold before admitting any tree.
    // This avoids exhausting the budget on nearby models and dropping the
    // whole distant forest. Also account for padded workgroup slots.
    let cost=|threshold:f32,points:&PointForest|->(u64,u64){
        choices.iter().fold((0,0),|(n,j),&(d,s,t)|{
            let r=points.pick_range(t,s,d,view.focal,threshold);
            (n+u64::from(r.count),j+u64::from(r.count.div_ceil(64)))
        })
    };
    let mut threshold=points.threshold.max(2.2);
    let budget=u64::from(points.budget);
    const MAX_JOBS:u64=180_000;
    let fits=|(n,j):(u64,u64)|n<=budget&&j<=MAX_JOBS;
    for _ in 0..28{
        if fits(cost(threshold,&points)){break;}
        threshold*=1.25;
    }
    let refined=(threshold/1.10).max(2.2);
    if fits(cost(refined,&points)){threshold=refined;}
    points.threshold=threshold;
    for (distance,site,tile) in choices{
        let r=points.pick_range(tile,site,distance,view.focal,threshold);
        if points.jobs.len() as u64+u64::from(r.count.div_ceil(64))>MAX_JOBS{continue;}
        // At the outer edge let the planet's continuous canopy take over.
        let upper=((1.0-super::lod::smoothstep(reach*0.79,reach,distance))*12.0).round().clamp(0.0,12.0) as u8;
        if upper>0{points.push_tree(site,eye,r,upper);}
    }
}
pub(super) fn install(app:&mut App) {
    if !enabled(){return;}
    info!("Igor point forest enabled; counters at lower right (IGOR_POINT_DEBUG=1 highlights compute pixels)");
    app.init_resource::<PointForest>()
        .add_systems(Startup,spawn_overlay)
        .add_systems(PostUpdate,(select_points,update_overlay).chain().after(super::lod::update_lods));
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
    let msg=format!("IGOR ONLY | {status} | {} trees / {} points ({} UV) | {} visible / {} sites | {} templates | LOD {:.2}px | {} GPU trees",
        forest.trees.len(),forest.selected,forest.textured,forest.eligible,forest.candidates,forest.templates.len(),forest.threshold,forest.submitted.load(Ordering::Acquire));
    if let Ok(mut text)=overlay.single_mut(){text.0=msg;}
}
fn single_sample_camera(mut cameras:Query<&mut Msaa,With<WorldCamera>>) {
    for mut msaa in &mut cameras {if *msaa!=Msaa::Off{*msaa=Msaa::Off;}}
}

#[derive(Resource)]
struct ExtractedForest {
    geometry:Arc<Vec<UVec4>>,patches:Arc<Vec<[u32;32]>>,layers:Arc<Vec<Arc<Vec<u8>>>>, ready:Arc<AtomicBool>,status:Arc<AtomicU32>,submitted:Arc<AtomicU32>,
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
    commands.insert_resource(ExtractedForest{geometry:forest.geometry.clone(),patches:forest.patches.clone(),layers:forest.layers.clone(),ready:forest.ready.clone(),status:forest.status.clone(),submitted:forest.submitted.clone(),
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
            storage_buffer_read_only_sized(false,None),storage_buffer_sized(false,None),storage_buffer_sized(false,None),
            storage_buffer_read_only_sized(false,None),texture_2d_array(TextureSampleType::Float{filterable:true}),sampler(SamplerBindingType::Filtering))));
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
    points:RawBufferVec<[u32;4]>,patches:RawBufferVec<[u32;32]>,atlas:Option<(u32,usize,Texture,TextureView,Sampler)>,trees:RawBufferVec<[u32;12]>,jobs:RawBufferVec<[u32;4]>,
    uniform:UniformBuffer<FrameUniform>,screen:Option<(u64,Buffer)>,projection:Option<(u64,Buffer)>,
}
impl Default for GpuFrame {
    fn default() -> Self {
        Self{uploaded:None,
            points:RawBufferVec::new(BufferUsages::STORAGE),patches:RawBufferVec::new(BufferUsages::STORAGE),atlas:None,
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
        state.patches.clear();
        state.patches.extend(forest.patches.iter().copied());
        if state.patches.is_empty(){state.patches.push([0;32]);}
        state.patches.write_buffer(&device,&queue);
        state.uploaded=Some(forest.geometry.clone());
    }
    // Grow the texture array geometrically, upload only newly baked layers.
    let layer_count=forest.layers.len();
    let capacity=(layer_count.max(1) as u32).next_power_of_two();
    if state.atlas.as_ref().is_none_or(|(cap,_,_,_,_)|*cap<capacity){
        let texture=device.create_texture(&TextureDescriptor{label:Some("point source texture array"),
            size:Extent3d{width:ATLAS_SIDE,height:ATLAS_SIDE,depth_or_array_layers:capacity},
            mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,format:TextureFormat::Rgba8Unorm,
            usage:TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_DST,view_formats:&[]});
        let view=texture.create_view(&TextureViewDescriptor{dimension:Some(TextureViewDimension::D2Array),..default()});
        let sampler=device.create_sampler(&SamplerDescriptor{min_filter:FilterMode::Linear,mag_filter:FilterMode::Linear,..default()});
        state.atlas=Some((capacity,0,texture,view,sampler));
    }
    let (_,uploaded,texture,_,_)=state.atlas.as_mut().unwrap();
    for (i,bytes) in forest.layers.iter().enumerate().skip(*uploaded){
        queue.write_texture(TexelCopyTextureInfo{texture, mip_level:0,origin:Origin3d{x:0,y:0,z:i as u32},aspect:TextureAspect::All},
            bytes,TexelCopyBufferLayout{offset:0,bytes_per_row:Some(ATLAS_SIDE*4),rows_per_image:Some(ATLAS_SIDE)},
            Extent3d{width:ATLAS_SIDE,height:ATLAS_SIDE,depth_or_array_layers:1});
    }
    *uploaded=layer_count;
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
    let (_,_,_,atlas,sampler)=state.atlas.as_ref().unwrap();
    let group=device.create_bind_group("point forest compute",&cache.get_bind_group_layout(&pipelines.compute),
        &BindGroupEntries::sequential((uniform.clone(),state.points.binding().unwrap(),state.trees.binding().unwrap(),
            state.jobs.binding().unwrap(),screen.as_entire_binding(),projection.as_entire_binding(),state.patches.binding().unwrap(),atlas,sampler)));
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
    #[test] fn patch_barycentric_layout(){
        let p=raw_patch([Vec3::ZERO,Vec3::X,Vec3::Y],[Vec3::Z;3],[Vec2::ZERO,Vec2::X,Vec2::Y],Vec4::ONE,3,Some(4),true,0.5,Vec4::X).unwrap();
        assert_eq!(std::mem::size_of_val(&p),128);
        assert_eq!(p[7],3);
        assert_eq!(f32::from_bits(p[8]),1.0);
        assert_eq!(f32::from_bits(p[13]),1.0);
        assert_eq!(p[23]>>16,5);
    }
    #[test] fn packed_records_are_16_bytes_and_signed() {
        let s=Sample{position:Vec3::new(-1.0,2.0,3.0),radius:0.1,color:Vec3::ONE,normal:Vec3::Y,leaf:true,patch:0,covered:true};
        let p=packed(s);
        assert_eq!(std::mem::size_of::<UVec4>(),16);
        assert_eq!(p.x as u16 as i16,-256);
        assert_eq!((p.x>>16) as i16,512);
        assert_eq!(p.w>>16,1);
        assert_eq!(aggregate(&[s,s],0.25).len(),1);
    }
}
