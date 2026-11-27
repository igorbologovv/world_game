//! Sparse spherical-world adapter to the original point hierarchy.
//! Same weighted aggregates, 4x parent reduction, local LODs, hysteresis
//! and budget policy as the demo. No Bevy types or GPU device dependencies.
#[path = "planetary_selection/traversal.rs"]
mod traversal;

use super::{Aggregate,Point,Sample,raster_shadow_sample,point_position,point_radius,LOCAL_WIDTHS,ORIGINAL_LEVELS,RAW};
#[cfg(test)]
use super::Range;
use crate::{planetary::Pack,render_api::LANES};
use glam::{DVec3,Vec3,Quat};
use std::{cmp::Ordering, collections::{BTreeMap,HashMap}, sync::Arc};
use tracing::{trace, trace_span};
use traversal::{SelectionScratch, Totals};

/// Unmodified game-world placement, before the loader normalization.
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct Placement{
    pub position:[f64;3],pub rotation:[f32;4],pub scale:f32,pub model:usize,pub phase:f32,
}

impl Placement {
    /// Stable key of the 16 m cell containing the untransformed placement.
    pub fn leaf_key(&self) -> LeafKey {
        cell(DVec3::from_array(self.position), WIDTHS[0])
    }

    /// Order placements by world position and then by model.
    pub fn spatial_cmp(&self, other: &Self) -> Ordering {
        self.position[0].total_cmp(&other.position[0])
            .then_with(|| self.position[1].total_cmp(&other.position[1]))
            .then_with(|| self.position[2].total_cmp(&other.position[2]))
            .then_with(|| self.model.cmp(&other.model))
    }
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
/// Signed world-grid coordinates of a 16 m leaf cell.
pub type LeafKey = [i64; 3];
type Cell = LeafKey;
type Identity=[u64;3];
const WIDTHS:[f64;5]=[16.0,64.0,256.0,1024.0,4096.0];
fn cell(p:DVec3,width:f64)->Cell{(p/width).to_array().map(|v|v.floor() as i64)}
fn anchor(k:Cell,width:f64)->DVec3{DVec3::new(k[0] as f64,k[1] as f64,k[2] as f64)*width}
fn identity(p:Placement)->Identity{p.position.map(f64::to_bits)}
fn parent(k:Cell)->Cell{k.map(|v|v.div_euclid(4))}
#[derive(Clone,Copy)]
struct Tree{
    input:Placement,origin:DVec3,rotation:Quat,scale:f32,center:DVec3,radius:f32,
}
impl Tree{
    fn new(input:Placement,pack:&Pack)->Option<(Self, Sample)>{
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
        Some((Self{input,origin,rotation,scale,center,radius}, summary))
    }
}
enum Members {
    Trees(Box<[Tree]>),
    Children(Box<[Arc<Node>]>),
}

struct Node{
    key:Cell,anchor:DVec3,lo:DVec3,hi:DVec3,aggregate:Aggregate,
    members:Members,count:u32,
}
impl Node{
    fn new(key:Cell,width:f64,members:Members)->Self{
        Self{key,anchor:anchor(key,width),lo:DVec3::splat(f64::INFINITY),hi:DVec3::splat(f64::NEG_INFINITY),
            aggregate:Aggregate::default(),members,count:0}
    }
    fn bounds(&mut self,lo:DVec3,hi:DVec3){self.lo=self.lo.min(lo);self.hi=self.hi.max(hi);}
    fn add_tree(&mut self,t:Tree,summary:Sample){
        self.count+=1;
        let r=DVec3::splat(f64::from(t.radius));
        self.bounds(t.center-r,t.center+r);
        if summary.coverage>0.0{
            let mut s=summary;
            s.pos+=(t.origin-self.anchor).as_vec3();
            self.aggregate.add(s);
        }
    }
    fn add_child(&mut self,c:&Node){
        self.count+=c.count;self.bounds(c.lo,c.hi);
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

    fn leaf(key: Cell, trees: impl Iterator<Item = (Tree, Sample)>) -> Option<Arc<Self>> {
        let mut node = Self::new(key, WIDTHS[0], Members::Trees(Box::default()));
        let trees: Vec<_> = trees.map(|(tree, summary)| {
            node.add_tree(tree, summary);
            tree
        }).collect();
        if trees.is_empty() {
            return None;
        }
        node.members = Members::Trees(trees.into_boxed_slice());
        Some(Arc::new(node))
    }

    fn branch(key: Cell, level: usize, children: Vec<Arc<Self>>) -> Option<Arc<Self>> {
        if children.is_empty() {
            return None;
        }
        let mut node = Self::new(key, WIDTHS[level], Members::Children(Box::default()));
        for child in &children {
            node.add_child(child);
        }
        node.members = Members::Children(children.into_boxed_slice());
        Some(Arc::new(node))
    }

    fn children(&self) -> &[Arc<Self>] {
        match &self.members {
            Members::Children(children) => children,
            Members::Trees(_) => &[],
        }
    }

    fn shadow_placements(&self, eye: DVec3, placements: &mut Vec<Placement>) {
        match &self.members {
            Members::Trees(trees) => placements.extend(trees.iter()
                .filter(|tree| tree.center.distance_squared(eye) < 500.0f64.powi(2))
                .map(|tree| tree.input)),
            Members::Children(children) => {
                for child in children.iter() {
                    child.shadow_placements(eye, placements);
                }
            }
        }
    }
}
#[derive(Default)]
struct History{
    nodes:HashMap<(usize,Cell),u8>,trees:HashMap<Identity,u8>,
}
struct Item{
    origin:[f32;4],rotation:[f32;4],range:[u32;4],count:u32,fade:u32,level:usize,model:usize,
}
/// Host-provided scoped execution for independent selection workloads.
pub trait SelectionExecutor {
    /// Maximum simultaneous workloads, including one on the calling thread if used.
    fn worker_count(&self) -> usize;

