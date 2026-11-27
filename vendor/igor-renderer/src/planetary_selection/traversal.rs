use std::ops::Range as WorkRange;

use super::{
    super::{choose_local, collapse_world, dense_policy, Point, Range, UNKNOWN},
    identity, Camera, Cell, History, Identity, Item, Members, Node, Pack, PlanetHierarchy,
    SelectionExecutor, LANES, RAW, WIDTHS,
};
use glam::{Quat, Vec3};
use tracing::{trace, trace_span};

// Descendant trees give a cheap upper bound on useful work. Only projected
// subtrees contribute, and a collapsed subtree counts as a single operation.
const MIN_TREES_PER_WORKER: usize = 512;

#[derive(Clone, Copy, Default)]
pub(super) struct Totals {
    pub(super) points: u64,
    pub(super) jobs: u64,
    pub(super) raw: u64,
    pub(super) covered: u32,
    pub(super) clusters: u32,
    pub(super) visited: u32,
}

impl Totals {
    fn add(&mut self, other: Self) {
        self.points += other.points;
        self.jobs += other.jobs;
        self.raw += other.raw;
        self.covered += other.covered;
        self.clusters += other.clusters;
        self.visited += other.visited;
    }

    pub(super) fn fits(self, budget: u64, max_jobs: u64) -> bool {
        self.points <= budget && self.jobs <= max_jobs
    }
}

#[derive(Default)]
struct Work {
    range: WorkRange<usize>,
    items: Vec<Item>,
    nodes: Vec<((usize, Cell), u8)>,
    trees: Vec<(Identity, u8)>,
    totals: Totals,
}

impl Work {
    fn reset(&mut self) {
        self.items.clear();
        self.nodes.clear();
        self.trees.clear();
        self.totals = Totals::default();
    }

    fn push(&mut self, item: Item, represented: u32, cluster: bool) {
        if item.count == 0 || item.fade == 0 {
            return;
        }
        self.totals.points += u64::from(item.count);
        self.totals.jobs += u64::from(item.count.div_ceil(LANES));
        if !cluster && item.range[0] & RAW != 0 {
            self.totals.raw += u64::from(item.count);
        }
        self.totals.covered += represented;
        self.totals.clusters += u32::from(cluster);
        self.items.push(item);
    }
}

#[derive(Default)]
pub(super) struct SelectionScratch {
    partition_nodes: Vec<((usize, Cell), u8)>,
    partition_visited: u32,
    work: Vec<Work>,
    active: usize,
}

impl SelectionScratch {
    pub(super) fn probe(
        &mut self,
        hierarchy: &PlanetHierarchy,
        history: &History,
        pack: &Pack,
        camera: Camera,
        threshold: f32,
        executor: &impl SelectionExecutor,
    ) -> Totals {
        let _span = trace_span!("PlanetScene::traverse_hierarchy").entered();
        let traversal = Traversal {
            history,
            pack,
            camera,
            threshold,
        };
        self.partition_nodes.clear();
        self.partition_visited = 0;
        let workers = executor.worker_count().max(1);
        let top = WIDTHS.len() - 1;
        let roots = &hierarchy.roots;
        if workers == 1
            || roots.iter().map(|root| root.count as usize).sum::<usize>()
                < 2 * MIN_TREES_PER_WORKER
        {
            let work = &mut self.prepare(1)[0];
            for root in roots {
                traversal.visit(top, root, work);
            }
            return work.totals;
        }

        let (subtrees, ranges) = trace_span!("PlanetScene::partition_workloads").in_scope(|| {
            let (subtrees, weight) = traversal.partition(
                roots,
                workers,
                &mut self.partition_nodes,
                &mut self.partition_visited,
            );
            let groups = (weight / MIN_TREES_PER_WORKER)
                .min(workers)
                .min(subtrees.len())
                .max(1);
            let ranges = workload_ranges(&subtrees, weight, groups);
            trace!(
                subtrees = subtrees.len(),
                workloads = groups,
                estimated_trees = weight,
                "Point forest selection workloads"
            );
            (subtrees, ranges)
        });
        let partition_visited = self.partition_visited;
        let work = self.prepare(ranges.len());
        for (slot, range) in work.iter_mut().zip(ranges) {
            slot.range = range;
        }
        let run = |slot: &mut Work| {
            for subtree in &subtrees[slot.range.clone()] {
                slot.totals.visited += 1;
                traversal.visit_visible(subtree.level, subtree.node, subtree.collapse, slot);
            }
        };
        if work.len() == 1 {
            run(&mut work[0]);
        } else {
            let _span = trace_span!("PlanetScene::parallel_traversal").entered();
            executor.for_each(work, |slot| {
                let _span = trace_span!("PlanetScene::worker_workload").entered();
                run(slot);
            });
        }
        let mut totals = Totals {
            visited: partition_visited,
            ..Totals::default()
        };
        for slot in work {
            totals.add(slot.totals);
        }
        totals
    }

