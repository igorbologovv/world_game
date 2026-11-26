//! Experimental bridge for the standalone Igor direct-compute renderer.
//! No second device/window: use Bevy's RenderDevice and real scene depth.
use std::{collections::{BTreeMap, HashMap}, sync::{Arc, atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering}}};
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
    ForestModels, Species, lod::{VegetationSettings, View}, hero_oak::HeroOakAssets, tree_assets::TreeAssets};

use bevy::tasks::{block_on,poll_once,AsyncComputeTaskPool,Task};
use igor_renderer::{planetary::{Pack,Template,RAW},splat::{planetary_level,PlanetAggregate}};

const BUDGET:u32=igor_renderer::render_api::DEFAULT_POINT_BUDGET as u32;
const JOB_WIDTH:u32=igor_renderer::render_api::JOB_WIDTH;
pub(super) fn enabled()->bool{std::env::var("IGOR_POINT_FOREST").as_deref()==Ok("1")}
#[derive(Clone,Copy)]
struct GpuTree{origin_scale:Vec4,rotation:Vec4,range:UVec4}
impl GpuTree{
    fn words(self)->[u32;12]{
        let a=self.origin_scale.to_array().map(f32::to_bits);
        let b=self.rotation.to_array().map(f32::to_bits);
        let c=self.range.to_array();
        [a[0],a[1],a[2],a[3],b[0],b[1],b[2],b[3],c[0],c[1],c[2],c[3]]
    }
}
#[derive(Resource)]
pub(super) struct PointForest{
    pack:Option<Arc<Pack>>,pending:Option<Task<Result<Pack,String>>>,
    requested:Vec<usize>,next_build:f64,error:Option<String>,
    ready:Arc<AtomicBool>,status:Arc<AtomicU32>,submitted:Arc<AtomicU32>,
    storage_limit:Arc<AtomicU64>,layer_limit:Arc<AtomicU32>,
    candidates:u32,eligible:u32,clusters:u32,trees:Vec<GpuTree>,jobs:Vec<UVec4>,selected:u32,textured:u32,budget:u32,
    threshold:f32,history:HashMap<[u64;3],u8>,spare:u32,
}
impl Default for PointForest{
    fn default()->Self{Self{pack:None,pending:None,requested:Vec::new(),next_build:0.0,error:None,
        ready:Arc::new(AtomicBool::new(false)),status:Arc::new(AtomicU32::new(0)),submitted:Arc::new(AtomicU32::new(0)),
        storage_limit:Arc::new(AtomicU64::new(0)),layer_limit:Arc::new(AtomicU32::new(0)),
        candidates:0,eligible:0,clusters:0,trees:Vec::new(),jobs:Vec::new(),selected:0,textured:0,
        budget:std::env::var("IGOR_POINT_BUDGET").ok().and_then(|v|v.parse().ok()).unwrap_or(BUDGET).clamp(1,8_000_000),
        threshold:2.2,history:HashMap::new(),spare:0}}
}
fn site_key(s:Site)->[u64;3]{s.position.to_array().map(f64::to_bits)}
fn tree_origin(template:&Template,site:Site,eye:DVec3)->(Vec3,Quat,f32){
    let rotation=site.rotation();let scale=site.scale/template.transform.scale;
    let root=Vec3::from_array(template.transform.root);
    let origin=(site.position-eye).as_vec3()+rotation*(root*site.scale-Vec3::new(8.0,0.0,8.0)*scale);
    (origin,rotation,scale)
}
fn choose(template:&Template,site:Site,depth:f32,distance:f32,focal:f32,threshold:f32,prev:u8)->usize{
    let scale=site.scale/template.transform.scale;
    let radius=template.info.bound*scale;
    // At the edge of a conservative sphere, an almost-zero depth must
    // not force the entire high-poly template to raw indefinitely.
    let ppm=focal*scale/(depth-radius).max(depth*0.25).max(0.25);
    let stagger=0.94+0.12*(site.yaw/std::f32::consts::TAU).clamp(0.0,1.0);
    let weight=0.18+0.82*((distance-radius).max(0.0)/160.0).clamp(0.0,1.0);
    planetary_level(ppm,(threshold*weight*stagger).max(0.25),prev)
}
// Stable world-space cells, using the original demo's 16m and 4x
// hierarchy. Only collapse well-separated distant trees.
fn world_cell(site:Site,depth:f32,distance:f32,focal:f32,threshold:f32)->Option<(i32,i64,i64,i64)>{
    if distance<160.0||depth<=0.0{return None;}
    for width in [1024,256,64,16]{
        let w=width as f64;
        if depth>width as f32*2.0 && focal*width as f32/(depth-width as f32)<=threshold*2.0{
            let p=site.position/w;
            return Some((width,p.x.floor() as i64,p.y.floor() as i64,p.z.floor() as i64));
        }
    }
    None
}
fn select_points(mut points:ResMut<PointForest>,forest:Res<ForestStream>,models:Res<ForestModels>,
    settings:Res<VegetationSettings>,hero:Res<HeroOakAssets>,assets:Res<TreeAssets>,time:Res<Time>,
    cameras:Query<(Entity,GridTransform,&Camera,&Projection),With<WorldCamera>>,frames:ReferenceFrames){
    points.trees.clear();points.jobs.clear();points.selected=0;points.textured=0;points.candidates=0;points.eligible=0;points.clusters=0;
    if let Some(result)=points.pending.as_mut().and_then(|task|block_on(poll_once(task))){
        points.pending=None;
        match result{Ok(pack)=>{
            info!("Original Igor pack: {} templates, {} raw, {} LOD, {} patches, {} atlas pages",pack.templates.len(),pack.raw.len(),pack.lod.len(),pack.patches.len(),pack.atlas.layers);
            points.pack=Some(Arc::new(pack));points.history.clear();points.error=None;
        },Err(err)=>{error!("Original Igor pack: {err}");points.error=Some(err);}}
    }
    if !points.ready.load(Ordering::Acquire)||hero.is_pending()||assets.is_pending(){return;}
    let Ok((entity,grid,camera,projection))=cameras.single() else{return;};
    let Some(frame)=frames.parent_frame(entity) else{return;};
    let eye=frame.grid_position_double(&grid.cell,&grid.transform);
    let view=View::new(camera,projection);let Projection::Perspective(perspective)=projection else{return;};
    let aspect=camera.physical_viewport_size().map_or(1.78,|s|s.x as f32/s.y.max(1) as f32);
    let ty=(perspective.fov*0.5).tan();let tx=ty*aspect;
    let inverse=grid.transform.rotation.conjugate();let reach=settings.proxy_range().y;
    let mut visible=Vec::new();let mut needed=BTreeMap::<usize,(Species,usize)>::new();
    for site in forest.point_sites(){
        points.candidates+=1;
        let delta=(site.position-eye).as_vec3();let distance=delta.length();let radius=site.height().max(5.0)*1.25;
        if distance>reach+radius{continue;}
        let v=inverse*delta;let depth=-v.z;
        if depth+radius<0.05||v.x.abs()>depth.max(0.0)*tx+radius*(1.0+tx)||v.y.abs()>depth.max(0.0)*ty+radius*(1.0+ty){continue;}
        if !super::within_horizon(eye,site.position,f64::from(radius)){continue;}
        let variant=models.portraits.point_variant(site.species,site.variant);
        let tile=impostor::tile(site.species,variant);needed.insert(tile,(site.species,variant));
        visible.push((distance,depth,site,tile));
    }
    visible.sort_unstable_by(|a,b|a.0.total_cmp(&b.0).then_with(||site_key(a.2).cmp(&site_key(b.2))));
    points.eligible=visible.len() as u32;
    let now=time.elapsed_secs_f64();
    let missing=needed.keys().any(|k|points.pack.as_ref().is_none_or(|p|!p.templates.contains_key(k)));
    if missing&&points.pending.is_none()&&now>=points.next_build&&!needed.is_empty(){
        let keys:Vec<_>=needed.keys().copied().collect();
        if keys!=points.requested{
            let input:Vec<_>=needed.into_values().collect();let portraits=models.portraits.clone();
            let limit=points.storage_limit.load(Ordering::Acquire);let layers=points.layer_limit.load(Ordering::Acquire);
            points.requested=keys;points.next_build=now+3.0;points.error=None;
            points.pending=Some(AsyncComputeTaskPool::get().spawn(async move{
                let inputs=input.into_iter().map(|(species,variant)|portraits.planet_input(species,variant)).collect();
                Pack::build(inputs,limit,layers)
            }));
        }
    }
    let Some(pack)=points.pack.clone() else{return;};
    visible.retain(|v|pack.templates.contains_key(&v.3));
    // Evaluate the actual model centre in view space, not its ground anchor.
    for (distance,depth,site,tile) in &mut visible{
        let t=&pack.templates[tile];let (origin,rotation,scale)=tree_origin(t,*site,eye);
        let center=origin+rotation*(Vec3::from_array(t.info.center)*scale);
        *distance=center.length();*depth=-(inverse*center).z;
    }
    visible.sort_unstable_by(|a,b|a.0.total_cmp(&b.0).then_with(||site_key(a.2).cmp(&site_key(b.2))));
    let budget=u64::from(points.budget);
    const MAX_JOBS:u64=180_000;
    let cost=|t:f32,history:&HashMap<[u64;3],u8>|visible.iter().fold((0u64,0u64),|(n,j),&(d,z,s,k)|{
        if world_cell(s,z,d,view.focal,t).is_some(){return (n+1,j+1);}
        let template=&pack.templates[&k];let level=choose(template,s,z,d,view.focal,t,*history.get(&site_key(s)).unwrap_or(&255));
        let r=template.info.ranges[level];(n+u64::from(r.count),j+u64::from(r.count.div_ceil(64)))
    });
    let fits=|(n,j):(u64,u64)|n<=budget&&j<=MAX_JOBS;
    let mut threshold=points.threshold.max(2.2);
    for _ in 0..40{if fits(cost(threshold,&points.history)){break;}threshold=(threshold*1.15).min(512.0);}
    let c=cost(threshold,&points.history);
    if c.0<=budget*75/100{points.spare+=1;}else{points.spare=0;}
    if points.spare>=12{
        let refined=(threshold/1.08).max(2.2);
        if fits(cost(refined,&points.history)){threshold=refined;}points.spare=0;
    }
    points.threshold=threshold;
    let old=std::mem::take(&mut points.history);
    let mut groups=BTreeMap::<(i32,i64,i64,i64),PlanetAggregate>::new();
    for (distance,depth,site,tile) in visible{
        let template=&pack.templates[&tile];
        let (relative,rotation,scale)=tree_origin(template,site,eye);
        if let Some(cell)=world_cell(site,depth,distance,view.focal,threshold){
            let mut sample=template.info.summary;
            if sample.coverage>0.0{
                sample.position=(relative+rotation*(Vec3::from_array(sample.position)*scale)).to_array();
                sample.normal=(rotation*Vec3::from_array(sample.normal)).to_array();
                sample.radius*=scale;
                groups.entry(cell).or_default().add(sample);
            }
            continue;
        }
        let key=site_key(site);
        let level=choose(template,site,depth,distance,view.focal,threshold,*old.get(&key).unwrap_or(&255));
        let r=template.info.ranges[level];
        if r.count==0||points.selected.saturating_add(r.count)>points.budget||points.jobs.len() as u64+u64::from(r.count.div_ceil(64))>MAX_JOBS{continue;}
        let upper=((1.0-super::lod::smoothstep(reach*0.79,reach,distance))*12.0).round().clamp(0.0,12.0) as u32;
        if upper==0{continue;}
        let index=points.trees.len() as u32;
        points.trees.push(GpuTree{origin_scale:relative.extend(scale),rotation:Vec4::from_array(rotation.to_array()),
            range:UVec4::new(r.offset,r.count,0,site.yaw.to_bits())});
        for first in (0..r.count).step_by(64){points.jobs.push(UVec4::new(index,first,(r.count-first).min(64),upper));}
        points.history.insert(key,level as u8);points.selected+=r.count;if r.offset&RAW!=0{points.textured+=r.count;}
    }
    for (_,aggregate) in groups{
        if points.selected>=points.budget||points.jobs.len() as u64>=MAX_JOBS{break;}
        let Some((position,radius,rgba,normal))=aggregate.finish()else{continue;};
        let index=points.trees.len() as u32;
        // The original world record is a unit disk with world radius in
        // the instance. Carry its color/normal directly in the instance
        // instead of rewriting the static LOD buffer each frame.
        points.trees.push(GpuTree{origin_scale:Vec3::from_array(position).extend(radius),
            rotation:Vec4::W,range:UVec4::new(rgba,normal,1,0)});
        points.jobs.push(UVec4::new(index,0,1,12));points.selected+=1;points.clusters+=1;
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
    // Shader assets live in the main world; Bevy extracts them into
    // the render pipeline cache. Keep a strong handle in the render world.
    if app.get_sub_app(RenderApp).is_none(){return;}
    let shader=app.world_mut().resource_mut::<Assets<Shader>>()
        .add(Shader::from_wgsl(igor_renderer::planetary::compute_shader(),"igor_renderer/planetary.wgsl"));
    if let Some(render)=app.get_sub_app_mut(RenderApp) {
        render.insert_resource(PointShader(shader));
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
    let msg=format!("IGOR LIB | {status} | {} instances ({} clusters) / {} points ({} UV) | {} visible / {} sites | {} templates | LOD {:.2}px | {} GPU trees | {}",
        forest.trees.len(),forest.clusters,forest.selected,forest.textured,forest.eligible,forest.candidates,forest.pack.as_ref().map_or(0,|p|p.templates.len()),forest.threshold,forest.submitted.load(Ordering::Acquire),
        if forest.pending.is_some(){"baking original geometry"}else{forest.error.as_deref().unwrap_or("")});
    if let Ok(mut text)=overlay.single_mut(){text.0=msg;}
}
fn single_sample_camera(mut cameras:Query<&mut Msaa,With<WorldCamera>>) {
    for mut msaa in &mut cameras {if *msaa!=Msaa::Off{*msaa=Msaa::Off;}}
}


#[derive(Resource)]
struct ExtractedForest{
    pack:Option<Arc<Pack>>,ready:Arc<AtomicBool>,status:Arc<AtomicU32>,submitted:Arc<AtomicU32>,
    storage_limit:Arc<AtomicU64>,layer_limit:Arc<AtomicU32>,trees:Vec<GpuTree>,jobs:Vec<UVec4>,
}
#[derive(Component)]
struct PointCamera{forward:Vec3,right:Vec3,up:Vec3,sun:Vec3,sunlight:Vec3,elapsed:f32,projection:Mat4,far:f32}
fn extract_frame(mut commands:Commands,forest:Extract<Res<PointForest>>,
    cameras:Extract<Query<(RenderEntity,&Transform,&Projection,&Exposure),With<WorldCamera>>>,
    lights:Extract<Query<(&DirectionalLight,&Transform)>>,time:Extract<Res<Time>>){
    commands.insert_resource(ExtractedForest{pack:forest.pack.clone(),ready:forest.ready.clone(),status:forest.status.clone(),
        submitted:forest.submitted.clone(),storage_limit:forest.storage_limit.clone(),layer_limit:forest.layer_limit.clone(),
        trees:forest.trees.clone(),jobs:forest.jobs.clone()});
    let sun=lights.iter().max_by(|a,b|a.0.illuminance.total_cmp(&b.0.illuminance));let mut perspective=false;
    for (entity,transform,projection,exposure) in &*cameras{
        let Projection::Perspective(p)=projection else{commands.entity(entity).remove::<PointCamera>();continue;};
        perspective=true;
        let (direction,light)=sun.map_or((Vec3::Y,Vec3::ONE),|(light,transform)|
            (transform.back().as_vec3(),light.color.to_linear().to_vec3()*light.illuminance*exposure.exposure()));
        commands.entity(entity).insert(PointCamera{forward:transform.rotation*Vec3::NEG_Z,right:transform.rotation*Vec3::X,
            up:transform.rotation*Vec3::Y,sun:direction,sunlight:light,elapsed:time.elapsed_secs(),
            projection:projection.get_clip_from_view(),far:p.far});
    }
    if !perspective{forest.ready.store(false,Ordering::Release);forest.status.store(0,Ordering::Release);forest.submitted.store(0,Ordering::Release);}
}
#[derive(Clone,Default,ShaderType)]
struct FrameUniform{
    origin:Vec4,forward:Vec4,right:Vec4,up:Vec4,screen:Vec4,extent:Vec4,params:UVec4,effects:Vec4,
    sun:Vec4,sunlight:Vec4,depth_coeff:Vec4,viewport:UVec4,
}
#[derive(Resource)]
struct Pipelines{compute:BindGroupLayoutDescriptor,composite:BindGroupLayoutDescriptor,
    clear:CachedComputePipelineId,depth:CachedComputePipelineId,owner:CachedComputePipelineId,resolve:CachedComputePipelineId,draw:CachedRenderPipelineId}
#[derive(Resource)]
struct PointShader(Handle<Shader>);
fn init_pipelines(mut commands:Commands,assets:Res<AssetServer>,shader:Res<PointShader>,cache:Res<PipelineCache>,fullscreen:Res<FullscreenShader>){
    let compute=BindGroupLayoutDescriptor::new("original Igor compute",&BindGroupLayoutEntries::sequential(
        ShaderStages::COMPUTE,(uniform_buffer::<FrameUniform>(false),
        storage_buffer_read_only_sized(false,None),storage_buffer_read_only_sized(false,None),storage_buffer_read_only_sized(false,None),
        storage_buffer_read_only_sized(false,None),storage_buffer_read_only_sized(false,None),
        texture_2d_array(TextureSampleType::Float{filterable:true}),texture_2d_array(TextureSampleType::Float{filterable:true}),
        sampler(SamplerBindingType::Filtering),storage_buffer_read_only_sized(false,None),
        storage_buffer_sized(false,None),storage_buffer_sized(false,None))));
    let composite=BindGroupLayoutDescriptor::new("original Igor composite",&BindGroupLayoutEntries::sequential(
        ShaderStages::FRAGMENT,(uniform_buffer::<FrameUniform>(false),storage_buffer_read_only_sized(false,None))));
    let shader=shader.0.clone();
    let pipeline=|entry:&'static str|cache.queue_compute_pipeline(ComputePipelineDescriptor{label:Some(entry.into()),
        layout:vec![compute.clone()],shader:shader.clone(),entry_point:Some(entry.into()),zero_initialize_workgroup_memory:true,..default()});
    let clear=pipeline("clear_cs");let depth=pipeline("depth_cs");let owner=pipeline("owner_cs");let resolve=pipeline("resolve_cs");
    let draw=cache.queue_render_pipeline(RenderPipelineDescriptor{label:Some("original Igor depth composite".into()),layout:vec![composite.clone()],
        vertex:fullscreen.to_vertex_state(),fragment:Some(FragmentState{shader:assets.load("shaders/point_forest_composite.wgsl"),
            entry_point:Some("fragment".into()),shader_defs:vec![],constants:vec![],
            targets:vec![Some(ColorTargetState{format:TextureFormat::Rgba16Float,blend:None,write_mask:ColorWrites::ALL})]}),
        depth_stencil:Some(DepthStencilState{format:CORE_3D_DEPTH_FORMAT,depth_write_enabled:Some(true),depth_compare:Some(CompareFunction::GreaterEqual),stencil:default(),bias:default()}),..default()});
    commands.insert_resource(Pipelines{compute,composite,clear,depth,owner,resolve,draw});
}
struct GpuPack{source:Arc<Pack>,raw:RawBufferVec<[u32;4]>,lod:RawBufferVec<[u32;4]>,
    patches:RawBufferVec<[u32;32]>,materials:RawBufferVec<[u32;16]>,_texture:Texture,color:TextureView,normal:TextureView,sampler:Sampler}