    /// Run every workload exactly once and return after all writes are complete.
    /// Workloads may run in any order. Implementations may borrow the slice and callback.
    fn for_each<T: Send>(&self, workloads: &mut [T], run: impl Fn(&mut T) + Sync);
}

struct SerialSelection;

impl SelectionExecutor for SerialSelection {
    fn worker_count(&self) -> usize {
        1
    }

    fn for_each<T: Send>(&self, workloads: &mut [T], run: impl Fn(&mut T) + Sync) {
        workloads.iter_mut().for_each(run);
    }
}

/// GPU word arrays match the existing planetary shader ABI: 48 byte
/// instance, 16 byte job. A cluster stores its color/normal inline.
#[derive(Default)]
pub struct PlanetSelection{
    pub instances:Vec<[u32;12]>,pub jobs:Vec<[u32;4]>,pub points:u32,pub raw_points:u32,
    pub covered:u32,pub clusters:u32,pub visited:u32,pub threshold:f32,pub budget_met:bool,
    pub lod_counts:[u32;22],
    /// Maximum jobs allowed for this selection, including padding.
    pub job_limit: u64,
    /// Rejections during this selection, including a failed terminal probe.
    pub probe_rejections: ProbeRejections,
}

/// Mutually exclusive counts of rejected probes in one selection.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ProbeRejections {
    /// Probes over the hard point limit alone.
    pub points: u32,
    /// Probes over the hard job limit alone.
    pub jobs: u32,
    /// Probes over both hard limits.
    pub both: u32,
    /// Refinement probes within both hard limits but over 85% of the point budget.
    pub refinement_headroom: u32,
}

impl ProbeRejections {
    fn record(&mut self, threshold: f32, work: Totals, point_limit: u64, job_limit: u64) {
        let (count, reason) = match (work.points > point_limit, work.jobs > job_limit) {
            (true, true) => (&mut self.both, "points_and_jobs"),
            (true, false) => (&mut self.points, "point_limit"),
            (false, true) => (&mut self.jobs, "job_limit"),
            (false, false) => (&mut self.refinement_headroom, "refinement_headroom"),
        };
        *count += 1;
        trace!(threshold, points = work.points, jobs = work.jobs, point_limit, job_limit, reason,
            "Point forest budget probe rejected");
    }
}
/// Immutable spatial blocks and hierarchy nodes built against one geometry pack.
#[derive(Default)]
pub struct PlanetHierarchy {
    roots: Vec<Arc<Node>>,
    blocks: usize,
    // Retained only by the synchronous placement-list API.
    placements: Option<Vec<Placement>>,
}

impl PlanetHierarchy {
    /// Build from a complete placement list. Order within a leaf follows the input.
    /// Use the same pack for selection and shadow baking.
    pub fn build(pack: &Pack, placements: Vec<Placement>) -> Self {
        let mut cells = BTreeMap::<LeafKey, Vec<Placement>>::new();
        for &placement in &placements {
            cells.entry(placement.leaf_key()).or_default().push(placement);
        }
        let mut hierarchy = Self::build_cells(pack, cells);
        hierarchy.placements = Some(placements);
        hierarchy
    }

