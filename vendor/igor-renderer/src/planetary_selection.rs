//! Sparse spherical-world adapter to the original point hierarchy.
//! Same weighted aggregates, 4x parent reduction, local LODs, hysteresis
//! and budget policy as the demo. No Bevy types or GPU device dependencies.
use super::{Aggregate,Sample,Point,Range,choose_local,collapse_world,raster_shadow_sample,point_position,point_radius,LOCAL_WIDTHS,ORIGINAL_LEVELS,UNKNOWN,RAW};
use crate::{planetary::Pack,render_api::LANES};
use glam::{DVec3,Vec3,Quat};
use std::{collections::{BTreeMap,HashMap}, sync::Arc};
use tracing::trace_span;

/// Unmodified game-world placement, before the loader normalization.
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct Placement{
    pub position:[f64;3],pub rotation:[f32;4],pub scale:f32,pub model:usize,pub phase:f32,
}
#[derive(Clone,Copy)]
pub struct Camera{
    pub position:[f64;3],pub forward:[f32;3],pub right:[f32;3],pub up:[f32;3],
    pub focal:f32,pub width:u32,pub height:u32,pub reach:f32,pub planet_radius:f64,
}
impl Camera{
    fn eye(self)->DVec3{DVec3::from_array(self.position)}
    fn projected(self,center:DVec3,radius:f32,span:f32)->Option<f32>{
        let d=(center-self.eye()).as_vec3();
        if d.length()>self.reach+radius{return None;}
        let z=d.dot(Vec3::from_array(self.forward));
        if z+radius<=0.05{return None;}
        let sx=self.width as f32*0.5/self.focal;
        let sy=self.height as f32*0.5/self.focal;
        if d.dot(Vec3::from_array(self.right)).abs()>z*sx+radius*(1.0+sx*sx).sqrt(){return None;}
        if d.dot(Vec3::from_array(self.up)).abs()>z*sy+radius*(1.0+sy*sy).sqrt(){return None;}
        // Conservative horizon test against the upper edge of the bound.
        if self.planet_radius>0.0{
            let crown=center+center.normalize_or_zero()*f64::from(radius);
            let ray=crown-self.eye();
            let closest=(-self.eye().dot(ray)/ray.length_squared().max(1.0)).clamp(0.0,1.0);
            if (self.eye()+ray*closest).length_squared()<self.planet_radius*self.planet_radius{return None;}
        }
        Some(span*self.focal/(z-radius).max(0.05))
    }
    fn fade(self,center:DVec3)->u32{
        let d=(center-self.eye()).length() as f32;
        let t=((d-self.reach*0.79)/(self.reach*0.21).max(0.01)).clamp(0.0,1.0);
        ((1.0-t*t*(3.0-2.0*t))*12.0).round() as u32
    }
}
type Cell=[i64;3];
type Identity=[u64;3];
const WIDTHS:[f64;5]=[16.0,64.0,256.0,1024.0,4096.0];
fn cell(p:DVec3,width:f64)->Cell{(p/width).to_array().map(|v|v.floor() as i64)}
fn anchor(k:Cell,width:f64)->DVec3{DVec3::new(k[0] as f64,k[1] as f64,k[2] as f64)*width}
fn identity(p:Placement)->Identity{p.position.map(f64::to_bits)}
fn parent(k:Cell)->Cell{k.map(|v|v.div_euclid(4))}
#[derive(Clone,Copy)]
struct Tree{
    input:Placement,origin:DVec3,rotation:Quat,scale:f32,center:DVec3,radius:f32,summary:Sample,
}
impl Tree{
    fn new(input:Placement,pack:&Pack)->Option<Self>{
        let t=pack.templates.get(&input.model)?;
        let rotation=Quat::from_array(input.rotation);
        let scale=input.scale/t.transform.scale;
        if !scale.is_finite()||scale<=0.0{return None;}
        let origin=DVec3::from_array(input.position)+(rotation*(Vec3::from_array(t.transform.root)*input.scale-Vec3::new(8.0,0.0,8.0)*scale)).as_dvec3();
        let center=origin+(rotation*(Vec3::from_array(t.info.center)*scale)).as_dvec3();
        let radius=t.info.bound*scale;
        let s=t.info.summary;
        // Keep the summary in tree-local coordinates until building a cell.
        let summary=Sample{pos:rotation*(Vec3::from_array(s.position)*scale),normal:rotation*Vec3::from_array(s.normal),
            color:Vec3::from_array(s.color),radius:s.radius*scale,coverage:s.coverage};
        Some(Self{input,origin,rotation,scale,center,radius,summary})
    }
}
struct Node{
    key:Cell,anchor:DVec3,lo:DVec3,hi:DVec3,aggregate:Aggregate,
    members:Vec<usize>,count:u32,
}
impl Node{
    fn new(key:Cell,width:f64)->Self{
        Self{key,anchor:anchor(key,width),lo:DVec3::splat(f64::INFINITY),hi:DVec3::splat(f64::NEG_INFINITY),
            aggregate:Aggregate::default(),members:Vec::new(),count:0}
    }
    fn bounds(&mut self,lo:DVec3,hi:DVec3){self.lo=self.lo.min(lo);self.hi=self.hi.max(hi);}
    fn add_tree(&mut self,index:usize,t:Tree){
        self.members.push(index);self.count+=1;
        let r=DVec3::splat(f64::from(t.radius));
        self.bounds(t.center-r,t.center+r);
        if t.summary.coverage>0.0{
            let mut s=t.summary;
            s.pos+=(t.origin-self.anchor).as_vec3();
            self.aggregate.add(s);
        }
    }
    fn add_child(&mut self,index:usize,c:&Node){
        self.members.push(index);self.count+=c.count;self.bounds(c.lo,c.hi);
        // Same exact weighted-sum reduction as the original planar grid,
        // translated between cell-local origins to preserve precision.
        let a=c.aggregate;
        if a.area==0.0{return;}
        let shift=(c.anchor-self.anchor).as_vec3();
        let d=&mut self.aggregate;
        if d.area==0.0{
            *d=a;d.weighted_pos+=shift*a.area;d.lo+=shift;d.hi+=shift;
        }else{
            d.weighted_pos+=a.weighted_pos+shift*a.area;d.weighted_color+=a.weighted_color;
            d.weighted_normal+=a.weighted_normal;d.area+=a.area;
            d.lo=d.lo.min(a.lo+shift);d.hi=d.hi.max(a.hi+shift);
        }
    }
    fn sphere(&self)->(DVec3,f32){((self.lo+self.hi)*0.5,((self.hi-self.lo).length()*0.5) as f32)}
}
#[derive(Default)]
struct History{
    nodes:HashMap<(usize,Cell),u8>,trees:HashMap<Identity,u8>,
}
struct Item{
    origin:[f32;4],rotation:[f32;4],range:[u32;4],count:u32,fade:u32,level:usize,model:usize,
}
#[derive(Default)]
struct Work{
    items:Vec<Item>,history:History,points:u64,jobs:u64,raw:u64,covered:u32,clusters:u32,visited:u32,
}
impl Work{
    fn push(&mut self,item:Item,represented:u32,cluster:bool){
        if item.count==0||item.fade==0{return;}
        self.points+=u64::from(item.count);self.jobs+=u64::from(item.count.div_ceil(LANES));
        if !cluster && item.range[0]&RAW!=0{self.raw+=u64::from(item.count);}
        self.covered+=represented;self.clusters+=u32::from(cluster);self.items.push(item);
    }
    fn fits(&self,budget:u64,max_jobs:u64)->bool{self.points<=budget&&self.jobs<=max_jobs}
}
/// GPU word arrays match the existing planetary shader ABI: 48 byte
/// instance, 16 byte job. A cluster stores its color/normal inline.
#[derive(Default)]
pub struct PlanetSelection{
    pub instances:Vec<[u32;12]>,pub jobs:Vec<[u32;4]>,pub points:u32,pub raw_points:u32,
    pub covered:u32,pub clusters:u32,pub visited:u32,pub threshold:f32,pub budget_met:bool,
    pub lod_counts:[u32;22],
}
/// Immutable tree transforms and world hierarchy built against one geometry pack.
#[derive(Default)]
pub struct PlanetHierarchy {
    placements: Vec<Placement>,
    trees: Vec<Tree>,
    levels: Vec<Vec<Node>>,
}

