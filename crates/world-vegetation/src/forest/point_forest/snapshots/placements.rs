use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
};

use bevy::log::trace_span;
use igor_renderer::planetary::{LeafKey, Placement};

use super::{
    super::super::{impostor, placement::PatchKey, streaming::PointSiteChanges, Species},
    Portraits, Templates,
};

type TemplateCounts = BTreeMap<usize, (Species, usize, usize)>;

struct PatchPlacements {
    placements: Box<[Placement]>,
    cells: Box<[LeafKey]>,
    templates: Box<[(usize, (Species, usize, usize))]>,
}

/// Worker-owned placement inputs. Only this index consults patch keys and cell ranges.
#[derive(Default)]
pub(super) struct PlacementIndex {
    patches: HashMap<PatchKey, PatchPlacements>,
    cells: BTreeMap<(LeafKey, PatchKey), Range<usize>>,
    templates: TemplateCounts,
    pub(super) candidates: usize,
}

impl PlacementIndex {
    pub(super) fn apply(
        &mut self,
        changes: PointSiteChanges,
        portraits: &Portraits,
    ) -> BTreeSet<LeafKey> {
        let _span = trace_span!("point_forest.collect_placements").entered();
        if changes.reset {
            // Drop the obsolete index on the worker.
            *self = Self::default();
        }
        let mut dirty = BTreeSet::new();
        for (patch, sites) in changes.patches {
            if let Some(old) = self.patches.remove(&patch) {
                self.candidates -= old.placements.len();
                for &(tile, (_, _, count)) in &old.templates {
                    let total = &mut self.templates.get_mut(&tile).unwrap().2;
                    *total -= count;
                    if *total == 0 {
                        self.templates.remove(&tile);
                    }
                }
                for &cell in &old.cells {
                    self.cells.remove(&(cell, patch));
                    dirty.insert(cell);
                }
            }
            let Some(sites) = sites else { continue };
            let mut templates = TemplateCounts::new();
            let mut placed: Vec<_> = sites
                .iter()
                .map(|site| {
                    let variant = portraits.point_variant(site.species, site.variant);
                    let model = impostor::tile(site.species, variant);
                    templates
                        .entry(model)
                        .or_insert((site.species, variant, 0))
                        .2 += 1;
                    let placement = Placement {
                        position: site.position.to_array(),
                        rotation: site.rotation().to_array(),
                        scale: site.scale,
                        model,
                        phase: site.yaw,
                    };
                    (placement.leaf_key(), placement)
                })
                .collect();
            // Preserve the sampling order for equal placements within one batch.
            placed.sort_by_key(|&(cell, _)| cell);
            let mut placements = Vec::with_capacity(placed.len());
            let mut cells = Vec::new();
            for run in placed.chunk_by(|a, b| a.0 == b.0) {
                let cell = run[0].0;
                let start = placements.len();
                placements.extend(run.iter().map(|&(_, placement)| placement));
                self.cells.insert((cell, patch), start..placements.len());
                cells.push(cell);
                if !changes.reset {
                    dirty.insert(cell);
                }
            }
            for (&tile, &(species, variant, count)) in &templates {
                self.templates
                    .entry(tile)
                    .or_insert((species, variant, 0))
                    .2 += count;
            }
            self.candidates += placements.len();
            self.patches.insert(
                patch,
                PatchPlacements {
                    placements: placements.into_boxed_slice(),
                    cells: cells.into_boxed_slice(),
                    templates: templates.into_iter().collect(),
                },
            );
        }
        dirty
    }

    pub(super) fn required_templates(&self) -> Templates {
        self.templates
            .iter()
            .map(|(&tile, &(species, variant, _))| (tile, (species, variant)))
            .collect()
    }

    pub(super) fn all_cells(&self) -> impl Iterator<Item = (LeafKey, Vec<Placement>)> + '_ {
        let mut last = None;
        self.cells.keys().filter_map(move |&(cell, _)| {
            if last == Some(cell) {
                return None;
            }
            last = Some(cell);
            Some((cell, self.cell_placements(cell)))
        })
    }

    pub(super) fn cell_placements(&self, cell: LeafKey) -> Vec<Placement> {
        let first = (cell, PatchKey(i32::MIN, i32::MIN));
        let last = (cell, PatchKey(i32::MAX, i32::MAX));
        let mut sources = self
            .cells
            .range(first..=last)
            .map(|(&(_, patch), range)| &self.patches[&patch].placements[range.clone()]);
        let Some(first) = sources.next() else {
            return Vec::new();
        };
        let mut placements = first.to_vec();
        for source in sources {
            placements.extend_from_slice(source);
        }
        placements.sort_by(Placement::spatial_cmp);
        placements
    }
}
