mod placements;

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use bevy::{
    log::{error, info, trace, trace_span},
    tasks::{AsyncComputeTaskPool, Task},
};
use igor_renderer::planetary::{
    Camera, Pack, PlanetHierarchy, PlanetScene, PlanetSelection, SelectionExecutor,
};

use super::super::{
    impostor::Portraits,
    streaming::{ForestStream, PointRevision, PointSiteChanges},
    Species,
};
use placements::PlacementIndex;

type Templates = BTreeMap<usize, (Species, usize)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SourceRevision {
    points: PointRevision,
    models: u64,
}

impl SourceRevision {
    fn same_generation(self, other: Self) -> bool {
        self.points.construction == other.points.construction && self.models == other.models
    }
}

// The render world can own the last reference after extraction. Move the large
// CPU arrays to a worker even when that is where the final release happens.
pub(super) struct Geometry {
    pub(super) id: u64,
    models: u64,
    pack: Option<Pack>,
}

impl Geometry {
    pub(super) fn pack(&self) -> &Pack {
        self.pack.as_ref().unwrap()
    }

    fn covers(&self, templates: &Templates) -> bool {
        templates
            .keys()
            .all(|tile| self.pack().templates.contains_key(tile))
    }
}

impl Drop for Geometry {
    fn drop(&mut self) {
        let pack = self.pack.take();
        AsyncComputeTaskPool::get()
            .spawn(async move {
                let _span = trace_span!("point_forest.drop_geometry_pack").entered();
                drop(pack);
            })
            .detach();
    }
}

#[derive(Clone)]
pub(super) struct Snapshot {
    revision: SourceRevision,
    pub(super) geometry: Arc<Geometry>,
    pub(super) hierarchy: Arc<PlanetHierarchy>,
    pub(super) candidates: usize,
}