impl PlanetHierarchy {
    /// Build from placements in deterministic order, skipping models absent from the pack.
    /// Use the same pack for selection and shadow baking.
    pub fn build(pack: &Pack, placements: Vec<Placement>) -> Self {
        let _span = trace_span!("PlanetHierarchy::build").entered();
        let trees: Vec<_> = trace_span!("PlanetScene::collect_trees")
            .in_scope(|| placements.iter().filter_map(|&p| Tree::new(p, pack)).collect());
        let leaves = trace_span!("PlanetScene::build_leaf_level").in_scope(|| {
            let mut leaves = BTreeMap::<Cell, Node>::new();
            for (i, &tree) in trees.iter().enumerate() {
                let key = cell(DVec3::from_array(tree.input.position), WIDTHS[0]);
                leaves.entry(key).or_insert_with(|| Node::new(key, WIDTHS[0])).add_tree(i, tree);
            }
            leaves.into_values().collect()
        });
        let mut levels: Vec<Vec<Node>> = vec![leaves];
        {
            let _span = trace_span!("PlanetScene::build_parent_levels").entered();
            for level in 1..WIDTHS.len() {
                let mut parents = BTreeMap::<Cell, Node>::new();
                for (i, child) in levels[level - 1].iter().enumerate() {
                    let key = parent(child.key);
                    parents.entry(key).or_insert_with(|| Node::new(key, WIDTHS[level])).add_child(i, child);
                }
                levels.push(parents.into_values().collect());
            }
        }
        Self { placements, trees, levels }
    }