    fn prepare(&mut self, active: usize) -> &mut [Work] {
        if self.work.len() < active {
            self.work.resize_with(active, Work::default);
        }
        self.active = active;
        let work = &mut self.work[..active];
        for slot in work.iter_mut() {
            slot.reset();
        }
        work
    }

    pub(super) fn commit(&mut self, history: &mut History) -> &mut Vec<Item> {
        let _span = trace_span!("PlanetScene::merge_workloads").entered();
        let work = &mut self.work[..self.active];
        history.nodes.clear();
        history.trees.clear();
        history.nodes.reserve(
            self.partition_nodes.len() + work.iter().map(|slot| slot.nodes.len()).sum::<usize>(),
        );
        history
            .trees
            .reserve(work.iter().map(|slot| slot.trees.len()).sum());
        history.nodes.extend(self.partition_nodes.drain(..));
        for slot in work.iter_mut() {
            history.nodes.extend(slot.nodes.drain(..));
            history.trees.extend(slot.trees.drain(..));
        }
        let (first, rest) = work.split_first_mut().unwrap();
        first
            .items
            .reserve(rest.iter().map(|slot| slot.items.len()).sum());
        for slot in rest {
            first.items.append(&mut slot.items);
        }
        &mut first.items
    }
}

#[derive(Clone, Copy)]
struct Subtree<'a> {
    level: usize,
    node: &'a Node,
    collapse: bool,
    weight: usize,
}

struct Traversal<'a> {
    history: &'a History,
    pack: &'a Pack,
    camera: Camera,
    threshold: f32,
}