impl Snapshot {
    pub(super) fn construction_revision(&self) -> u64 {
        self.revision.points.construction
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct BuildKey {
    revision: SourceRevision,
    geometry: Option<u64>,
}

struct TemplateDemand {
    revision: SourceRevision,
    templates: Templates,
}

struct BuildResult {
    demand: TemplateDemand,
    snapshot: Option<Snapshot>,
    placements: PlacementIndex,
}

struct GeometryResult {
    models: u64,
    pack: Result<Arc<Geometry>, String>,
}

#[derive(Default)]
pub(super) struct Snapshots {
    active: Option<Snapshot>,
    scene: PlanetScene,
    hierarchy_task: Option<Task<BuildResult>>,
    last_build: Option<BuildKey>,
    placements: PlacementIndex,
    demand: Option<TemplateDemand>,
    geometry_task: Option<Task<GeometryResult>>,
    prepared_geometry: Option<Arc<Geometry>>,
    last_geometry_request: Option<(u64, Vec<usize>)>,
    next_geometry_id: u64,
    next_geometry_build: f64,
    retirement: Option<Task<()>>,
    pub(super) geometry_error: Option<String>,
}

impl Snapshots {
    pub(super) fn active(&self) -> Option<&Snapshot> {
        self.active.as_ref()
    }

    pub(super) fn baking_geometry(&self) -> bool {
        self.geometry_task.is_some()
    }

    pub(super) fn rebuilding(&self) -> bool {
        self.hierarchy_task.is_some()
    }

    pub(super) fn select(
        &mut self,
        camera: Camera,
        base: f32,
        budget: u64,
        max_jobs: u64,
        executor: &impl SelectionExecutor,
    ) -> PlanetSelection {
        let snapshot = self
            .active
            .as_ref()
            .expect("select requires a published snapshot");
        self.scene
            .select_with_executor(snapshot.geometry.pack(), camera, base, budget, max_jobs, executor)
    }

    pub(super) fn update(
        &mut self,
        stream: &mut ForestStream,
        portraits: &Portraits,
        models: u64,
        now: f64,
        limits: (u64, u32),
    ) -> bool {
        let revision = SourceRevision {
            points: stream.point_revision(),
            models,
        };
        self.receive_geometry(models);
        let changed = self.receive_hierarchy(revision);
        self.request_geometry(revision, portraits, now, limits);
        self.request_hierarchy(revision, stream, portraits);
        changed
    }

    fn receive_geometry(&mut self, models: u64) {
        if self
            .prepared_geometry
            .as_ref()
            .is_some_and(|pack| pack.models != models)
        {
            self.prepared_geometry = None;
            self.geometry_error = None;
        }
        let Some(result) = take_ready(&mut self.geometry_task) else {
            return;
        };
        if result.models != models {
            return;
        }
        match result.pack {
            Ok(geometry) => {
                let pack = geometry.pack();
                info!(
                    "Original Igor pack: {} templates, {} raw, {} LOD, {} patches, {} atlas pages",
                    pack.templates.len(),
                    pack.raw.len(),
                    pack.lod.len(),
                    pack.patches.len(),
                    pack.atlas.layers
                );
                self.prepared_geometry = Some(geometry);
                self.geometry_error = None;
            }
            Err(error) => {
                error!("Original Igor pack: {error}");
                self.geometry_error = Some(error);
            }
        }
    }

    fn receive_hierarchy(&mut self, revision: SourceRevision) -> bool {
        // Backpressure also bounds retired hierarchies when the worker pool is busy.
        take_ready(&mut self.retirement);
        if self.retirement.is_some() {
            return false;
        }
        let Some(result) = take_ready(&mut self.hierarchy_task) else {
            return false;
        };
        // This index travelled with the one in-flight job. If its generation is
        // obsolete, the next worker replaces it from a complete stream capture.
        self.placements = result.placements;
        if !result.demand.revision.same_generation(revision) {
            trace!(
                point_revision = result.demand.revision.points.sites,
                "Discarding forest hierarchy from an obsolete generation"
            );
            if let Some(snapshot) = result.snapshot {
                self.retire(snapshot);
            }
            return false;
        }
        self.demand = Some(result.demand);
        // A completed stream revision can make progress while patches are still
        // arriving. The next task captures the latest revision, with no queue.
        result
            .snapshot
            .is_some_and(|snapshot| self.publish(snapshot))
    }

    fn publish(&mut self, snapshot: Snapshot) -> bool {
        let old = trace_span!("point_forest.publish_snapshot").in_scope(|| {
            trace!(
                point_revision = snapshot.revision.points.sites,
                construction_revision = snapshot.revision.points.construction,
                geometry = snapshot.geometry.id,
                "Publishing point forest hierarchy"
            );
            let changed = self.active.as_ref().is_none_or(|old| {
                old.geometry.id != snapshot.geometry.id
                    || !Arc::ptr_eq(&old.hierarchy, &snapshot.hierarchy)
                    || old.construction_revision() != snapshot.construction_revision()
            });
            if !changed {
                self.active = Some(snapshot);
                return None;
            }
            let hierarchy = self.scene.replace_hierarchy(snapshot.hierarchy.clone());
            let snapshot = self.active.replace(snapshot);
            Some((hierarchy, snapshot))
        });
        if let Some(old) = old {
            self.retire(old);
            true
        } else {
            false
        }
    }

    fn retire(&mut self, value: impl Send + 'static) {
        self.retirement = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _span = trace_span!("point_forest.drop_hierarchy").entered();
            drop(value);
        }));
    }

    fn request_hierarchy(
        &mut self,
        revision: SourceRevision,
        stream: &mut ForestStream,
        portraits: &Portraits,
    ) {
        if self.hierarchy_task.is_some() {
            return;
        }
        let key = BuildKey {
            revision,
            geometry: self.prepared_geometry.as_ref().map(|geometry| geometry.id),
        };
        if self.last_build == Some(key) {
            return;
        }
        let previous = self.last_build.map(|build| build.revision);
        let changes = trace_span!("point_forest.capture_site_batches").in_scope(|| {
            stream.take_point_changes(previous.is_none_or(|old| !old.same_generation(revision)))
        });
        trace!(
            point_revision = revision.points.sites,
            construction_revision = revision.points.construction,
            batches = changes.patches.len(),
            full_capture = changes.reset,
            "Scheduling point forest hierarchy"
        );
        let portraits = portraits.clone();
        let prepared = self.prepared_geometry.clone();
        let active = self.active.clone();
        let placements = std::mem::take(&mut self.placements);
        self.last_build = Some(key);
        self.hierarchy_task = Some(AsyncComputeTaskPool::get().spawn(async move {
            build_hierarchy(
                revision, previous, changes, placements, &portraits, prepared, active,
            )
        }));
    }

    fn request_geometry(
        &mut self,
        revision: SourceRevision,
        portraits: &Portraits,
        now: f64,
        (storage, layers): (u64, u32),
    ) {
        if self.geometry_task.is_some() || now < self.next_geometry_build {
            return;
        }
        let Some(demand) = &self.demand else { return };
        if !demand.revision.same_generation(revision) || demand.templates.is_empty() {
            return;
        }
        let covered = self
            .prepared_geometry
            .iter()
            .chain(self.active.iter().map(|snapshot| &snapshot.geometry))
            .any(|geometry| {
                geometry.models == revision.models && geometry.covers(&demand.templates)
            });
        if covered {
            return;
        }
        let keys: Vec<_> = demand.templates.keys().copied().collect();
        let request = (revision.models, keys);
        if self.last_geometry_request.as_ref() == Some(&request) {
            return;
        }
        self.last_geometry_request = Some(request);
        self.next_geometry_build = now + 3.0;
        self.geometry_error = None;
        self.next_geometry_id += 1;
        let id = self.next_geometry_id;
        let models = revision.models;
        let templates = demand.templates.clone();
        let portraits = portraits.clone();
        self.geometry_task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let _span = trace_span!("point_forest.build_geometry_pack").entered();
            let inputs = templates
                .into_values()
                .map(|(species, variant)| portraits.planet_input(species, variant))
                .collect();
            let pack = Pack::build(inputs, storage, layers).map(|pack| {
                Arc::new(Geometry {
                    id,
                    models,
                    pack: Some(pack),
                })
            });
            GeometryResult { models, pack }
        }));
    }
}