    /// Compare sorted inputs before rebuilding with the same geometry pack.
    pub fn has_placements(&self, placements: &[Placement]) -> bool {
        self.placements == placements && !self.levels.is_empty()
    }

    /// Off-screen placements are included; selection and camera LOD do not affect shadows.
    pub fn shadow_placements(&self,eye:[f64;3])->Vec<Placement>{
        let _span = trace_span!("PlanetScene::shadow_placements").entered();
        let eye=DVec3::from_array(eye);
        self.trees.iter().filter(|t|t.center.distance_squared(eye)<500.0f64.powi(2)).map(|t|t.input).collect()
    }
}

/// Reuse the selector across frames and hierarchy swaps to preserve LOD history.
#[derive(Default)]
pub struct PlanetScene{
    hierarchy: Arc<PlanetHierarchy>,
    history:History,threshold:f32,base:f32,budget:u64,max_jobs:u64,spare:u32,
}
impl PlanetScene{
    /// Return shadow casters from the installed hierarchy.
    pub fn shadow_placements(&self,eye:[f64;3])->Vec<Placement>{
        self.hierarchy.shadow_placements(eye)
    }

    /// Rebuild synchronously when the placements change. Use a fresh hierarchy
    /// via `replace_hierarchy` when the underlying geometry pack changes.
    pub fn update_sites(&mut self,pack:&Pack,placements:Vec<Placement>)->bool{
        if self.hierarchy.has_placements(&placements) {
            return false;
        }
        self.hierarchy = Arc::new(PlanetHierarchy::build(pack, placements));
        true
    }

    /// Install a completed hierarchy while retaining camera LOD history and budget state.
    /// Returns the old hierarchy so its allocations can be released on another thread.
    pub fn replace_hierarchy(&mut self, hierarchy: Arc<PlanetHierarchy>) -> Arc<PlanetHierarchy> {
        std::mem::replace(&mut self.hierarchy, hierarchy)
    }