impl Traversal<'_> {
    fn collapse(&self, level: usize, node: &Node) -> Option<bool> {
        let (center, radius) = node.sphere();
        let px = self
            .camera
            .projected(center, radius, WIDTHS[level] as f32)?;
        let old = *self
            .history
            .nodes
            .get(&(level, node.key))
            .unwrap_or(&UNKNOWN);
        Some(collapse_world(px, self.threshold, old))
    }

    fn partition<'a>(
        &self,
        roots: &'a [std::sync::Arc<Node>],
        workers: usize,
        nodes: &mut Vec<((usize, Cell), u8)>,
        visited: &mut u32,
    ) -> (Vec<Subtree<'a>>, usize) {
        let mut prepare = |level, node: &'a Node| {
            let Some(collapse) = self.collapse(level, node) else {
                *visited += 1;
                return None;
            };
            let weight = if collapse { 1 } else { node.count as usize };
            Some(Subtree {
                level,
                node,
                collapse,
                weight,
            })
        };
        let mut frontier: Vec<_> = roots
            .iter()
            .filter_map(|root| prepare(WIDTHS.len() - 1, root))
            .collect();
        let mut weight: usize = frontier.iter().map(|subtree| subtree.weight).sum();
        let mut expanded = 0;
        // Limit the planning cost even if the camera sees a very irregular distribution.
        for _ in 0..workers.saturating_mul(4).saturating_add(WIDTHS.len()) {
            let groups = (weight / MIN_TREES_PER_WORKER).min(workers);
            if groups < 2 {
                break;
            }
            let target = weight.div_ceil(groups.saturating_mul(2));
            let Some((index, &subtree)) = frontier
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.collapse && s.level > 0 && s.weight > target)
                .max_by_key(|(_, s)| s.weight)
            else {
                break;
            };
            expanded += 1;
            nodes.push(((subtree.level, subtree.node.key), 0));
            let old_len = frontier.len();
            frontier.extend(
                subtree
                    .node
                    .children()
                    .iter()
                    .filter_map(|child| prepare(subtree.level - 1, child)),
            );
            weight = weight - subtree.weight
                + frontier[old_len..].iter().map(|s| s.weight).sum::<usize>();
            // Keep the frontier in depth-first order after replacing a branch with its children.
            let children = frontier.len() - old_len;
            frontier[index..].rotate_right(children);
            frontier.remove(index + children);
        }
        *visited += expanded;
        (frontier, weight)
    }

    fn visit(&self, level: usize, node: &Node, work: &mut Work) {
        work.totals.visited += 1;
        if let Some(collapse) = self.collapse(level, node) {
            self.visit_visible(level, node, collapse, work);
        }
    }

    fn visit_visible(&self, level: usize, node: &Node, collapse: bool, work: &mut Work) {
        let cam = self.camera;
        let pack = self.pack;
        work.nodes.push(((level, node.key), u8::from(collapse)));
        if collapse {
            if let Some(sample) = node.aggregate.sample() {
                let position = node.anchor + sample.pos.as_dvec3();
                let point = Point::encode(
                    Vec3::ZERO,
                    1.0,
                    sample.color,
                    sample.normal,
                    sample.coverage,
                );
                work.push(
                    Item {
                        origin: (position - cam.eye())
                            .as_vec3()
                            .extend(sample.radius)
                            .to_array(),
                        rotation: Quat::IDENTITY.to_array(),
                        range: [point.rgba, point.normal, 1, 0],
                        count: 1,
                        fade: cam.fade(position),
                        level: pack.widths.len(),
                        model: 0,
                    },
                    node.count,
                    true,
                );
            }
            return;
        }
        let trees = match &node.members {
            Members::Children(children) => {
                for child in children.iter() {
                    self.visit(level - 1, child, work);
                }
                return;
            }
            Members::Trees(trees) => trees,
        };
        for tree in trees.iter() {
            let Some(ppm) = cam.projected(tree.center, tree.radius, tree.scale) else {
                continue;
            };
            let model = &pack.templates[&tree.input.model];
            let distance = ((tree.center - cam.eye()).length() as f32 - tree.radius).max(0.0);
            let weight = 0.18 + 0.82 * (distance / 160.0).clamp(0.0, 1.0);
            let id = identity(tree.input);
            let hash = (id[0] ^ id[1].rotate_left(21) ^ id[2].rotate_left(42)) as u32;
            let stagger = if dense_policy(&pack.levels) {
                0.94 + 0.12 * crate::random(hash ^ 0x3f912ac7)
            } else {
                1.0
            };
            let local = (self.threshold * weight * stagger).max(0.25);
            let old = *self.history.trees.get(&id).unwrap_or(&UNKNOWN);
            let chosen = choose_local(ppm, local, old, &pack.levels, &pack.widths);
            work.trees.push((id, chosen as u8));
            let Range { offset, count } = model.info.ranges[chosen];
            work.push(
                Item {
                    origin: (tree.origin - cam.eye())
                        .as_vec3()
                        .extend(tree.scale)
                        .to_array(),
                    rotation: tree.rotation.to_array(),
                    range: [offset, count, 0, tree.input.phase.to_bits()],
                    count,
                    fade: cam.fade(tree.center),
                    level: chosen,
                    model: tree.input.model,
                },
                1,
                false,
            );
        }
    }
}

fn workload_ranges(
    subtrees: &[Subtree<'_>],
    mut weight: usize,
    groups: usize,
) -> Vec<WorkRange<usize>> {
    let mut ranges = Vec::with_capacity(groups);
    let mut start = 0;
    for remaining in (2..=groups).rev() {
        let target = weight.div_ceil(remaining);
        let limit = subtrees.len() - (remaining - 1);
        let mut end = start + 1;
        let mut current = subtrees[start].weight;
        while end < limit && current < target {
            let next = current + subtrees[end].weight;
            if current.abs_diff(target) <= next.abs_diff(target) {
                break;
            }
            current = next;
            end += 1;
        }
        ranges.push(start..end);
        start = end;
        weight -= current;
    }
    ranges.push(start..subtrees.len());
    ranges
}