    /// Build from complete, ordered placement lists for distinct 16 m cells.
    pub fn build_cells(
        pack: &Pack,
        cells: impl IntoIterator<Item = (LeafKey, Vec<Placement>)>,
    ) -> Self {
        let _span = trace_span!("PlanetHierarchy::full_rebuild").entered();
        Self::default().replace_cells(pack, cells, true).unwrap_or_default()
    }

    /// Replace complete placement lists for distinct dirty cells, sharing all
    /// unchanged blocks and branches. An empty list removes the cell. Returns
    /// `None` when the renderable contents of all supplied cells are unchanged.
    /// The pack must be the same one used to build this hierarchy.
    pub fn update_cells(
        &self,
        pack: &Pack,
        cells: impl IntoIterator<Item = (LeafKey, Vec<Placement>)>,
    ) -> Option<Self> {
        let _span = trace_span!("PlanetHierarchy::incremental_update").entered();
        self.replace_cells(pack, cells, false)
    }

    fn replace_cells(
        &self,
        pack: &Pack,
        cells: impl IntoIterator<Item = (LeafKey, Vec<Placement>)>,
        full: bool,
    ) -> Option<Self> {
        let mut blocks = self.blocks;
        let mut rebuilt_blocks = 0;
        let mut removed_blocks = 0;
        let mut changes = BTreeMap::new();
        {
            let _span = trace_span!("PlanetScene::build_leaf_level").entered();
            for (key, placements) in cells {
                let old = self.node(0, key);
                let old_trees = match old.map(|node| &node.members) {
                    Some(Members::Trees(trees)) => trees.as_ref(),
                    _ => &[],
                };
                if old_trees.iter().map(|tree| tree.input).eq(placements.iter().copied()) {
                    continue;
                }
                let leaf = Node::leaf(key, placements.into_iter().filter_map(|p| Tree::new(p, pack)));
                let new_trees = match leaf.as_ref().map(|node| &node.members) {
                    Some(Members::Trees(trees)) => trees.as_ref(),
                    _ => &[],
                };
                // Unavailable templates and invalid scales may leave this cell unchanged.
                if old_trees.iter().map(|tree| tree.input).eq(new_trees.iter().map(|tree| tree.input)) {
                    continue;
                }
                if leaf.is_some() {
                    rebuilt_blocks += 1;
                    blocks += usize::from(old.is_none());
                } else {
                    removed_blocks += 1;
                    blocks -= 1;
                }
                changes.insert(key, leaf);
            }
        }

        let mut updated_ancestors = 0;
        let updated = if changes.is_empty() {
            None
        } else {
            let _span = trace_span!("PlanetScene::build_parent_levels").entered();
            for level in 1..WIDTHS.len() {
                let mut parents = BTreeMap::<_, Vec<_>>::new();
                for (key, node) in changes {
                    parents.entry(parent(key)).or_default().push((key, node));
                }
                updated_ancestors += parents.len();
                changes = parents.into_iter().map(|(key, updates)| {
                    let old = self.node(level, key).map_or(&[][..], Node::children);
                    (key, Node::branch(key, level, merge_children(old, updates)))
                }).collect();
            }
            Some(Self { roots: merge_children(&self.roots, changes), blocks, placements: None })
        };
        trace!(
            rebuilt_blocks,
            reused_blocks = blocks - rebuilt_blocks,
            removed_blocks,
            updated_ancestors,
            full_rebuilds = usize::from(full),
            "Point forest hierarchy work"
        );
        updated
    }

    fn node(&self, level: usize, key: Cell) -> Option<&Node> {
        let mut keys = [key; WIDTHS.len()];
        for depth in level + 1..WIDTHS.len() {
            keys[depth] = parent(keys[depth - 1]);
        }
        let mut siblings = &self.roots[..];
        for depth in (level..WIDTHS.len()).rev() {
            let index = siblings.binary_search_by_key(&keys[depth], |node| node.key).ok()?;
            let node = &siblings[index];
            if depth == level {
                return Some(node);
            }
            siblings = node.children();
        }
        None
    }

    /// Compare inputs supplied through the complete placement-list API.
    pub fn has_placements(&self, placements: &[Placement]) -> bool {
        self.placements.as_deref() == Some(placements)
    }