    fn visit(&self,pack:&Pack,cam:Camera,level:usize,index:usize,threshold:f32,work:&mut Work){
        let node=&self.hierarchy.levels[level][index];work.visited+=1;
        let (center,radius)=node.sphere();
        let Some(px)=cam.projected(center,radius,WIDTHS[level] as f32)else{return;};
        let key=(level,node.key);
        let collapse=collapse_world(px,threshold,*self.history.nodes.get(&key).unwrap_or(&UNKNOWN));
        work.history.nodes.insert(key,u8::from(collapse));
        if collapse{
            if let Some(s)=node.aggregate.sample(){
                let pos=node.anchor+s.pos.as_dvec3();
                let p=Point::encode(Vec3::ZERO,1.0,s.color,s.normal,s.coverage);
                work.push(Item{origin:(pos-cam.eye()).as_vec3().extend(s.radius).to_array(),
                    rotation:Quat::IDENTITY.to_array(),range:[p.rgba,p.normal,1,0],
                    count:1,fade:cam.fade(pos),level:22,model:0},node.count,true);
            }
            return;
        }
        if level>0{
            for &i in &node.members{self.visit(pack,cam,level-1,i,threshold,work);}
            return;
        }
        for &i in &node.members{
            let t=self.hierarchy.trees[i];
            let Some(ppm)=cam.projected(t.center,t.radius,t.scale)else{continue;};
            let model=&pack.templates[&t.input.model];
            let distance=((t.center-cam.eye()).length() as f32-t.radius).max(0.0);
            let weight=0.18+0.82*(distance/160.0).clamp(0.0,1.0);
            let id=identity(t.input);
            let hash=(id[0]^id[1].rotate_left(21)^id[2].rotate_left(42)) as u32;
            let stagger=if pack.levels.len()>ORIGINAL_LEVELS.len(){0.94+0.12*crate::random(hash^0x3f912ac7)}else{1.0};
            let local=(threshold*weight*stagger).max(0.25);
            let chosen=choose_local(ppm,local,*self.history.trees.get(&id).unwrap_or(&UNKNOWN),&pack.levels);
            work.history.trees.insert(id,chosen as u8);
            let Range{offset,count}=model.info.ranges[chosen];
            work.push(Item{origin:(t.origin-cam.eye()).as_vec3().extend(t.scale).to_array(),rotation:t.rotation.to_array(),
                range:[offset,count,0,t.input.phase.to_bits()],count,fade:cam.fade(t.center),level:chosen,model:t.input.model},1,false);
        }
    }
    fn probe(&self,pack:&Pack,cam:Camera,threshold:f32)->Work{
        let _span = trace_span!("PlanetScene::traverse_hierarchy").entered();
        let mut work=Work::default();
        if let Some(top)=self.hierarchy.levels.len().checked_sub(1){
            for i in 0..self.hierarchy.levels[top].len(){self.visit(pack,cam,top,i,threshold,&mut work);}
        }
        work
    }
    /// Select visible points using the pack used to build the installed hierarchy.
    pub fn select(&mut self,pack:&Pack,cam:Camera,base:f32,budget:u64,max_jobs:u64)->PlanetSelection{
        let _span = trace_span!("PlanetScene::select").entered();
        let base=base.clamp(0.25,512.0);
        if self.base!=base||self.budget!=budget||self.max_jobs!=max_jobs{
            self.base=base;self.budget=budget;self.max_jobs=max_jobs;self.threshold=base;self.spare=0;self.history=History::default();
        }
        let current=self.threshold.max(base);
        let dense=pack.levels.len()>ORIGINAL_LEVELS.len();
        let mut threshold=current;
        let mut tried=false;
        let mut accepted=None;
        // Identical refinement cadence and hysteresis acceptance to demo.
        if self.spare>=12&&current>base{
            tried=true;
            let candidate=(current/if dense{1.08}else{1.125}).max(base);
            let work=self.probe(pack,cam,candidate);
            if work.fits(budget,max_jobs)&&work.points<=budget.saturating_mul(85)/100{
                threshold=candidate;accepted=Some(work);
            }
        }
        let work=accepted.unwrap_or_else(||loop{
            let work=self.probe(pack,cam,threshold);
            if work.fits(budget,max_jobs)||threshold>=512.0{break work;}
            threshold=(threshold*if dense{1.15}else{1.25}).min(512.0);
        });
        let fits=work.fits(budget,max_jobs);
        self.spare=if !tried&&fits&&threshold==current&&work.points<=budget.saturating_mul(75)/100{
            self.spare.saturating_add(1)
        }else{0};
        self.threshold=threshold;
        if !fits{
            // Never send an over-budget dispatch or silently drop trees.
            return PlanetSelection{threshold,budget_met:false,visited:work.visited,..Default::default()};
        }
        let Work{mut items,history,points,jobs,raw,covered,clusters,visited}=work;
        self.history=history;
        // Same demo ordering: fine levels first, model next, world last.
        items.sort_by_key(|i|(i.level,i.model));
        let _span = trace_span!("PlanetScene::generate_instances_and_jobs").entered();
        let mut out=PlanetSelection{instances:Vec::with_capacity(items.len()),jobs:Vec::with_capacity(jobs as usize),
            points:points as u32,raw_points:raw as u32,covered,clusters,visited,threshold,budget_met:true,lod_counts:[0;22]};
        for item in items{
            let a=item.origin.map(f32::to_bits);let b=item.rotation.map(f32::to_bits);let c=item.range;
            let index=out.instances.len() as u32;
            out.instances.push([a[0],a[1],a[2],a[3],b[0],b[1],b[2],b[3],c[0],c[1],c[2],c[3]]);
            for first in (0..item.count).step_by(LANES as usize){
                out.jobs.push([index,first,(item.count-first).min(LANES),item.fade]);
            }
            if item.level<22{out.lod_counts[item.level]+=1;}
        }
        out
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::planetary::{Template,TemplateInfo,Transform,Summary,AtlasData};
    fn pack()->Pack{
        let mut ranges=[Range{offset:0,count:1};22];
        ranges[0]=Range{offset:RAW,count:128};
        ranges[1]=Range{offset:0,count:64};
        let template=Template{transform:Transform{tile:0,root:[0.0;3],scale:1.0},
            info:TemplateInfo{ranges,radii:[2.0;22],center:[8.0,2.0,8.0],bound:2.0,
                summary:Summary{position:[8.0,2.0,8.0],color:[0.2,0.4,0.1],normal:[0.0,1.0,0.0],radius:2.0,coverage:1.0}}};
        Pack{raw:vec![],lod:vec![],patches:vec![],materials:vec![],atlas:AtlasData{side:1,layers:1,tiles:vec![]},
            templates:HashMap::from([(0,template)]),levels:(0..22).collect()}
    }
    fn placement(x:f64,z:f64)->Placement{Placement{position:[x,0.0,z],rotation:[0.0,0.0,0.0,1.0],scale:1.0,model:0,phase:0.0}}
    fn camera()->Camera{Camera{position:[0.0,2.0,0.0],forward:[0.0,0.0,1.0],right:[1.0,0.0,0.0],up:[0.0,1.0,0.0],
        focal:800.0,width:1600,height:900,reach:2000.0,planet_radius:0.0}}
    #[test]fn hierarchy_reuses_static_placement_and_covers_once(){
        let pack=pack();let mut scene=PlanetScene::default();
        let sites=vec![placement(0.0,20.0),placement(-2.0,1000.0),placement(-3.0,1001.0)];
        assert!(scene.update_sites(&pack,sites.clone()));
        assert!(!scene.update_sites(&pack,sites));
        assert_eq!(scene.hierarchy.levels[4].iter().map(|n|n.count).sum::<u32>(),3);
        let out=scene.select(&pack,camera(),2.2,1000,1000);
        assert!(out.budget_met);assert_eq!(out.covered,3);
        assert!(out.clusters>=1);
        assert_eq!(out.jobs.iter().map(|j|j[2]).sum::<u32>(),out.points);
    }
    #[test]fn budget_and_distant_parent_reduction(){
        let pack=pack();let mut scene=PlanetScene::default();
        scene.update_sites(&pack,(0..32).map(|i|placement(i as f64-16.0,600.0)).collect());
        let out=scene.select(&pack,camera(),2.2,8,8);
        assert!(out.budget_met);assert_eq!(out.covered,32);assert!(out.points<=8);
    }
    #[test]
    fn replacing_hierarchy_preserves_lod_history_and_budget_state() {
        let pack = pack();
        let mut scene = PlanetScene::default();
        scene.update_sites(&pack, vec![placement(0.0, 20.0)]);
        assert!(scene.select(&pack, camera(), 2.2, 1000, 1000).budget_met);
        let history = scene.history.trees.clone();
        assert!(!history.is_empty());
        let threshold = scene.threshold;
        let spare = scene.spare;

        let next = Arc::new(PlanetHierarchy::build(&pack, vec![placement(0.0, 30.0)]));
        let retired = scene.replace_hierarchy(next.clone());
        assert!(retired.has_placements(&[placement(0.0, 20.0)]));
        assert!(Arc::ptr_eq(&scene.hierarchy, &next));
        assert_eq!(scene.history.trees, history);
        assert_eq!((scene.threshold, scene.spare, scene.budget, scene.max_jobs), (threshold, spare, 1000, 1000));
    }
    #[test]fn translated_parent_preserves_weighted_summary(){
        let mut child=Node::new([-1,0,0],16.0);
        child.aggregate.add(Sample{pos:Vec3::new(15.0,1.0,2.0),color:Vec3::ONE,normal:Vec3::Y,radius:1.0,coverage:1.0});
        child.lo=child.anchor;child.hi=child.anchor+DVec3::splat(16.0);
        let mut p=Node::new([-1,0,0],64.0);p.add_child(0,&child);
        let a=child.anchor+child.aggregate.sample().unwrap().pos.as_dvec3();
        let b=p.anchor+p.aggregate.sample().unwrap().pos.as_dvec3();
        assert_eq!(a,b);
    }
}