impl GpuPack{
    fn upload(pack:Arc<Pack>,device:&RenderDevice,queue:&RenderQueue)->Self{
        let mut raw=RawBufferVec::new(BufferUsages::STORAGE);raw.extend(pack.raw.iter().copied());if pack.raw.is_empty(){raw.push([0;4]);}raw.write_buffer(device,queue);
        let mut lod=RawBufferVec::new(BufferUsages::STORAGE);lod.extend(pack.lod.iter().copied());if pack.lod.is_empty(){lod.push([0;4]);}lod.write_buffer(device,queue);
        let mut patches=RawBufferVec::new(BufferUsages::STORAGE);patches.extend(pack.patches.iter().copied());if pack.patches.is_empty(){patches.push([0;32]);}patches.write_buffer(device,queue);
        let mut materials=RawBufferVec::new(BufferUsages::STORAGE);materials.extend(pack.materials.iter().copied());if pack.materials.is_empty(){materials.push([0;16]);}materials.write_buffer(device,queue);
        let atlas=&pack.atlas;
        let texture=device.create_texture(&TextureDescriptor{label:Some("original Igor texture atlas"),size:Extent3d{width:atlas.side,height:atlas.side,depth_or_array_layers:atlas.layers},
            mip_level_count:1,sample_count:1,dimension:TextureDimension::D2,format:TextureFormat::Rgba8Unorm,
            usage:TextureUsages::TEXTURE_BINDING|TextureUsages::COPY_DST,view_formats:&[TextureFormat::Rgba8UnormSrgb]});
        for t in &atlas.tiles{
            queue.write_texture(TexelCopyTextureInfo{texture:&texture,mip_level:0,origin:Origin3d{x:t.origin[0],y:t.origin[1],z:t.origin[2]},aspect:TextureAspect::All},
                &t.rgba,TexelCopyBufferLayout{offset:0,bytes_per_row:Some(t.size[0]*4),rows_per_image:None},
                Extent3d{width:t.size[0],height:t.size[1],depth_or_array_layers:1});
        }
        let make=|format|texture.create_view(&TextureViewDescriptor{format:Some(format),dimension:Some(TextureViewDimension::D2Array),..default()});
        let color=make(TextureFormat::Rgba8UnormSrgb);let normal=make(TextureFormat::Rgba8Unorm);
        let sampler=device.create_sampler(&SamplerDescriptor{min_filter:FilterMode::Linear,mag_filter:FilterMode::Linear,..default()});
        Self{source:pack,raw,lod,patches,materials,_texture:texture,color,normal,sampler}
    }
}
struct GpuFrame{pack:Option<GpuPack>,trees:RawBufferVec<[u32;12]>,jobs:RawBufferVec<[u32;4]>,
    uniform:UniformBuffer<FrameUniform>,screen:Option<(u64,Buffer)>,projection:Option<(u64,Buffer)>}