    /// Off-screen placements are included; selection and camera LOD do not affect shadows.
    pub fn shadow_placements(&self, eye: [f64; 3]) -> Vec<Placement> {
        let _span = trace_span!("PlanetScene::shadow_placements").entered();
        let eye = DVec3::from_array(eye);
        let mut placements = Vec::new();
        for root in &self.roots {
            root.shadow_placements(eye, &mut placements);
        }
        // Shadow baking previously consumed world-position order from the game.
        placements.sort_by(Placement::spatial_cmp);
        placements
    }
}

// Changes arrive in key order. Each surviving old child only needs one Arc clone.
fn merge_children(
    old: &[Arc<Node>],
    changes: impl IntoIterator<Item = (Cell, Option<Arc<Node>>)>,
) -> Vec<Arc<Node>> {
    let changes = changes.into_iter();
    let mut children = Vec::with_capacity(old.len().max(changes.size_hint().0));
    let mut old = old.iter().peekable();
    for (key, replacement) in changes {
        while old.peek().is_some_and(|node| node.key < key) {
            children.push(old.next().unwrap().clone());
        }
        if old.peek().is_some_and(|node| node.key == key) {
            old.next();
        }
        children.extend(replacement);
    }
    children.extend(old.cloned());
    children
}

/// Reuse the selector across frames and hierarchy swaps to preserve LOD history.
#[derive(Default)]
pub struct PlanetScene{
    hierarchy: Arc<PlanetHierarchy>,
    history:History,scratch:SelectionScratch,threshold:f32,base:f32,budget:u64,max_jobs:u64,spare:u32,
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

    /// Select visible points using the pack used to build the installed hierarchy.
    pub fn select(&mut self,pack:&Pack,cam:Camera,base:f32,budget:u64,max_jobs:u64)->PlanetSelection{
        self.select_with_executor(pack, cam, base, budget, max_jobs, &SerialSelection)
    }