/// Local sun-projected four-band opacity atlas, derived from the demo's
/// precomputed 1m LOD. Basis is world-fixed until the next async bake.
pub struct PlanetShadow{
    pub side:u32,pub pixels:Vec<u8>,pub center:[f64;3],pub sun:[f32;3],
    origin:DVec3,x_axis:Vec3,z_axis:Vec3,up:Vec3,step:f32,band:f32,
}
impl PlanetShadow{
    pub fn bake(pack:&Pack,sites:Vec<Placement>,eye:[f64;3],sun:[f32;3])->Option<Self>{
        let eye=DVec3::from_array(eye);let up=eye.try_normalize()?.as_vec3();
        let sun=Vec3::from_array(sun).try_normalize()?;
        let altitude=sun.dot(up);
        // No local direct sunlight below the horizon. Near-horizontal light
        // is clamped to bound the required caster distance.
        if altitude<=0.05{return None;}
        let ground=sites.iter().min_by(|a,b|{
            let dist=|p:&Placement| (DVec3::from_array(p.position).normalize_or_zero()-eye.normalize()).length_squared();
            dist(a).total_cmp(&dist(b))
        })?;
        let origin=eye.normalize()*DVec3::from_array(ground.position).length();
        let east=up.any_orthonormal_vector();let north=up.cross(east);
        let x_axis=east-up*(sun.dot(east)/altitude.max(0.25));
        let z_axis=north-up*(sun.dot(north)/altitude.max(0.25));
        let side=512u32;let step=1.0;let band=8.0;
        let half=side as f32*step*0.5;
        let mut pixels=vec![0u8;side as usize*side as usize*4];
        let level=LOCAL_WIDTHS.iter().position(|&w|w==1.0)?;
        for input in sites{
            let Some(t)=Tree::new(input,pack) else{continue;};
            let rel=(t.origin-origin).as_vec3();
            let range=pack.templates[&input.model].info.ranges[level];
            if range.offset&RAW!=0{continue;}
            let start=range.offset as usize;
            let Some(records)=pack.lod.get(start..start+range.count as usize)else{continue;};
            for &v in records{
                let p=Point{xy:v[0],zr:v[1],rgba:v[2],normal:v[3]};
                let pos=rel+t.rotation*(point_position(&p)*t.scale);
                let height=pos.dot(up);
                let coverage=(p.rgba>>24) as f32/255.0;
                if coverage<=0.0{continue;}
                let x=pos.dot(x_axis)+half;let z=pos.dot(z_axis)+half;
                let radius=(point_radius(&p)*t.scale).min(2.5);
                if x < -5.0||z < -5.0||x > side as f32*step+5.0||z > side as f32*step+5.0{continue;}
                raster_shadow_sample(&mut pixels,side,side,step,x,z,height/band,radius,coverage);
            }
        }
        Some(Self{side,pixels,center:eye.to_array(),sun:sun.to_array(),origin,x_axis,z_axis,up,step,band})
    }
    /// Camera-relative affine UV and height coordinates. No large f32
    /// absolute planet coordinates are sent to the shader.
    pub fn uniform(&self,eye:[f64;3])->[[f32;4];3]{
        let d=(DVec3::from_array(eye)-self.origin).as_vec3();
        let inv=1.0/(self.side as f32*self.step);
        [(self.x_axis*inv).extend(0.5+d.dot(self.x_axis)*inv).to_array(),
         (self.z_axis*inv).extend(0.5+d.dot(self.z_axis)*inv).to_array(),
         (self.up/self.band).extend(d.dot(self.up)/self.band).to_array()]
    }
}