impl Default for GpuFrame{
    fn default()->Self{Self{pack:None,trees:RawBufferVec::new(BufferUsages::STORAGE),jobs:RawBufferVec::new(BufferUsages::STORAGE),
        uniform:UniformBuffer::default(),screen:None,projection:None}}
}
fn render_points(view:ViewQuery<(&PointCamera,&ExtractedView,&ViewTarget,&ViewDepthStencilTexture,&Msaa)>,
    forest:Option<Res<ExtractedForest>>,pipelines:Res<Pipelines>,cache:Res<PipelineCache>,
    device:Res<RenderDevice>,queue:Res<RenderQueue>,mut state:Local<GpuFrame>,mut ctx:RenderContext){
    let Some(forest)=forest else{return;};let (camera,view,target,depth_texture,msaa)=view.into_inner();
    let limits=device.limits();
    forest.storage_limit.store(u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size),Ordering::Release);
    forest.layer_limit.store(limits.max_texture_array_layers,Ordering::Release);
    let (Some(clear),Some(depth),Some(owner),Some(resolve),Some(draw))=(cache.get_compute_pipeline(pipelines.clear),
        cache.get_compute_pipeline(pipelines.depth),cache.get_compute_pipeline(pipelines.owner),cache.get_compute_pipeline(pipelines.resolve),
        cache.get_render_pipeline(pipelines.draw))else{
        forest.ready.store(false,Ordering::Release);forest.submitted.store(0,Ordering::Release);
        let states=[cache.get_compute_pipeline_state(pipelines.clear),cache.get_compute_pipeline_state(pipelines.depth),
            cache.get_compute_pipeline_state(pipelines.owner),cache.get_compute_pipeline_state(pipelines.resolve),cache.get_render_pipeline_state(pipelines.draw)];
        let failed=states.iter().any(|s|matches!(s,CachedPipelineState::Err(_)));let status=if failed{2}else{1};
        if forest.status.swap(status,Ordering::AcqRel)!=status&&failed{for s in states{if let CachedPipelineState::Err(e)=s{error!("Original Igor pipeline: {e:?}");}}}
        return;
    };
    let v=view.viewport;let bytes=u64::from(v.z)*u64::from(v.w)*12;
    if msaa.samples()!=1||target.main_texture_format()!=TextureFormat::Rgba16Float||v.z==0||v.w==0||
        bytes>u64::from(limits.max_storage_buffer_binding_size)||limits.max_storage_buffers_per_shader_stage<8{
        forest.ready.store(false,Ordering::Release);forest.submitted.store(0,Ordering::Release);
        if forest.status.swap(3,Ordering::AcqRel)!=3{warn!("Original Igor unsupported view/limits: {limits:?}");}return;
    }
    forest.ready.store(true,Ordering::Release);forest.status.store(4,Ordering::Release);
    let Some(pack)=forest.pack.as_ref()else{return;};
    if forest.jobs.is_empty(){forest.submitted.store(0,Ordering::Release);return;}
    if state.pack.as_ref().is_none_or(|old|!Arc::ptr_eq(&old.source,pack)){state.pack=Some(GpuPack::upload(pack.clone(),&device,&queue));}
    state.trees.clear();state.trees.extend(forest.trees.iter().map(|t|t.words()));state.trees.write_buffer(&device,&queue);
    state.jobs.clear();state.jobs.extend(forest.jobs.iter().map(|j|j.to_array()));state.jobs.write_buffer(&device,&queue);
    let count=forest.jobs.len() as u32;let pr=camera.projection;
    state.uniform.set(FrameUniform{origin:Vec4::ZERO,forward:camera.forward.extend(0.0),right:camera.right.extend(0.0),up:camera.up.extend(0.0),
        screen:Vec4::new(v.z as f32,v.w as f32,2.0/(v.w as f32*pr.y_axis.y.abs()),23.0),
        extent:Vec4::new(0.0,0.0,0.0,camera.far.max(1.0)),params:UVec4::new(count,0,u32::from(std::env::var("IGOR_POINT_DEBUG").as_deref()==Ok("1")),0),
        effects:Vec4::new(camera.elapsed,1.0,0.0,0.0),sun:camera.sun.extend(0.0),sunlight:camera.sunlight.extend(0.0),
        depth_coeff:Vec4::new(pr.z_axis.z,pr.w_axis.z,pr.z_axis.w,pr.w_axis.w),viewport:v});
    state.uniform.write_buffer(&device,&queue);
    if state.screen.as_ref().is_none_or(|(size,_)|*size!=bytes){state.screen=Some((bytes,device.create_buffer(&BufferDescriptor{label:Some("original Igor depth owner color"),
        size:bytes,usage:BufferUsages::STORAGE,mapped_at_creation:false})));}
    let projection_bytes=u64::from(count)*64*8;
    if projection_bytes>u64::from(limits.max_storage_buffer_binding_size){forest.submitted.store(0,Ordering::Release);return;}
    if state.projection.as_ref().is_none_or(|(size,_)|*size<projection_bytes){
        let size=projection_bytes.next_power_of_two().min(u64::from(limits.max_storage_buffer_binding_size));
        state.projection=Some((size,device.create_buffer(&BufferDescriptor{label:Some("original Igor projection cache"),size,usage:BufferUsages::STORAGE,mapped_at_creation:false})));
    }
    let gpu=state.pack.as_ref().unwrap();let screen=&state.screen.as_ref().unwrap().1;let projection=&state.projection.as_ref().unwrap().1;
    let uniform=state.uniform.binding().unwrap();
    let group=device.create_bind_group("original Igor compute",&cache.get_bind_group_layout(&pipelines.compute),
        &BindGroupEntries::sequential((uniform.clone(),gpu.raw.binding().unwrap(),gpu.lod.binding().unwrap(),state.trees.binding().unwrap(),
        gpu.patches.binding().unwrap(),gpu.materials.binding().unwrap(),&gpu.color,&gpu.normal,&gpu.sampler,
        state.jobs.binding().unwrap(),screen.as_entire_binding(),projection.as_entire_binding())));
    let composite=device.create_bind_group("original Igor composite",&cache.get_bind_group_layout(&pipelines.composite),
        &BindGroupEntries::sequential((uniform,screen.as_entire_binding())));
    let encoder=ctx.command_encoder();
    for (label,pipeline,x,y) in [("clear",clear,v.z.div_ceil(8),v.w.div_ceil(8)),
        ("depth",depth,count.min(JOB_WIDTH),count.div_ceil(JOB_WIDTH)),("owner",owner,count.min(JOB_WIDTH),count.div_ceil(JOB_WIDTH)),
        ("resolve",resolve,v.z.div_ceil(8),v.w.div_ceil(8))]{
        let mut pass=encoder.begin_compute_pass(&ComputePassDescriptor{label:Some(label),..default()});
        pass.set_pipeline(pipeline);pass.set_bind_group(0,&group,&[]);pass.dispatch_workgroups(x,y,1);
    }
    let attachments=[Some(target.get_color_attachment())];
    let mut pass=encoder.begin_render_pass(&RenderPassDescriptor{label:Some("original Igor depth composite"),color_attachments:&attachments,
        depth_stencil_attachment:Some(depth_texture.get_attachment(StoreOp::Store)),..default()});
    pass.set_viewport(v.x as f32,v.y as f32,v.z as f32,v.w as f32,0.0,1.0);
    pass.set_pipeline(draw);pass.set_bind_group(0,&composite,&[]);pass.draw(0..3,0..1);
    forest.submitted.store(forest.trees.len() as u32,Ordering::Release);
}