    /// Select using the host's scoped executor. Probes share the previous LOD
    /// history and only the accepted probe commits its updates.
    pub fn select_with_executor(
        &mut self,
        pack: &Pack,
        cam: Camera,
        base: f32,
        budget: u64,
        max_jobs: u64,
        executor: &impl SelectionExecutor,
    ) -> PlanetSelection {
        let _span = trace_span!("PlanetScene::select").entered();
        let base=base.clamp(0.25,512.0);
        if self.base!=base||self.budget!=budget||self.max_jobs!=max_jobs{
            self.base=base;self.budget=budget;self.max_jobs=max_jobs;self.threshold=base;self.spare=0;
            self.history.nodes.clear();
            self.history.trees.clear();
        }
        let mut probe = |threshold| {
            self.scratch.probe(&self.hierarchy, &self.history, pack, cam, threshold, executor)
        };
        let current=self.threshold.max(base);
        let dense=pack.levels.len()>ORIGINAL_LEVELS.len();
        let mut threshold=current;
        let mut tried=false;
        let mut accepted=None;
        let mut probe_rejections = ProbeRejections::default();
        // Identical refinement cadence and hysteresis acceptance to demo.
        if self.spare>=12&&current>base{
            tried=true;
            let candidate=(current/if dense{1.08}else{1.125}).max(base);
            let work=probe(candidate);
            if work.fits(budget,max_jobs)&&work.points<=budget.saturating_mul(85)/100{
                threshold=candidate;accepted=Some(work);
            } else {
                probe_rejections.record(candidate, work, budget, max_jobs);
            }
        }
        let work=accepted.unwrap_or_else(||loop{
            let work=probe(threshold);
            if work.fits(budget,max_jobs){break work;}
            probe_rejections.record(threshold, work, budget, max_jobs);
            if threshold>=512.0{break work;}
            threshold=(threshold*if dense{1.15}else{1.25}).min(512.0);
        });
        let fits=work.fits(budget,max_jobs);
        self.spare=if !tried&&fits&&threshold==current&&work.points<=budget.saturating_mul(75)/100{
            self.spare.saturating_add(1)
        }else{0};
        self.threshold=threshold;
        if !fits{
            // Never send an over-budget dispatch or silently drop trees.
            return PlanetSelection{threshold,budget_met:false,visited:work.visited,
                job_limit:max_jobs,probe_rejections,..Default::default()};
        }
        let items = self.scratch.commit(&mut self.history);
        // Same demo ordering: fine levels first, model next, world last.
        items.sort_by_key(|i|(i.level,i.model));
        let _span = trace_span!("PlanetScene::generate_instances_and_jobs").entered();
        let mut out=PlanetSelection{instances:Vec::with_capacity(items.len()),jobs:Vec::with_capacity(work.jobs as usize),
            points:work.points as u32,raw_points:work.raw as u32,covered:work.covered,clusters:work.clusters,visited:work.visited,threshold,budget_met:true,lod_counts:[0;22],
            job_limit:max_jobs,probe_rejections};
        for item in items.drain(..){
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

    struct TestExecutor {
        workers: usize,
        largest_dispatch: std::cell::Cell<usize>,
    }

    impl SelectionExecutor for TestExecutor {
        fn worker_count(&self) -> usize {
            self.workers
        }

        fn for_each<T: Send>(&self, workloads: &mut [T], run: impl Fn(&mut T) + Sync) {
            self.largest_dispatch.set(self.largest_dispatch.get().max(workloads.len()));
            std::thread::scope(|scope| {
                let run = &run;
                // Start in reverse order to exercise ordered merging independently of scheduling.
                for workload in workloads.iter_mut().rev() {
                    scope.spawn(move || run(workload));
                }
            });
        }
    }

    fn dense_hierarchy(pack: &Pack) -> Arc<PlanetHierarchy> {
        let mut placements = Vec::new();
        for x in 0..64 {
            for z in 0..64 {
                let mut site = placement(20.0 + f64::from(x) * 4.0, 100.0 + f64::from(z) * 4.0);
                site.phase = (x * 64 + z) as f32;
                placements.push(site);
            }
        }
        Arc::new(PlanetHierarchy::build(pack, placements))
    }

    fn assert_same_selection(serial: &PlanetSelection, parallel: &PlanetSelection) {
        assert_eq!(serial.instances, parallel.instances);
        assert_eq!(serial.jobs, parallel.jobs);
        assert_eq!(serial.points, parallel.points);
        assert_eq!(serial.raw_points, parallel.raw_points);
        assert_eq!(serial.covered, parallel.covered);
        assert_eq!(serial.clusters, parallel.clusters);
        assert_eq!(serial.visited, parallel.visited);
        assert_eq!(serial.threshold.to_bits(), parallel.threshold.to_bits());
        assert_eq!(serial.budget_met, parallel.budget_met);
        assert_eq!(serial.lod_counts, parallel.lod_counts);
        assert_eq!(serial.job_limit, parallel.job_limit);
        assert_eq!(serial.probe_rejections, parallel.probe_rejections);
    }

    #[test]
    fn probe_diagnostics_report_hard_limits_and_refinement_headroom() {
        let pack = pack();
        let mut scene = PlanetScene::default();
        scene.update_sites(&pack, vec![placement(0.0, 20.0)]);
        for (points, jobs) in [(0, 2), (140, 0), (0, 0)] {
            let out = scene.select(&pack, camera(), 2.2, points, jobs);
            assert!(!out.budget_met);
            assert_eq!(out.threshold, 512.0);
            assert_eq!(out.job_limit, jobs);
            assert!(out.jobs.is_empty());
            let rejections = out.probe_rejections;
            assert_eq!(rejections.points > 0, points == 0 && jobs != 0);
            assert_eq!(rejections.jobs > 0, jobs == 0 && points != 0);
            assert_eq!(rejections.both > 0, points == 0 && jobs == 0);
            assert_eq!(rejections.refinement_headroom, 0);
        }

        let first = scene.select(&pack, camera(), 2.2, 140, 2);
        assert_eq!(first.points, 128);
        assert_eq!(first.probe_rejections, ProbeRejections::default());
        scene.threshold = 3.0;
        scene.spare = 12;
        let out = scene.select(&pack, camera(), 2.2, 140, 2);
        assert!(out.budget_met);
        assert_eq!(out.threshold, 3.0);
        assert_eq!(out.points, 128);
        assert_eq!(out.jobs.len(), 2);
        assert_eq!(out.probe_rejections, ProbeRejections {
            refinement_headroom: 1,
            ..Default::default()
        });
    }

    #[test]
    fn parallel_selection_matches_serial_with_one_dense_root_and_budget_retries() {
        let pack = pack();
        let hierarchy = dense_hierarchy(&pack);
        assert_eq!(hierarchy.roots.len(), 1);
        assert_eq!(hierarchy.roots[0].children().len(), 1);
        let edited_hierarchy = Arc::new(hierarchy.update_cells(
            &pack, [(placement(20.0, 100.0).leaf_key(), vec![])],
        ).unwrap());
        let mut views = vec![
            ([148.0, 2.0, -160.0], 1_000_000, 200_000),
            ([180.0, 3.0, -360.0], 1_000_000, 200_000),
            ([216.0, 2.0, -650.0], 1_000_000, 200_000),
            ([148.0, 2.0, -160.0], 4_000, 2_000),
            ([85.0, 2.0, -1000.0], 4_000, 2_000),
        ];
        // Exercise spare-capacity hysteresis across multiple frames and then a job-limited probe.
        views.extend(std::iter::repeat_n(([148.0, 2.0, -1500.0], 4_000, 2_000), 16));
        views.extend([
            ([148.0, 2.0, -160.0], 1_000_000, 64),
            ([148.0, 2.0, -160.0], 0, 0),
        ]);
        for workers in [1, 2, 4, 8] {
            let executor = TestExecutor { workers, largest_dispatch: std::cell::Cell::new(0) };
            let mut serial = PlanetScene::default();
            let mut parallel = PlanetScene::default();
            serial.replace_hierarchy(hierarchy.clone());
            parallel.replace_hierarchy(hierarchy.clone());
            let mut earlier_lods = None;
            let mut changed_lods = false;
            let mut tightened_threshold = false;
            for (frame, &(position, budget, jobs)) in views.iter().enumerate() {
                if frame == 2 {
                    serial.replace_hierarchy(edited_hierarchy.clone());
                    parallel.replace_hierarchy(edited_hierarchy.clone());
                }
                let cam = Camera { position, ..camera() };
                let reference = serial.select(&pack, cam, 2.2, budget, jobs);
                let selected = parallel.select_with_executor(&pack, cam, 2.2, budget, jobs, &executor);
                assert_same_selection(&reference, &selected);
                assert_eq!(serial.history.nodes, parallel.history.nodes);
                assert_eq!(serial.history.trees, parallel.history.trees);
                assert_eq!(serial.spare, parallel.spare);
                if budget == 1_000_000 && jobs == 200_000 {
                    changed_lods |= earlier_lods.is_some_and(|earlier| earlier != selected.lod_counts);
                    earlier_lods = Some(selected.lod_counts);
                }
                tightened_threshold |= budget == 4_000 && selected.threshold > 2.2;
                if selected.budget_met {
                    assert!(u64::from(selected.points) <= budget);
                    assert!(selected.jobs.len() as u64 <= jobs);
                }
                if budget == 0 {
                    assert!(!selected.budget_met);
                    assert!(parallel.history.nodes.is_empty() && parallel.history.trees.is_empty());
                }
            }
            assert!(changed_lods && tightened_threshold);
            assert_eq!(executor.largest_dispatch.get(), if workers == 1 { 0 } else { workers });
        }
    }

    #[test]
    fn parallel_selection_falls_back_for_small_and_culled_workloads() {
        let pack = pack();
        let mut scene = PlanetScene::default();
        let executor = TestExecutor { workers: 8, largest_dispatch: std::cell::Cell::new(0) };
        scene.update_sites(&pack, vec![placement(0.0, 30.0)]);
        let near = scene.select_with_executor(&pack, camera(), 2.2, 1000, 1000, &executor);
        assert_eq!(near.covered, 1);
        scene.replace_hierarchy(dense_hierarchy(&pack));
        let culled = Camera { forward: [0.0, 0.0, -1.0], ..camera() };
        let far = scene.select_with_executor(&pack, culled, 2.2, 1000, 1000, &executor);
        assert_eq!(far.covered, 0);
        assert_eq!(far.visited, 1);
        assert_eq!(executor.largest_dispatch.get(), 0);
    }

    #[test]fn hierarchy_reuses_static_placement_and_covers_once(){
        let pack=pack();let mut scene=PlanetScene::default();
        let sites=vec![placement(0.0,20.0),placement(-2.0,1000.0),placement(-3.0,1001.0)];
        assert!(scene.update_sites(&pack,sites.clone()));
        assert!(!scene.update_sites(&pack,sites));
        assert_eq!(scene.hierarchy.roots.iter().map(|n|n.count).sum::<u32>(),3);
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

    #[test]
    fn incremental_cells_match_full_reconstruction_and_share_untouched_paths() {
        let pack = pack();
        let changed = placement(-80.0, 78.0);
        let removed = placement(-4140.0, 80.0);
        let nearby = placement(-76.0, 90.0);
        let untouched = placement(4200.0, 80.0);
        let distant = placement(50.0, 1200.0);
        let original = PlanetHierarchy::build(
            &pack, vec![changed, removed, nearby, untouched, distant],
        );
        let replacement = Placement { scale: 1.5, phase: 0.3, ..changed };
        let added = placement(-16.0, 400.0);
        let incremental = original.update_cells(&pack, [
            (changed.leaf_key(), vec![replacement]),
            (removed.leaf_key(), vec![]),
            (added.leaf_key(), vec![added]),
            ([1000; 3], vec![]),
        ]).unwrap();
        assert_eq!(incremental.blocks, 5);
        assert!(incremental.node(0, removed.leaf_key()).is_none());
        let removed_root = (1..WIDTHS.len()).fold(removed.leaf_key(), |key, _| parent(key));
        assert!(incremental.node(WIDTHS.len() - 1, removed_root).is_none());
        for placement in [nearby, untouched, distant] {
            assert!(std::ptr::eq(
                original.node(0, placement.leaf_key()).unwrap(),
                incremental.node(0, placement.leaf_key()).unwrap(),
            ));
        }
        let untouched_root = (1..WIDTHS.len()).fold(untouched.leaf_key(), |key, _| parent(key));
        assert!(std::ptr::eq(
            original.node(WIDTHS.len() - 1, untouched_root).unwrap(),
            incremental.node(WIDTHS.len() - 1, untouched_root).unwrap(),
        ));
        assert!(incremental.update_cells(&pack, [
            (replacement.leaf_key(), vec![replacement]),
            ([1000; 3], vec![]),
        ]).is_none());

        let full = PlanetHierarchy::build(&pack, vec![replacement, nearby, added, distant, untouched]);
        assert_eq!(incremental.shadow_placements([0.0; 3]), full.shadow_placements([0.0; 3]));
        let mut edited_scene = PlanetScene::default();
        let mut full_scene = PlanetScene::default();
        edited_scene.replace_hierarchy(Arc::new(incremental));
        full_scene.replace_hierarchy(Arc::new(full));
        for (position, budget) in [([0.0, 2.0, 0.0], 1000), ([-14.0, 1.0, 8.0], 10)] {
            let view = Camera { position, ..camera() };
            let edited = edited_scene.select(&pack, view, 2.2, budget, 1000);
            let rebuilt = full_scene.select(&pack, view, 2.2, budget, 1000);
            assert!(edited.covered > 0);
            assert_eq!(edited.instances, rebuilt.instances);
            assert_eq!(edited.jobs, rebuilt.jobs);
            assert_eq!(edited.lod_counts, rebuilt.lod_counts);
            assert_eq!((edited.covered, edited.visited, edited.threshold, edited.budget_met),
                (rebuilt.covered, rebuilt.visited, rebuilt.threshold, rebuilt.budget_met));
        }
    }

    #[test]
    fn removing_last_cell_produces_an_empty_hierarchy() {
        let pack = pack();
        let site = placement(0.0, 20.0);
        let full = PlanetHierarchy::build(&pack, vec![site]);
        let empty = full.update_cells(&pack, [(site.leaf_key(), vec![])]).unwrap();
        assert_eq!(empty.blocks, 0);
        assert!(empty.roots.is_empty());
        let mut scene = PlanetScene::default();
        scene.replace_hierarchy(Arc::new(empty));
        let selection = scene.select(&pack, camera(), 2.2, 100, 100);
        assert!(selection.budget_met);
        assert!(selection.instances.is_empty() && selection.jobs.is_empty());
    }

    #[test]fn translated_parent_preserves_weighted_summary(){
        let mut child=Node::new([-1,0,0],16.0,Members::Trees(Box::default()));
        child.aggregate.add(Sample{pos:Vec3::new(15.0,1.0,2.0),color:Vec3::ONE,normal:Vec3::Y,radius:1.0,coverage:1.0});
        child.lo=child.anchor;child.hi=child.anchor+DVec3::splat(16.0);
        let mut p=Node::new([-1,0,0],64.0,Members::Children(Box::default()));p.add_child(&child);
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
            let Some((t,_))=Tree::new(input,pack) else{continue;};
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