fn build_hierarchy(
    revision: SourceRevision,
    previous: Option<SourceRevision>,
    changes: PointSiteChanges,
    mut placements: PlacementIndex,
    portraits: &Portraits,
    prepared: Option<Arc<Geometry>>,
    active: Option<Snapshot>,
) -> BuildResult {
    let _span = trace_span!("point_forest.build_hierarchy").entered();
    let reset = changes.reset;
    let dirty = placements.apply(changes, portraits);
    let templates = placements.required_templates();
    let snapshot =
        choose_geometry(revision, &templates, prepared, active.as_ref()).map(|geometry| {
            let baseline = active.filter(|old| {
                !reset && previous == Some(old.revision) && Arc::ptr_eq(&old.geometry, &geometry)
            });
            let hierarchy = if let Some(old) = baseline {
                let cells = trace_span!("point_forest.sort_placements").in_scope(|| {
                    dirty
                        .into_iter()
                        .map(|key| (key, placements.cell_placements(key)))
                        .collect::<Vec<_>>()
                });
                old.hierarchy
                    .update_cells(geometry.pack(), cells)
                    .map_or_else(|| old.hierarchy, Arc::new)
            } else {
                Arc::new(PlanetHierarchy::build_cells(
                    geometry.pack(),
                    placements.all_cells(),
                ))
            };
            Snapshot {
                revision,
                geometry,
                hierarchy,
                candidates: placements.candidates,
            }
        });
    BuildResult {
        demand: TemplateDemand {
            revision,
            templates,
        },
        snapshot,
        placements,
    }
}

fn choose_geometry(
    revision: SourceRevision,
    templates: &Templates,
    prepared: Option<Arc<Geometry>>,
    active: Option<&Snapshot>,
) -> Option<Arc<Geometry>> {
    // Keep using the active pack while new templates bake. A prepared pack
    // from an older set of sites must not displace a usable active pack.
    let prepared = prepared.filter(|pack| pack.models == revision.models);
    if prepared.as_ref().is_some_and(|pack| pack.covers(templates)) {
        return prepared;
    }
    match active {
        Some(old) if old.geometry.models == revision.models || templates.is_empty() => {
            Some(old.geometry.clone())
        }
        Some(_) => None,
        None => prepared,
    }
}

