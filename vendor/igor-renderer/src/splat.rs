//! Flat point hierarchy. No ray intersection is used to construct or select it.
use crate::{Scene,CELL,random,rotate,trees::pack_normal};
use bytemuck::{Pod,Zeroable};
use glam::{Vec3,UVec3};
use std::collections::{HashMap,BTreeMap};
#[path="planetary_selection.rs"]
pub mod planetary_selection;
const BRICKS:usize=64;
const BRICK_WIDTH:f32=4.0;
const BRICK_MAX:f32=2.0;
const LEVELS:usize=22;
const RAW:u32=0x8000_0000;
// Units are template-local meters. Densify the mid-field only; keep
// the original near levels and distant 4/8/16 m hierarchy.
const LOCAL_WIDTHS:[f32;LEVELS]=[0.0,0.0625,0.09375,0.125,0.1875,0.25,0.375,0.5,0.625,0.75,0.875,1.0,1.25,1.5,1.75,2.0,2.5,3.0,3.5,4.0,8.0,16.0];
const ORIGINAL_LEVELS:[usize;8]=[0,5,7,11,15,19,20,21];
const DENSE_LEVELS:[usize;15]=[0,1,2,3,4,5,6,7,9,11,13,15,19,20,21];
const EXTRA_LEVELS:[usize;7]=[1,2,3,4,6,9,13];
const MID_LEVELS:[usize;7]=[8,10,12,14,16,17,18];