pub(super) fn take_ready<T>(slot: &mut Option<Task<T>>) -> Option<T> {
    let task = slot.as_mut()?;
    // The system polls again next frame, so no wakeup or blocking executor is needed.
    match Pin::new(task).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(result) => {
            *slot = None;
            Some(result)
        }
        Poll::Pending => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forest::{
        impostor,
        placement::{PatchKey, Site},
        streaming::Sites,
    };
    use bevy::{math::DVec3, tasks::TaskPool};
    use igor_renderer::{
        planetary::{AtlasData, Summary, Template, TemplateInfo, Transform, LEVELS},
        splat::Range,
    };

    fn geometry(id: u64, models: u64, species: Species) -> Arc<Geometry> {
        AsyncComputeTaskPool::get_or_init(TaskPool::new);
        let tile = impostor::tile(species, 0);
        let template = Template {
            transform: Transform {
                tile,
                root: [id as f32, 0.0, 0.0],
                scale: 1.0,
            },
            info: TemplateInfo {
                ranges: [Range {
                    offset: 0,
                    count: 1,
                }; LEVELS],
                radii: [1.0; LEVELS],
                center: [8.0, 0.0, 8.0],
                bound: 1.0,
                summary: Summary {
                    position: [8.0, 0.0, 8.0],
                    normal: [0.0, 1.0, 0.0],
                    color: [0.2, 0.4, 0.1],
                    radius: 1.0,
                    coverage: 1.0,
                },
            },
        };
        Arc::new(Geometry {
            id,
            models,
            pack: Some(Pack {
                raw: vec![],
                lod: vec![],
                patches: vec![],
                materials: vec![],
                atlas: AtlasData {
                    side: 1,
                    layers: 1,
                    tiles: vec![],
                },
                templates: [(tile, template)].into(),
                levels: (0..LEVELS).collect(),
            }),
        })
    }

    fn revision(sites: u64, models: u64) -> SourceRevision {
        SourceRevision {
            points: PointRevision {
                sites,
                construction: 0,
            },
            models,
        }
    }

    fn batch(x: f64) -> Sites {
        Arc::from([Site {
            position: DVec3::new(x, 10.0, 0.0),
            species: Species::Oak,
            variant: 0,
            scale: 1.0,
            yaw: 0.0,
        }])
    }

    #[test]
    fn duplicate_batch_edits_share_blocks_and_pack_changes_rebuild() {
        let portraits = Portraits::default();
        let old_pack = geometry(1, 0, Species::Oak);
        let first = revision(1, 0);
        let second = revision(2, 0);
        let a = PatchKey(0, 0);
        let b = PatchKey(0, 1);
        let old = build_hierarchy(
            first,
            None,
            PointSiteChanges {
                reset: true,
                patches: vec![(a, Some(batch(8.0))), (b, Some(batch(-8.0)))],
            },
            PlacementIndex::default(),
            &portraits,
            Some(old_pack.clone()),
            None,
        );
        let original = old.snapshot.unwrap();
        let repeated = build_hierarchy(
            second,
            Some(first),
            PointSiteChanges {
                reset: false,
                patches: vec![(b, Some(batch(-8.0))), (a, Some(batch(8.0)))],
            },
            old.placements,
            &portraits,
            Some(old_pack),
            Some(original.clone()),
        );
        let repeated_snapshot = repeated.snapshot.unwrap();
        assert!(Arc::ptr_eq(
            &original.hierarchy,
            &repeated_snapshot.hierarchy
        ));

        let new_pack = geometry(2, 0, Species::Oak);
        let next = build_hierarchy(
            second,
            Some(second),
            PointSiteChanges {
                reset: false,
                patches: vec![],
            },
            repeated.placements,
            &portraits,
            Some(new_pack.clone()),
            Some(repeated_snapshot),
        )
        .snapshot
        .unwrap();
        assert!(!Arc::ptr_eq(&original.hierarchy, &next.hierarchy));
        assert!(Arc::ptr_eq(&new_pack, &next.geometry));
        let full = build_hierarchy(
            second,
            None,
            PointSiteChanges {
                reset: true,
                patches: vec![(a, Some(batch(8.0))), (b, Some(batch(-8.0)))],
            },
            PlacementIndex::default(),
            &portraits,
            Some(new_pack),
            None,
        )
        .snapshot
        .unwrap();
        assert_eq!(
            next.hierarchy.shadow_placements([0.0; 3]),
            full.hierarchy.shadow_placements([0.0; 3])
        );
        let select = |snapshot: &Snapshot| {
            let mut scene = PlanetScene::default();
            scene.replace_hierarchy(snapshot.hierarchy.clone());
            let camera = Camera {
                position: [0.0; 3],
                forward: [0.0, 1.0, 0.0],
                right: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                focal: 800.0,
                width: 4096,
                height: 4096,
                reach: 2000.0,
                planet_radius: 0.0,
            };
            scene
                .select(snapshot.geometry.pack(), camera, 2.2, 1000, 1000)
                .instances
        };
        let updated = select(&next);
        assert!(!updated.is_empty());
        assert_eq!(updated, select(&full));
        assert_ne!(updated, select(&original));
    }

    #[test]
    fn patch_edits_rebuild_shared_cells_without_discarding_other_contributors() {
        let portraits = Portraits::default();
        let pack = geometry(1, 0, Species::Oak);
        let a = PatchKey(0, 0);
        let b = PatchKey(0, 1);
        let c = PatchKey(0, 2);
        let first = revision(1, 0);
        let second = revision(2, 0);
        let old = build_hierarchy(
            first,
            None,
            PointSiteChanges {
                reset: true,
                patches: vec![(a, Some(batch(6.0))), (b, Some(batch(8.0)))],
            },
            PlacementIndex::default(),
            &portraits,
            Some(pack.clone()),
            None,
        );
        let edited = build_hierarchy(
            second,
            Some(first),
            PointSiteChanges {
                reset: false,
                patches: vec![(a, None), (c, Some(batch(24.0)))],
            },
            old.placements,
            &portraits,
            Some(pack.clone()),
            old.snapshot,
        )
        .snapshot
        .unwrap();
        let positions: Vec<_> = edited
            .hierarchy
            .shadow_placements([0.0; 3])
            .into_iter()
            .map(|placement| placement.position[0])
            .collect();
        assert_eq!(positions, [8.0, 24.0]);
        assert_eq!(edited.candidates, 2);

        let full = build_hierarchy(
            second,
            None,
            PointSiteChanges {
                reset: true,
                patches: vec![(b, Some(batch(8.0))), (c, Some(batch(24.0)))],
            },
            PlacementIndex::default(),
            &portraits,
            Some(pack),
            None,
        )
        .snapshot
        .unwrap();
        assert_eq!(
            edited.hierarchy.shadow_placements([0.0; 3]),
            full.hierarchy.shadow_placements([0.0; 3])
        );
    }

    #[test]
    fn partial_or_obsolete_prepared_pack_does_not_displace_active_geometry() {
        let portraits = Portraits::default();
        let patch = PatchKey(0, 0);
        let first = revision(1, 0);
        let old = build_hierarchy(
            first,
            None,
            PointSiteChanges {
                reset: true,
                patches: vec![(patch, Some(batch(8.0)))],
            },
            PlacementIndex::default(),
            &portraits,
            Some(geometry(1, 0, Species::Oak)),
            None,
        )
        .snapshot
        .unwrap();
        let demand = BTreeMap::from([(impostor::tile(Species::Oak, 0), (Species::Oak, 0))]);
        let next = choose_geometry(
            first,
            &demand,
            Some(geometry(2, 0, Species::Palm)),
            Some(&old),
        )
        .unwrap();
        assert!(Arc::ptr_eq(&old.geometry, &next));
        assert!(choose_geometry(
            revision(2, 1),
            &demand,
            Some(geometry(3, 0, Species::Oak)),
            Some(&old),
        )
        .is_none());
    }
}