#[repr(C)]
#[derive(Clone,Copy,Debug,Pod,Zeroable)]
pub struct Point {pub xy:u32,pub zr:u32,pub rgba:u32,pub normal:u32}
impl Point {
    fn encode(p:Vec3,r:f32,c:Vec3,n:Vec3,coverage:f32)->Self {
        let q=(p.clamp(Vec3::ZERO,Vec3::splat(15.9997))*4096.0).round().as_uvec3();
        let r=(r*4096.0).ceil().clamp(1.0,65535.0) as u32;
        let rgb=c.clamp(Vec3::ZERO,Vec3::ONE).to_array().map(|v|(v*255.0+0.5) as u32);
        let alpha=(coverage.clamp(0.0,1.0)*255.0+0.5) as u32;
        Self{xy:q.x|(q.y<<16),zr:q.z|(r<<16),rgba:rgb[0]|(rgb[1]<<8)|(rgb[2]<<16)|(alpha<<24),normal:pack_normal(n.normalize_or(Vec3::Y))}
    }
}
#[derive(Clone,Copy,Debug,Default)]
struct Aggregate {weighted_pos:Vec3,weighted_color:Vec3,weighted_normal:Vec3,area:f32,lo:Vec3,hi:Vec3}
#[derive(Clone,Copy,Debug)]
struct Sample {pos:Vec3,color:Vec3,normal:Vec3,radius:f32,coverage:f32}
impl Aggregate {
    fn add(&mut self,s:Sample) {
        let area=(s.radius*s.radius*s.coverage).max(1e-8);
        let lo=s.pos-Vec3::splat(s.radius);
        let hi=s.pos+Vec3::splat(s.radius);
        if self.area==0.0 {self.lo=lo;self.hi=hi;}else{self.lo=self.lo.min(lo);self.hi=self.hi.max(hi);}
        self.weighted_pos+=s.pos*area;self.weighted_color+=s.color*area;
        // Opposite-facing surfaces can cancel; normalize only after aggregation.
        self.weighted_normal+=s.normal*area;self.area+=area;
    }
    fn sample(self)->Option<Sample> {
        if self.area==0.0{return None;}
        let p=self.weighted_pos/self.area;
        let extent=(p-self.lo).max(self.hi-p);
        let bound=extent.length().max(0.001);
        // A representative disk is not an opaque cube. Keep an estimate of
        // covered area separately; radius/coverage are approximate at each LOD.
        let radius=bound.min(self.area.sqrt().max(extent.max_element()*0.45)).max(0.001);
        Some(Sample{pos:p,color:self.weighted_color/self.area,normal:self.weighted_normal.normalize_or(Vec3::Y),radius,coverage:(self.area/(radius*radius)).clamp(0.02,1.0)})
    }
}
#[derive(Clone,Copy,Debug,Default)]
pub struct Range {pub offset:u32,pub count:u32}
#[derive(Clone,Copy)]
struct Brick {ranges:[Range;LEVELS],center:Vec3,radius:f32}
impl Default for Brick{
    fn default()->Self{Self{ranges:[Range::default();LEVELS],center:Vec3::ZERO,radius:0.0}}
}
struct Template {ranges:[Range;LEVELS],radii:[f32;LEVELS],lo:Vec3,hi:Vec3,summary:Sample,bricks:Vec<Brick>}
// Partition in place: whole-template ranges remain valid. Aliased LOD
// ranges reuse the same partition and never get sorted twice.
fn point_position(p:&Point)->Vec3{
    Vec3::new((p.xy&65535) as f32,(p.xy>>16) as f32,(p.zr&65535) as f32)/4096.0
}
fn point_radius(p:&Point)->f32{(p.zr>>16) as f32/4096.0}
fn brick_index(p:&Point)->usize{cell_index(point_position(p),BRICK_WIDTH,4)}
fn partition_bricks(raw:&mut [Point],lod:&mut [Point],ranges:&[Range;LEVELS],levels:&[usize])->Vec<Brick>{
    let mut bricks=vec![Brick::default();BRICKS];
    for (i,b) in bricks.iter_mut().enumerate(){
        b.center=Vec3::new((i%4) as f32,(i/4%4) as f32,(i/16) as f32)*BRICK_WIDTH+Vec3::splat(BRICK_WIDTH*0.5);
    }
    let mut partitions=HashMap::<(u32,u32),[Range;BRICKS]>::new();
    for &level in levels{
        if LOCAL_WIDTHS[level]>BRICK_MAX{break;}
        let range=ranges[level];
        let key=(range.offset,range.count);
        let parts=if let Some(parts)=partitions.get(&key){*parts}else{
            let data=if range.offset&RAW!=0{&mut *raw}else{&mut *lod};
            let start=(range.offset&!RAW) as usize;
            let data=&mut data[start..start+range.count as usize];
            data.sort_by_key(brick_index);
            let mut parts=[Range::default();BRICKS];
            let mut i=0;
            while i<data.len(){
                let b=brick_index(&data[i]);let first=i;
                while i<data.len()&&brick_index(&data[i])==b{
                    let p=&data[i];
                    bricks[b].radius=bricks[b].radius.max((point_position(p)-bricks[b].center).length()+point_radius(p)+0.01);
                    i+=1;
                }
                parts[b]=Range{offset:range.offset+first as u32,count:(i-first) as u32};
            }
            partitions.insert(key,parts);parts
        };
        for (b,range) in bricks.iter_mut().zip(parts){b.ranges[level]=range;}
    }
    bricks
}
#[repr(C)]
#[derive(Clone,Copy,Debug,Pod,Zeroable)]
pub struct Instance {pub origin_scale:[f32;4],pub offset:u32,pub turn:u32,pub pad:[u32;2],pub bound:[f32;4]}
impl Instance {
    fn new(origin:Vec3,scale:f32,offset:u32,turn:u32)->Self {Self{origin_scale:origin.extend(scale).to_array(),offset,turn,pad:[0;2],bound:[0.0;4]}}
    fn world(self,p:Vec3)->Vec3 {Vec3::from_slice(&self.origin_scale)+rotate(p,self.turn)*self.origin_scale[3]}
}
#[derive(Clone,Copy)]
struct Site {trees:[Instance;2],models:[usize;2]}
#[derive(Clone,Copy)]
struct WorldMeta {sample:Sample,offset:u32}
struct WorldLevel {side:u32,width:f32,cells:Vec<Option<WorldMeta>>}
/// Static directional shadow field, indexed by projection onto y=0.
/// RGBA channels represent four world-height bands, not visible voxels.
pub struct ShadowField {pub width:u32,pub height:u32,pub pixels:Vec<u8>,pub transform:[f32;4]}
const SHADOW_PAD:f32=24.0;
fn shadow_projection(p:Vec3)->(f32,f32){(p.x+p.y*0.625,p.z+p.y*0.5)}
fn add_shadow_sample(pixels:&mut [u8],w:u32,h:u32,step:f32,p:Vec3,radius:f32,coverage:f32){
    if p.y<=0.12||coverage<=0.0{return;}
    let (x,z)=shadow_projection(p);
    raster_shadow_sample(pixels,w,h,step,x+SHADOW_PAD,z+SHADOW_PAD,p.y/4.0,radius,coverage);
}
// Same opacity and soft footprint for planar and spherical hosts.
fn raster_shadow_sample(pixels:&mut [u8],w:u32,h:u32,step:f32,x:f32,z:f32,band_height:f32,radius:f32,coverage:f32){
    let x=x/step-0.5;let z=z/step-0.5;
    let r=(radius*1.35/step).max(0.03);
    let span=r+0.75;
    let x0=((x-span).floor() as i32).max(0);
    let z0=((z-span).floor() as i32).max(0);
    let x1=((x+span).ceil() as i32).min(w as i32-1);
    let z1=((z+span).ceil() as i32).min(h as i32-1);
    let band=band_height.floor().clamp(0.0,3.0) as usize;
    // Area-aware soft footprint, accumulated as approximate opacity.
    let opacity=(coverage*r*r*0.80).min(0.80);
    for iz in z0..=z1{for ix in x0..=x1{
        let d=((ix as f32-x).powi(2)+(iz as f32-z).powi(2)).sqrt();
        let a=(1.0-d/span).clamp(0.0,1.0)*opacity;
        if a<=0.0{continue;}
        let i=(iz as usize*w as usize+ix as usize)*4+band;
        let old=f32::from(pixels[i])/255.0;
        pixels[i]=((old+(1.0-old)*a)*255.0+0.5) as u8;
    }}
}
/// Shared templates from the exact demo builder. Range offsets refer to
/// raw or lod according to render_api::RAW_RANGE_BIT.
pub struct TemplateLibrary{
    pub raw:Vec<Point>,pub lod:Vec<Point>,templates:Vec<Template>,levels:Vec<usize>,
}
impl TemplateLibrary{
    pub fn len(&self)->usize{self.templates.len()}
    pub fn is_empty(&self)->bool{self.templates.is_empty()}
    pub fn levels(&self)->&[usize]{&self.levels}
    pub fn width(level:usize)->f32{LOCAL_WIDTHS[level]}
    pub fn range(&self,model:usize,level:usize)->Range{self.templates[model].ranges[level]}
    pub fn bounds(&self,model:usize)->(Vec3,Vec3){let t=&self.templates[model];(t.lo,t.hi)}
    pub fn radius(&self,model:usize,level:usize)->f32{self.templates[model].radii[level]}
    pub fn choose_level(&self,px_per_m:f32,threshold:f32,previous:Option<usize>)->usize{
        choose_local(px_per_m,threshold,previous.map_or(UNKNOWN,|p|p as u8),&self.levels)
    }
}
pub struct PointScene {
    pub raw:Vec<Point>,
    pub lod:Vec<Point>,
    templates:Vec<Template>,
    sites:Vec<Option<Site>>,
    world:Vec<WorldLevel>,
    local_levels:Vec<usize>,
    brick_lod:bool,
    pub extent:Vec3,
}
#[derive(Clone,Copy)]
pub struct View {pub origin:Vec3,pub forward:Vec3,pub right:Vec3,pub up:Vec3,pub width:u32,pub height:u32}
impl View {
    fn focal(self)->f32 {self.height as f32/(2.0*(30f32.to_radians()).tan())}
    fn screen_size(self,center:Vec3,radius:f32,width:f32)->Option<f32> {
        let d=center-self.origin;
        let z=d.dot(self.forward);
        if z+radius<=0.05{return None;}
        let f=self.focal();
        let sx=self.width as f32*0.5/f;
        let sy=self.height as f32*0.5/f;
        // Conservative sphere vs normalized frustum planes (including near).
        if d.dot(self.right).abs()>z*sx+radius*(1.0+sx*sx).sqrt(){return None;}
        if d.dot(self.up).abs()>z*sy+radius*(1.0+sy*sy).sqrt(){return None;}
        Some(width*f/(z-radius).max(0.05))
    }
}
#[derive(Clone,Copy,Debug)]
pub struct Draw {pub point_count:u32,pub start:u32,pub end:u32}
#[repr(C)]
#[derive(Clone,Copy,Debug,Default,Pod,Zeroable)]
pub struct GpuChoice {pub ranges:[u32;4],pub metric:[f32;4]}
pub struct Selection {pub gpu_choices:Vec<GpuChoice>,pub instances:Vec<Instance>,pub draws:Vec<Draw>,pub points:u64,pub raw_points:u64,pub visited:u64,pub world_splats:u64,pub trees:u64,pub threshold:f32,pub budget_met:bool}
// History is indexed by stable spatial identity, never by draw order.
const UNKNOWN:u8=255;
#[derive(Clone)]
pub struct LodState {
    world:Vec<Vec<u8>>,
    trees:Vec<u8>,
    bricks:Vec<u8>,
    threshold:f32,
    base:f32,
    budget:u64,
    spare_frames:u32,
}
fn collapse_world(px:f32,threshold:f32,previous:u8)->bool{
    let factor=match previous{0=>0.85,1=>1.15,_=>1.0};
    px<=threshold*2.0*factor
}
fn choose_local(px_per_m:f32,threshold:f32,previous:u8,levels:&[usize])->usize{
    let Some(mut slot)=levels.iter().position(|&k|k==previous as usize) else {
        return levels.iter().copied().rev().find(|&k|LOCAL_WIDTHS[k]*px_per_m<=threshold).unwrap_or(0);
    };
    // A narrower dead band for the closer-spaced levels; even identical
    // models at different sites need not switch together (see visit).
    let h=if levels.len()>ORIGINAL_LEVELS.len(){0.10}else{0.15};
    while slot>0 && LOCAL_WIDTHS[levels[slot]]*px_per_m>threshold*(1.0+h){slot-=1;}
    while slot+1<levels.len() && LOCAL_WIDTHS[levels[slot+1]]*px_per_m<=threshold*(1.0-h){slot+=1;}
    levels[slot]
}
// Extra levels are built directly from the same alpha-tested, weighted
// source samples. Store only occupied cells, not 256^3 empty entries.
// Sort keys before emission: HashMap iteration order must not affect IDs.
fn append_sparse_level(samples:&[Sample],width:f32,lod:&mut Vec<Point>)->Range{
    let side=(16.0/width).ceil() as u32;
    let mut cells=HashMap::<usize,Aggregate>::with_capacity(samples.len().min(65536));
    for &s in samples{
        cells.entry(cell_index(s.pos,width,side)).or_default().add(s);
    }
    let mut cells:Vec<_>=cells.into_iter().collect();
    cells.sort_unstable_by_key(|(key,_)|*key);
    let start=lod.len() as u32;
    for (_,a) in cells{
        if let Some(s)=a.sample(){lod.push(Point::encode(s.pos,s.radius,s.color,s.normal,s.coverage));}
    }
    Range{offset:start,count:lod.len() as u32-start}
}
// Key order = finest LOD first, then model and brick. The world
// aggregate is last. Sparse groups avoid allocating all 65*22 groups
// per model on every budget probe.
type GroupKey=(usize,usize,usize);
struct Group {count:u32,values:Vec<(Instance,GpuChoice)>}
struct Work {groups:BTreeMap<GroupKey,Group>,points:u64,visited:u64,world_splats:u64,trees:u64}
impl Work {
    fn push(&mut self,key:GroupKey,mut instance:Instance,count:u32,choice:GpuChoice){
        if count==0{return;}
        instance.pad[1]=count;
        let group=self.groups.entry(key).or_insert_with(||Group{count,values:Vec::new()});
        debug_assert_eq!(group.count,count);
        group.values.push((instance,choice));
        self.points+=u64::from(count);
    }
}
// CPU-selected range is a strict upper bound. Offer the GPU two cheaper
// alternatives, but do not cross a brick boundary or select an empty range.
fn gpu_choice(ranges:&[Range;LEVELS],levels:&[usize],selected:usize,limit:f32,threshold:f32)->GpuChoice{
    let mut c=GpuChoice::default();
    c.metric[2]=threshold;
    let base=ranges[selected];let mut k=0;
    for &level in levels{
        if level<=selected||LOCAL_WIDTHS[level]>limit{continue;}
        let r=ranges[level];
        if r.count==0||r.count>=base.count{continue;}
        if k>0&&r.count>=c.ranges[2*k-1]{continue;}
        c.ranges[k*2]=r.offset;c.ranges[k*2+1]=r.count;c.metric[k]=LOCAL_WIDTHS[level];k+=1;
        if k==2{break;}
    }
    c
}
fn tree_instance(batch:crate::Batch,x:u32,z:u32,id:u32,under:bool)->Instance {
    let turn=id&3;
    let inverse=(4-turn)&3;
    let center=Vec3::new(CELL*0.5,0.0,CELL*0.5);
    let key=x+z*65537;
    let cell=Vec3::new(x as f32*CELL,0.0,z as f32*CELL);
    if under {
        let axis=|seed| {let j=random(key^seed);if j<0.5{4.0+j*2.0}else{11.0+(j-0.5)*2.0}};
        let anchor=Vec3::new(axis(0x715ed0b1),0.0,axis(0x48f52175));
        let scale=0.34+0.12*random(key^0x57d05d9b);
        Instance::new(cell+anchor-rotate(center,inverse)*scale,scale,0,inverse)
    }else{
        let lo=Vec3::from(batch.lo);let hi=Vec3::from(batch.hi);
        let jitter=|lo:f32,hi:f32,seed|{let left=lo.min(2.3);let right=(CELL-hi).min(2.3);random(key^seed)*(left+right)-left};
        let shift=Vec3::new(jitter(lo.x,hi.x,0x57a318d1),0.0,jitter(lo.z,hi.z,0x6a814b21));
        Instance::new(cell+center+rotate(shift-center,inverse),1.0,0,inverse)
    }
}
fn cell_index(p:Vec3,width:f32,side:u32)->usize {
    let c=(p/width).floor().as_uvec3().min(UVec3::splat(side-1));
    (c.x+side*(c.y+side*c.z)) as usize
}
impl PointScene {
    fn template_data(geo:&crate::TreeGeometry)->(Vec<Point>,Vec<Point>,Vec<Template>,Vec<usize>,bool){
        let mut raw=Vec::with_capacity(geo.surfels.len());
        let mut lod=Vec::new();
        let mut templates=Vec::with_capacity(geo.batches.len());
        let brick_lod=std::env::var("IGOR_BRICK_LOD").as_deref()==Ok("1");
        let dense_lod=std::env::var("IGOR_DENSE_LOD").as_deref()!=Ok("0");
        let mid_lod=dense_lod&&std::env::var("IGOR_MID_LOD").as_deref()!=Ok("0");
        let local_levels:Vec<usize>=if mid_lod{(0..LEVELS).collect()}else if dense_lod{DENSE_LEVELS.to_vec()}else{ORIGINAL_LEVELS.to_vec()};
        for batch in &geo.batches {
            let mut ranges=[Range::default();LEVELS];
            let start=raw.len() as u32;
            // Dense direct-address grid of 0.25 m, per template (64^3).
            // Coarser levels are reductions of it, not resampled ray images.
            let mut side=64u32;
            let mut grid=vec![Aggregate::default();(side*side*side) as usize];
            let mut summary=Aggregate::default();
            let mut samples=Vec::new();
            for s in &geo.surfels[batch.offset as usize..(batch.offset+batch.count) as usize] {
                let (center,r)=s.center_radius();
                let patch=&geo.patches[s.patch as usize];
                let n=Vec3::from_slice(&patch.plane).normalize_or(Vec3::Y);
                let tangent=n.cross(if n.y.abs()<0.9{Vec3::Y}else{Vec3::X}).normalize_or(Vec3::X);
                let bitangent=n.cross(tangent);
                // Full-detail records reuse the original disk and source
                // material ID. Alpha and triangle bounds are evaluated after
                // projection in the fragment shader, not by ray intersections.
                let mut point=Point::encode(center,r,Vec3::ZERO,n,1.0);
                point.rgba=s.patch;
                raw.push(point);
                // Only the coarser hierarchy uses sampled color and coverage.
                // Multiple samples retain more leaf detail than one center tap.
                let mut pos_sum=Vec3::ZERO;
                let mut color_sum=Vec3::ZERO;
                let mut normal_sum=Vec3::ZERO;
                let mut count=0u32;
                for pos in [center,center+tangent*r*0.35,center-tangent*r*0.35,center+bitangent*r*0.35,center-bitangent*r*0.35] {
                    let b=patch.coordinates(pos);
                    if b.min_element()>=-0.0001 && b.element_sum()<=1.0001 && geo.alpha_visible(patch,b) {
                        let (color,normal)=geo.surface(patch,b);
                        pos_sum+=pos;color_sum+=color;normal_sum+=normal;count+=1;
                    }
                }
                if count>0 {
                    let inv=1.0/count as f32;
                    let sample=Sample{pos:pos_sum*inv,color:color_sum*inv,normal:normal_sum.normalize_or(n),radius:r,coverage:count as f32/5.0};
                    grid[cell_index(sample.pos,0.25,side)].add(sample);
                    summary.add(sample);
                    if dense_lod{samples.push(sample);}
                }
            }
            ranges[0]=Range{offset:RAW|start,count:raw.len() as u32-start};
            for &level in &ORIGINAL_LEVELS[1..] {
                let start=lod.len() as u32;
                for a in &grid {if let Some(s)=a.sample(){lod.push(Point::encode(s.pos,s.radius,s.color,s.normal,s.coverage));}}
                ranges[level]=Range{offset:start,count:lod.len() as u32-start};
                if side>1 {
                    let half=side/2;
                    let mut parent=vec![Aggregate::default();(half*half*half) as usize];
                    for (i,a) in std::mem::take(&mut grid).into_iter().enumerate() {
                        if a.area==0.0{continue;}
                        let i=i as u32;let x=i%side;let y=i/side%side;let z=i/(side*side);
                        let j=(x/2+half*(y/2+half*(z/2))) as usize;
                        let dst=&mut parent[j];
                        // Exact reduction of weighted sums and bounds.
                        if dst.area==0.0 {*dst=a;}else{
                            dst.weighted_pos+=a.weighted_pos;dst.weighted_color+=a.weighted_color;
                            dst.weighted_normal+=a.weighted_normal;dst.area+=a.area;
                            dst.lo=dst.lo.min(a.lo);dst.hi=dst.hi.max(a.hi);
                        }
                    }
                    grid=parent;side=half;
                }
            }
            if dense_lod{
                for level in EXTRA_LEVELS{
                    ranges[level]=append_sparse_level(&samples,LOCAL_WIDTHS[level],&mut lod);
                }
                if mid_lod{
                    for level in MID_LEVELS{
                        ranges[level]=append_sparse_level(&samples,LOCAL_WIDTHS[level],&mut lod);
                    }
                }
                // Independent grids need not be nested. Repair inversions
                // only between active levels, preserving the 15-level
                // baseline when mid LOD is disabled. Never alias raw.
                for pair in local_levels[1..].windows(2).rev(){
                    let fine=pair[0];let coarse=pair[1];
                    if ranges[fine].count<ranges[coarse].count{
                        ranges[fine]=ranges[coarse];
                    }
                }
            }
            let lo=Vec3::from(batch.lo);let hi=Vec3::from(batch.hi);
            let summary=summary.sample().unwrap_or(Sample{pos:(lo+hi)*0.5,color:Vec3::new(0.15,0.3,0.1),normal:Vec3::Y,radius:0.01,coverage:0.0});
            let center=(lo+hi)*0.5;
            // Bound actual emitted disks, not the original mesh alone.
            // Raw disk centers can be snapped onto the source plane after
            // decoding; add a conservative margin for quantization/roundoff.
            let radii=std::array::from_fn(|level|{
                let range=ranges[level];
                let start=(range.offset&!RAW) as usize;
                let data=if level==0{&raw}else{&lod};
                data[start..start+range.count as usize].iter().map(|p|{
                    let pos=Vec3::new((p.xy&65535) as f32,(p.xy>>16) as f32,(p.zr&65535) as f32)/4096.0;
                    let r=(p.zr>>16) as f32/4096.0;
                    (pos-center).length()+r+0.01
                }).fold(0.01f32,f32::max)
            });
            let bricks=if brick_lod{partition_bricks(&mut raw,&mut lod,&ranges,&local_levels)}else{Vec::new()};
            templates.push(Template{ranges,radii,lo,hi,summary,bricks});
        }
        (raw,lod,templates,local_levels,brick_lod)
    }
    /// Build the same shared templates used by PointScene::build, without
    /// allocating the demo's procedural grid of tree instances.
    pub fn build_templates(geo:&crate::TreeGeometry)->TemplateLibrary{
        let (raw,lod,templates,levels,_)=Self::template_data(geo);
        TemplateLibrary{raw,lod,templates,levels}
    }
    pub(crate) fn planet_templates(geo:&crate::TreeGeometry)->(Vec<Point>,Vec<Point>,Vec<crate::planetary::TemplateInfo>,Vec<usize>){
        let (raw,lod,templates,levels,_)=Self::template_data(geo);
        let templates=templates.into_iter().map(|t|crate::planetary::TemplateInfo{
            ranges:t.ranges,radii:t.radii,center:((t.lo+t.hi)*0.5).to_array(),bound:(t.hi-t.lo).length()*0.5,
            summary:crate::planetary::Summary{position:t.summary.pos.to_array(),color:t.summary.color.to_array(),normal:t.summary.normal.to_array(),radius:t.summary.radius,coverage:t.summary.coverage}
        }).collect();
        (raw,lod,templates,levels)
    }
    pub fn build(scene:&Scene)->Self{
        let geo=&scene.geometry;
        let (raw,mut lod,templates,local_levels,brick_lod)=Self::template_data(geo);
        let side=scene.levels[0].dims.x;
        let mut sites=vec![None;(side*side) as usize];
        let mut world=Vec::new();
        let mut grid=vec![Aggregate::default();sites.len()];
        for z in 0..side {for x in 0..side {
            let id=scene.blocks[scene.levels[0].index(UVec3::new(x,0,z))].batch;
            if id==0{continue;}
            let ids=[(id&65535)-1,(id>>16)-1];
            let models=ids.map(|id|(id/4) as usize);
            let trees=std::array::from_fn(|i|tree_instance(geo.batches[models[i]],x,z,ids[i],i==1));
            let i=(x+side*z) as usize;
            sites[i]=Some(Site{trees,models});
            for t in 0..2 {
                let mut s=templates[models[t]].summary;
                if s.coverage==0.0{continue;}
                s.pos=trees[t].world(s.pos);
                s.normal=rotate(s.normal,trees[t].turn);
                s.radius*=trees[t].origin_scale[3];
                grid[i].add(s);
            }
        }}
        let mut n=side;
        let mut width=CELL;
        loop {
            let cells=grid.iter().map(|a|a.sample().map(|s|{
                let offset=lod.len() as u32;
                // World position and large radius live in the instance.
                lod.push(Point::encode(Vec3::ZERO,1.0,s.color,s.normal,s.coverage));
                WorldMeta{sample:s,offset}
            })).collect();
            world.push(WorldLevel{side:n,width,cells});
            if n<=1{break;}
            let next=(n+3)/4;
            let mut parent=vec![Aggregate::default();(next*next) as usize];
            for (i,a) in grid.into_iter().enumerate() {
                if a.area==0.0{continue;}
                let i=i as u32;let j=(i%n/4+next*(i/n/4)) as usize;
                let dst=&mut parent[j];
                if dst.area==0.0 {*dst=a;}else{
                    dst.weighted_pos+=a.weighted_pos;dst.weighted_color+=a.weighted_color;dst.weighted_normal+=a.weighted_normal;dst.area+=a.area;
                    dst.lo=dst.lo.min(a.lo);dst.hi=dst.hi.max(a.hi);
                }
            }
            grid=parent;n=next;width*=4.0;
        }
        let counts:Vec<_>=local_levels.iter().map(|&level|{
            let count:u64=templates.iter().map(|t|u64::from(t.ranges[level].count)).sum();
            format!("{:.5}m={count}",LOCAL_WIDTHS[level])
        }).collect();
        println!("Local LOD levels (shared template records): {}",counts.join(" | "));
        Self{raw,lod,templates,sites,world,local_levels,brick_lod,extent:scene.extent()}
    }
    pub fn shadow_field(&self,max_dim:u32,requested_step:f32)->ShadowField{
        let max_dim=max_dim.clamp(1,2048);
        let step=requested_step.max((self.extent.x.max(self.extent.z)+2.0*SHADOW_PAD)/max_dim as f32).max(0.5);
        let w=((self.extent.x+2.0*SHADOW_PAD)/step).ceil().max(1.0) as u32;
        let h=((self.extent.z+2.0*SHADOW_PAD)/step).ceil().max(1.0) as u32;
        let w=w.min(max_dim);let h=h.min(max_dim);
        let mut pixels=vec![0u8;w as usize*h as usize*4];
        // Reuse the precomputed 1 m template LOD, independent of camera
        // visibility and current LOD. Off-screen trees cast shadows too.
        let level=LOCAL_WIDTHS.iter().position(|&x|x==1.0).unwrap();
        for site in self.sites.iter().flatten(){
            for j in 0..2{
                let inst=site.trees[j];
                let range=self.templates[site.models[j]].ranges[level];
                let start=range.offset as usize;
                for pt in &self.lod[start..start+range.count as usize]{
                    let p=inst.world(point_position(pt));
                    let r=(point_radius(pt)*inst.origin_scale[3]).min(2.5);
                    let a=(pt.rgba>>24) as f32/255.0;
                    add_shadow_sample(&mut pixels,w,h,step,p,r,a);
                }
            }
        }
        ShadowField{width:w,height:h,pixels,transform:[SHADOW_PAD,SHADOW_PAD,1.0/(w as f32*step),1.0/(h as f32*step)]}
    }
    pub fn max_instances(&self)->usize {
        let trees:usize=self.sites.iter().flatten().map(|s|s.models.iter().map(|&m|self.templates[m].bricks.iter().filter(|b|b.radius>0.0).count().max(1)).sum::<usize>()).sum();
        trees+self.world.iter().map(|l|l.cells.len()).sum::<usize>()
    }
    pub fn bytes(&self)->usize {(self.raw.len()+self.lod.len())*std::mem::size_of::<Point>()}
    pub fn lod_state(&self)->LodState{
        LodState{world:self.world.iter().map(|l|vec![UNKNOWN;l.cells.len()]).collect(),trees:vec![UNKNOWN;self.sites.len()*2],bricks:if self.brick_lod{vec![UNKNOWN;self.sites.len()*2*BRICKS]}else{Vec::new()},threshold:0.25,base:-1.0,budget:0,spare_frames:0}
    }
    fn visit(&self,view:View,level:usize,x:u32,z:u32,threshold:f32,work:&mut Work,previous:Option<&LodState>,next:&mut LodState) {
        let l=&self.world[level];
        let i=(x+l.side*z) as usize;
        let Some(meta)=l.cells[i] else{return;};
        work.visited+=1;
        // Cull by the full spatial block, not by the approximate representative.
        let center=Vec3::new((x as f32+0.5)*l.width,8.0,(z as f32+0.5)*l.width);
        let radius=Vec3::new(l.width*0.5+0.001,8.01,l.width*0.5+0.001).length();
        let Some(px)=view.screen_size(center,radius,l.width) else{return;};
        let collapse=collapse_world(px,threshold,previous.map_or(UNKNOWN,|s|s.world[level][i]));
        next.world[level][i]=u8::from(collapse);
        if collapse {
            let mut instance=Instance::new(meta.sample.pos,meta.sample.radius,meta.offset,0);
            instance.pad[0]=1; // legacy presentation only
            instance.bound=meta.sample.pos.extend(meta.sample.radius*1.001+0.01).to_array();
            work.push((LEVELS,0,0),instance,1,GpuChoice::default());
            work.world_splats+=1;
            return;
        }
        if level>0 {
            let n=self.world[level-1].side;
            for dz in 0..4 {for dx in 0..4 {
                let xx=x*4+dx;let zz=z*4+dz;
                if xx<n&&zz<n{self.visit(view,level-1,xx,zz,threshold,work,previous,next);}
            }}
            return;
        }
        let Some(site)=self.sites[i] else{return;};
        for j in 0..2 {
            let template=&self.templates[site.models[j]];
            let mut instance=site.trees[j];
            let scale=instance.origin_scale[3];
            let center=instance.world((template.lo+template.hi)*0.5);
            let radius=(template.hi-template.lo).length()*0.5*scale;
            let Some(px_per_m)=view.screen_size(center,radius,scale) else{continue;};
            // Preserve local detail close to the camera when the budget forces
            // the global threshold upward. A smooth distance-dependent factor
            // makes far objects lose detail first. The hard budget still wins.
            let distance=((center-view.origin).length()-radius).max(0.0);
            let weight=0.18+0.82*(distance/160.0).clamp(0.0,1.0);
            let identity=i*2+j;
            // Small, fixed spatial variation staggers transitions across
            // trees. It never changes with the frame or camera position.
            let stagger=if self.local_levels.len()>ORIGINAL_LEVELS.len(){
                0.94+0.12*random((identity as u32)^0x3f912ac7)
            }else{1.0};
            let local_threshold=(threshold*weight*stagger).max(0.25);
            let selected=choose_local(px_per_m,local_threshold,previous.map_or(UNKNOWN,|s|s.trees[identity]),&self.local_levels);
            next.trees[identity]=selected as u8;
            // Refine near and middle trees independently in template-local
            // 4 m bricks. Far trees keep the cheap whole-template path.
            if self.brick_lod && LOCAL_WIDTHS[selected]<=BRICK_MAX {
                let mut visible=false;
                for (b,brick) in template.bricks.iter().enumerate(){
                    if brick.radius<=0.0{continue;}
                    let center=instance.world(brick.center);
                    let radius=brick.radius*scale+0.01;
                    let Some(px)=view.screen_size(center,radius,scale) else{continue;};
                    let distance=((center-view.origin).length()-radius).max(0.0);
                    let local=(threshold*(0.18+0.82*(distance/160.0).clamp(0.0,1.0))*stagger).max(0.25);
                    let id=identity*BRICKS+b;
                    let chosen=choose_local(px,local,previous.map_or(UNKNOWN,|s|s.bricks[id]),&self.local_levels);
                    let Some(level)=self.local_levels.iter().copied().rev().find(|&k|k<=chosen&&LOCAL_WIDTHS[k]<=BRICK_MAX&&brick.ranges[k].count>0) else{continue;};
                    next.bricks[id]=level as u8;
                    let range=brick.ranges[level];
                    let mut part=instance;part.offset=range.offset;part.bound=center.extend(radius).to_array();
                    let choice=gpu_choice(&brick.ranges,&self.local_levels,level,BRICK_MAX,local);
                    work.push((level,site.models[j],b+1),part,range.count,choice);
                    visible=true;
                }
                if visible{work.trees+=1;}
            }else{
                let range=template.ranges[selected];
                instance.offset=range.offset;
                let bound=template.radii[selected..].iter().copied().fold(0.0f32,f32::max);
                instance.bound=center.extend(bound*scale+0.01).to_array();
                if range.count>0 {
                    let choice=gpu_choice(&template.ranges,&self.local_levels,selected,16.0,local_threshold);
                    work.push((selected,site.models[j],0),instance,range.count,choice);
                    work.trees+=1;
                }
            }
        }
    }
    pub fn select(&self,view:View,base_threshold:f32,budget:u64)->Selection {
        self.fit(view,base_threshold.max(0.25),budget,None,1.5).0
    }
    // Preserve the last accepted threshold across frames. Coarsen only to
    // satisfy the hard cap; refine slowly after sustained spare capacity.
    pub fn select_stable(&self,view:View,base_threshold:f32,budget:u64,state:&mut LodState)->Selection{
        let base=base_threshold.clamp(0.25,512.0);
        if state.base!=base || state.budget!=budget{
            *state=self.lod_state();
            state.base=base;state.budget=budget;state.threshold=base;
        }
        let current=state.threshold.max(base);
        let mut tried=false;
        if state.spare_frames>=12 && current>base {
            tried=true;
            let candidate=(current/if self.local_levels.len()>ORIGINAL_LEVELS.len(){1.08}else{1.125}).max(base);
            let (work,next)=self.build_selection(view,candidate,budget,Some(state));
            if work.budget_met && work.points<=budget.saturating_mul(85)/100{
                let mut next=next;
                next.threshold=candidate;next.spare_frames=0;
                *state=next;return work;
            }
        }
        let (work,mut next)=self.fit(view,current,budget,Some(state),if self.local_levels.len()>ORIGINAL_LEVELS.len(){1.15}else{1.25});
        next.threshold=work.threshold;
        next.spare_frames=if !tried && work.budget_met && work.threshold==current && work.points<=budget.saturating_mul(75)/100{
            state.spare_frames.saturating_add(1)
        }else{0};
        if work.budget_met{*state=next;}
        work
    }
    fn fit(&self,view:View,mut threshold:f32,budget:u64,previous:Option<&LodState>,step:f32)->(Selection,LodState){
        loop{
            let result=self.build_selection(view,threshold,budget,previous);
            if result.0.budget_met || threshold>=512.0{return result;}
            threshold=(threshold*step).min(512.0);
        }
    }
    fn build_selection(&self,view:View,threshold:f32,budget:u64,previous:Option<&LodState>)->(Selection,LodState){
            // Failed budget probes must never change last-frame history.
            let mut next=previous.cloned().unwrap_or_else(||self.lod_state());
            let mut work=Work{groups:BTreeMap::new(),points:0,visited:0,world_splats:0,trees:0};
            let top=self.world.len()-1;
            let n=self.world[top].side;
            for z in 0..n {for x in 0..n {self.visit(view,top,x,z,threshold,&mut work,previous,&mut next);}}
            let mut instances=Vec::new();let mut draws=Vec::new();let mut gpu_choices=Vec::new();
            let mut raw_points=0u64;
            for ((level,_,_),group) in work.groups {
                let start=instances.len() as u32;
                if level==0{raw_points+=u64::from(group.count)*group.values.len() as u64;}
                for (instance,choice) in group.values{instances.push(instance);gpu_choices.push(choice);}
                draws.push(Draw{point_count:group.count,start,end:instances.len() as u32});
            }
            (Selection{gpu_choices,instances,draws,points:work.points,raw_points,visited:work.visited,world_splats:work.world_splats,trees:work.trees,threshold,budget_met:work.points<=budget},next)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn weighted_aggregate() {
        let mut a=Aggregate::default();
        for x in [1.0,3.0] {a.add(Sample{pos:Vec3::new(x,1.0,1.0),color:Vec3::splat(x/4.0),normal:Vec3::Y,radius:0.5,coverage:1.0});}
        let s=a.sample().unwrap();assert!((s.pos.x-2.0).abs()<1e-6);assert!((s.color.x-0.5).abs()<1e-6);
        assert!((0.0..=1.0).contains(&s.coverage));
    }
    #[test] fn frustum_rejects_behind() {
        let v=View{origin:Vec3::ZERO,forward:Vec3::Z,right:Vec3::X,up:Vec3::Y,width:640,height:360};
        assert!(v.screen_size(Vec3::new(0.0,0.0,-10.0),1.0,2.0).is_none());
        assert!(v.screen_size(Vec3::new(0.0,0.0,10.0),1.0,2.0).is_some());
    }
    #[test] fn local_hysteresis_keeps_previous_level_inside_band(){
        assert_eq!(choose_local(4.0,1.0,UNKNOWN,&ORIGINAL_LEVELS),5);
        assert_eq!(choose_local(4.2,1.0,5,&ORIGINAL_LEVELS),5);
        assert_eq!(choose_local(4.2,1.0,0,&ORIGINAL_LEVELS),0);
        assert_eq!(choose_local(4.8,1.0,5,&ORIGINAL_LEVELS),0);
        assert_eq!(choose_local(3.2,1.0,0,&ORIGINAL_LEVELS),5);
    }
    #[test] fn intermediate_levels_are_selected(){
        let levels:Vec<_>=(0..LEVELS).collect();
        assert_eq!(choose_local(20.0,1.0,UNKNOWN,&levels),0);
        assert_eq!(choose_local(14.0,1.0,UNKNOWN,&levels),1);
        assert_eq!(choose_local(10.0,1.0,UNKNOWN,&levels),2);
        assert_eq!(choose_local(7.0,1.0,UNKNOWN,&levels),3);
        assert_eq!(choose_local(5.0,1.0,UNKNOWN,&levels),4);
        assert_eq!(choose_local(4.2,1.0,5,&levels),5);
        assert_eq!(choose_local(4.2,1.0,4,&levels),4);
    }
    #[test] fn mid_levels_fill_gaps_without_changing_near_levels(){
        let levels:Vec<_>=(0..LEVELS).collect();
        assert_eq!(choose_local(1.5,1.0,UNKNOWN,&levels),8);
        assert_eq!(choose_local(1.5,1.0,UNKNOWN,&DENSE_LEVELS),7);
        assert_eq!(choose_local(1.1,1.0,UNKNOWN,&levels),10);
        assert_eq!(choose_local(0.7,1.0,UNKNOWN,&levels),12);
        assert_eq!(choose_local(0.30,1.0,UNKNOWN,&levels),17);
        for &level in &DENSE_LEVELS{
            assert!(levels.contains(&level));
        }
        assert!(LOCAL_WIDTHS.windows(2).all(|p|p[0]<p[1]));
        assert_eq!(&DENSE_LEVELS[..8],&levels[..8]);
    }
    #[test] fn sparse_levels_are_deterministic(){
        let samples:Vec<_>=[0.01,0.07,0.19].into_iter().map(|x|Sample{
            pos:Vec3::new(x,0.0,0.0),color:Vec3::ONE,normal:Vec3::Y,radius:0.01,coverage:1.0
        }).collect();
        let mut a=Vec::new();let mut b=Vec::new();
        assert_eq!(append_sparse_level(&samples,0.0625,&mut a).count,3);
        assert_eq!(append_sparse_level(&samples,0.125,&mut b).count,2);
        let mut c=Vec::new();append_sparse_level(&samples,0.0625,&mut c);
        assert_eq!(bytemuck::cast_slice::<Point,u8>(&a),bytemuck::cast_slice::<Point,u8>(&c));
    }
    #[test] fn world_hysteresis_keeps_previous_branch(){
        assert!(collapse_world(2.1,1.0,1));
        assert!(!collapse_world(1.9,1.0,0));
        assert!(!collapse_world(2.4,1.0,1));
        assert!(collapse_world(1.6,1.0,0));
    }
    #[test] fn shadow_field_projects_and_accumulates_height_band(){
        let p=Vec3::new(0.0,6.0,0.0);
        assert_eq!(shadow_projection(p),(3.75,3.0));
        let mut pixels=vec![0u8;64*64*4];
        add_shadow_sample(&mut pixels,64,64,1.0,p,1.0,1.0);
        let first=pixels.clone();
        assert!(pixels.chunks_exact(4).any(|p|p[1]>0));
        assert!(pixels.chunks_exact(4).all(|p|p[0]==0&&p[2]==0&&p[3]==0));
        add_shadow_sample(&mut pixels,64,64,1.0,p,1.0,1.0);
        assert!(pixels.iter().zip(first.iter()).all(|(a,b)|a>=b));
        assert!(pixels.iter().zip(first.iter()).any(|(a,b)|a>b));
    }
    #[test] fn brick_partition_preserves_point_counts(){
        let mut raw=vec![Point::encode(Vec3::new(9.0,1.0,1.0),0.1,Vec3::ONE,Vec3::Y,1.0),Point::encode(Vec3::ONE,0.1,Vec3::ONE,Vec3::Y,1.0)];
        let mut ranges=[Range::default();LEVELS];ranges[0]=Range{offset:RAW,count:2};
        let parts=partition_bricks(&mut raw,&mut [],&ranges,&[0]);
        assert_eq!(parts[0].ranges[0].count,1);assert_eq!(parts[2].ranges[0].count,1);
        assert_eq!(parts.iter().map(|b|b.ranges[0].count).sum::<u32>(),2);
        assert_eq!(point_position(&raw[0]),Vec3::ONE);
    }
    #[test] fn instance_layout() {assert_eq!(std::mem::size_of::<Instance>(),48);}
    #[test] fn point_packing() {
        let p=Point::encode(Vec3::new(1.0,2.0,3.0),0.5,Vec3::ONE,Vec3::Y,1.0);
        assert_eq!(p.xy&65535,4096);assert_eq!(p.xy>>16,8192);assert_eq!(p.zr&65535,12288);assert_eq!(p.zr>>16,2048);assert_eq!(p.rgba,0xffffffff);
        assert_eq!(std::mem::size_of::<Point>(),16);
    }
}

/// Original demo hysteresis selection, shared by the planetary adapter.
pub fn planetary_level(px_per_m:f32,threshold:f32,previous:u8)->usize{choose_local(px_per_m,threshold,previous,&[0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21])}

/// Use the demo's weighted area and bounds for planetary world clusters.
#[derive(Default)]
pub struct PlanetAggregate(Aggregate);
impl PlanetAggregate{
    pub fn add(&mut self,s:crate::planetary::Summary){
        self.0.add(Sample{pos:Vec3::from_array(s.position),color:Vec3::from_array(s.color),normal:Vec3::from_array(s.normal),radius:s.radius,coverage:s.coverage});
    }
    pub fn finish(self)->Option<([f32;3],f32,u32,u32)>{
        let s=self.0.sample()?;
        let p=Point::encode(Vec3::ZERO,1.0,s.color,s.normal,s.coverage);
        Some((s.pos.to_array(),s.radius,p.rgba,p.normal))
    }
}
