//! Global priority-flood drainage on the planet's cube-sphere height grid.
//!
//! All land cells receive a downstream route to the connected ocean, even
//! when the raw procedural height field contains closed basins. The minimum
//! spill elevation through a basin is the surface elevation of its lake.
//! This is an offline bake; no new per-frame work is required by the client.

use std::{cmp::Ordering, collections::BinaryHeap};

use super::{decode_index, index, neighbour, HydrologySettings, SIZE};
use world_core::planet::{RADIUS, WATER_SURFACE_ELEVATION};

/// No downstream neighbor (connected ocean).
pub(super) const NO_OUTLET: u32 = u32::MAX;
pub(super) const NO_LAKE: u32 = u32::MAX;
const MIN_SPILL: f64 = WATER_SURFACE_ELEVATION;
const MIN_SPILLWAY_LEVEL: f64 = WATER_SURFACE_ELEVATION + 0.35;
const MAX_DRY_CHANNEL_BACKWATER: f64 = 5.0;
const MIN_WET_DEPTH: f64 = 0.24;
pub(super) const AREA_UNIT_KM2: f64 =
    (2.0 * RADIUS / (SIZE - 1) as f64) * (2.0 * RADIUS / (SIZE - 1) as f64) / 1_000_000.0;
const NEIGHBORS: [(i32, i32); 8] = [
    (-1, 0),
    (1, 0),
    (0, -1),
    (0, 1),
    (-1, -1),
    (-1, 1),
    (1, -1),
    (1, 1),
];

/// Downstream graph ordered from ocean towards the watershed divides.
pub(super) struct Drainage {
    pub outlets: Vec<u32>,
    pub accumulation: Vec<f32>,
    /// Minimum elevation at which a cell can drain to the connected ocean.
    pub spill: Vec<f64>,
    /// Land cells in downstream-first order. Reverse iteration is upstream-first.
    pub order: Vec<u32>,
}

#[derive(Clone, Copy)]
struct Front {
    elevation: f64,
    cell: u32,
    queued: usize,
}
impl PartialEq for Front {
    fn eq(&self, other: &Self) -> bool {
        self.queued == other.queued && self.elevation.total_cmp(&other.elevation) == Ordering::Equal
    }
}
impl Eq for Front {}
impl PartialOrd for Front {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Front {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: pop the lowest spill elevation first.
        other
            .elevation
            .total_cmp(&self.elevation)
            .then_with(|| other.queued.cmp(&self.queued))
    }
}

fn adjacent(cell: usize, result: &mut [usize; 8]) -> usize {
    let (face, x, y) = decode_index(cell);
    let mut count = 0;
    for (dx, dy) in NEIGHBORS {
        // The face transform supports cardinal seam edges. Diagonals at cube
        // seams are ambiguous, so route across the seam using cardinals.
        if dx != 0 && dy != 0 && (x == 0 || y == 0 || x + 1 == SIZE || y + 1 == SIZE) {
            continue;
        }
        let (f, nx, ny) = neighbour(face, x, y, dx, dy);
        let next = index(f, nx, ny);
        if next != cell && !result[..count].contains(&next) {
            result[count] = next;
            count += 1;
        }
    }
    count
}

/// Route runoff to the connected ocean without modifying any terrain samples.
pub(super) fn route(heights: &[f64], ocean: &[bool]) -> Drainage {
    assert_eq!(heights.len(), ocean.len());
    assert!(heights.len() <= u32::MAX as usize);
    assert!(heights.iter().all(|h| h.is_finite()));

    let mut outlets = vec![NO_OUTLET; heights.len()];
    let mut spill = vec![f64::INFINITY; heights.len()];
    let mut visited = vec![false; heights.len()];
    let mut queue = BinaryHeap::new();
    let mut order = Vec::with_capacity(heights.len());
    let mut neighbors = [0usize; 8];
    let mut queued = 0;

    // Seed the priority queue with the *shoreline*, not millions of sea
    // cells. Inland sub-sea-level basins are not outlets: they must spill.
    let mut sea_count = 0usize;
    for (cell, &sea) in ocean.iter().enumerate() {
        if sea {
            sea_count += 1;
            visited[cell] = true;
            spill[cell] = WATER_SURFACE_ELEVATION;
        }
    }
    if sea_count == 0 {
        // A completely land-covered planet still needs one global outlet.
        let lowest = heights
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .unwrap();
        visited[lowest] = true;
        spill[lowest] = heights[lowest];
        queue.push(Front {
            elevation: spill[lowest],
            cell: lowest as u32,
            queued,
        });
    } else {
        for (cell, &sea) in ocean.iter().enumerate() {
            if !sea {
                continue;
            }
            let count = adjacent(cell, &mut neighbors);
            for &next in &neighbors[..count] {
                if visited[next] {
                    continue;
                }
                visited[next] = true;
                outlets[next] = cell as u32;
                spill[next] = heights[next].max(WATER_SURFACE_ELEVATION);
                queued += 1;
                queue.push(Front {
                    elevation: spill[next],
                    cell: next as u32,
                    queued,
                });
            }
        }
    }

    while let Some(Front {
        elevation, cell, ..
    }) = queue.pop()
    {
        let cell = cell as usize;
        order.push(cell as u32);
        let count = adjacent(cell, &mut neighbors);
        // The flood establishes the minimum spill and a flat route. On a
        // real downhill slope, select the steepest outlet instead of the
        // lowest absolute neighbor (which favors diagonals and sea seeds).
        if outlets[cell] != NO_OUTLET {
            outlets[cell] = downhill_outlet(cell, &neighbors[..count], &spill, outlets[cell]);
        }
        for &next in &neighbors[..count] {
            if visited[next] {
                continue;
            }
            visited[next] = true;
            outlets[next] = cell as u32;
            spill[next] = elevation.max(heights[next]);
            queued += 1;
            queue.push(Front {
                elevation: spill[next],
                cell: next as u32,
                queued,
            });
        }
    }
    assert!(
        visited.iter().all(|&v| v),
        "cube-sphere drainage disconnected"
    );

    let accumulation = accumulate(heights.len(), &outlets, &order);
    Drainage {
        outlets,
        accumulation,
        spill,
        order,
    }
}

fn downhill_outlet(cell: usize, neighbors: &[usize], spill: &[f64], flat_outlet: u32) -> u32 {
    // Any strictly lower spill must already have left the priority queue.
    // Preserve the FIFO flood parent on flats to keep the graph acyclic.
    let mut candidates = neighbors
        .iter()
        .copied()
        .filter(|&n| spill[n] < spill[cell]);
    let Some(first) = candidates.next() else {
        return flat_outlet;
    };
    let (face, x, y) = decode_index(cell);
    let direction = grid_direction(face, x, y);
    let grade = |next| {
        let (face, x, y) = decode_index(next);
        let distance = direction.distance(grid_direction(face, x, y));
        // Duplicate boundary texels occupy one geographic position.
        (spill[cell] - spill[next]) / distance.max(1.0 / (SIZE - 1) as f64)
    };
    let mut best = (first, grade(first));
    for next in candidates {
        let slope = grade(next);
        if slope > best.1 {
            best = (next, slope);
        }
    }
    best.0 as u32
}

fn grid_direction(face: usize, x: usize, y: usize) -> bevy_math::DVec3 {
    let last = (SIZE - 1) as f64;
    bevy_math::DVec3::from_array(super::sphere_direction(
        face,
        x as f64 / last,
        y as f64 / last,
    ))
}

fn accumulate(cells: usize, outlets: &[u32], order: &[u32]) -> Vec<f32> {
    let mut accumulation = vec![0.0; cells];
    for &node in order.iter().rev() {
        let cell = node as usize;
        let (_, x, y) = decode_index(cell);
        accumulation[cell] += cell_area(x, y);
        if outlets[cell] != NO_OUTLET {
            accumulation[outlets[cell] as usize] += accumulation[cell];
        }
    }
    accumulation
}

pub(super) struct Basin {
    pub cells: Vec<u32>,
    pub outlet: u32,
    pub level: f64,
    pub area_km2: f64,
    pub max_depth: f64,
}

pub(super) struct Lakes {
    pub ids: Vec<u32>,
    pub basins: Vec<Basin>,
}

impl Drainage {
    pub fn depressions(&self, heights: &[f64]) -> Lakes {
        let mut ids = vec![NO_LAKE; heights.len()];
        let mut basins = Vec::new();
        let mut neighbors = [0usize; 8];
        // The first flooded cell visited is the downstream-most one. Its outlet
        // is on the sill; routing the interior through it cannot create a cycle.
        for &start in &self.order {
            let i = start as usize;
            let level = self.spill[i];
            if ids[i] != NO_LAKE || level < MIN_SPILL || level - heights[i] < MIN_WET_DEPTH {
                continue;
            }
            let id = basins.len() as u32;
            ids[i] = id;
            let mut cells = vec![start];
            let mut cursor = 0;
            let mut area_km2 = 0.0;
            let mut max_depth = 0.0_f64;
            while let Some(&node) = cells.get(cursor) {
                let cell = node as usize;
                cursor += 1;
                let (_, x, y) = decode_index(cell);
                area_km2 += f64::from(cell_area(x, y)) * AREA_UNIT_KM2;
                max_depth = max_depth.max(level - heights[cell]);
                let count = adjacent(cell, &mut neighbors);
                for &next in &neighbors[..count] {
                    if ids[next] == NO_LAKE
                        && self.outlets[next] != NO_OUTLET
                        && self.spill[next] == level
                        && level - heights[next] >= MIN_WET_DEPTH
                    {
                        ids[next] = id;
                        cells.push(next as u32);
                    }
                }
            }
            if self.outlets[i] == NO_OUTLET {
                for cell in cells {
                    ids[cell as usize] = NO_LAKE;
                }
                continue;
            }
            basins.push(Basin {
                cells,
                outlet: start,
                level,
                area_km2,
                max_depth,
            });
        }
        Lakes { ids, basins }
    }

    pub fn retain_lakes(
        &mut self,
        mut lakes: Lakes,
        heights: &[f64],
        settings: &HydrologySettings,
    ) -> Lakes {
        let threshold = (settings.river_catchment_km2 / AREA_UNIT_KM2) as f32;
        let mut keep: Vec<_> = lakes
            .basins
            .iter()
            .map(|lake| {
                lake.max_depth >= settings.lake_min_depth_m
                    && lake.area_km2 >= settings.lake_min_area_km2
            })
            .collect();
        let mut heads: Vec<_> = self
            .order
            .iter()
            .copied()
            .filter(|&node| self.accumulation[node as usize] >= threshold)
            .collect();
        heads.extend(
            lakes
                .basins
                .iter()
                .zip(&keep)
                .filter_map(|(lake, &keep)| keep.then_some(lake.outlet)),
        );
        let mut traced = vec![false; heights.len()];
        while let Some(mut node) = heads.pop() {
            while node != NO_OUTLET {
                let i = node as usize;
                if traced[i] {
                    break;
                }
                traced[i] = true;
                let id = lakes.ids[i];
                if id != NO_LAKE && self.spill[i] - heights[i] > MAX_DRY_CHANNEL_BACKWATER {
                    let needs_connector = &mut keep[id as usize];
                    if !*needs_connector {
                        *needs_connector = true;
                        heads.push(lakes.basins[id as usize].outlet);
                    }
                }
                node = self.outlets[i];
            }
        }
        let mut retained = Vec::new();
        for (lake, retain) in lakes.basins.into_iter().zip(keep) {
            let id = if retain {
                retained.len() as u32
            } else {
                NO_LAKE
            };
            for &node in &lake.cells {
                lakes.ids[node as usize] = id;
                if retain && node != lake.outlet {
                    self.outlets[node as usize] = lake.outlet;
                }
            }
            if retain {
                retained.push(lake);
            }
        }
        self.accumulation = accumulate(self.spill.len(), &self.outlets, &self.order);
        Lakes {
            ids: lakes.ids,
            basins: retained,
        }
    }
}

/// Incise bounded outlets on large depressions and return the number of edited samples.
pub(super) fn lower_spillways(
    heights: &mut [f64],
    ocean: &[bool],
    drainage: &Drainage,
    lakes: Lakes,
    settings: &HydrologySettings,
) -> (usize, usize) {
    let mut cuts = 0;
    let mut breached = 0;
    let mut spillway = Vec::new();
    for lake in lakes.basins {
        if lake.area_km2 < settings.spillway_min_area_km2 || settings.spillway_drawdown_m == 0.0 {
            continue;
        }
        let target = (lake.level - settings.spillway_drawdown_m).max(MIN_SPILLWAY_LEVEL);
        if target >= lake.level {
            continue;
        }
        spillway.clear();
        let mut next = drainage.outlets[lake.outlet as usize];
        while next != NO_OUTLET && drainage.spill[next as usize] > target {
            let cell = next as usize;
            debug_assert!(!ocean[cell]);
            if heights[cell] > target {
                spillway.push(cell);
                if spillway.len() > settings.spillway_max_cells {
                    break;
                }
            }
            next = drainage.outlets[cell];
        }
        if !spillway.is_empty() && spillway.len() <= settings.spillway_max_cells {
            for &cell in &spillway {
                heights[cell] = target;
            }
            cuts += spillway.len();
            breached += 1;
        }
    }
    (breached, cuts)
}

pub(super) fn cell_area(x: usize, y: usize) -> f32 {
    // Preserve the legacy cube-sphere area weighting for catchment thresholds.
    const CORRECTION: f32 = (0.87 * 0.87) as f32;
    let [u, v] = [x, y].map(|n| n as f32 / (SIZE - 1) as f32 * 2.0 - 1.0);
    let sx = 1.0 + CORRECTION - CORRECTION * u * u;
    let sy = 1.0 + CORRECTION - CORRECTION * v * v;
    let jacobian = (1.0 + CORRECTION).powi(2) / (sx * sy).powf(1.5);
    let area = jacobian / (1.0 + u * u / sx + v * v / sy).powf(1.5);
    let overlap_x = if x == 0 || x + 1 == SIZE { 0.5 } else { 1.0 };
    let overlap_y = if y == 0 || y + 1 == SIZE { 0.5 } else { 1.0 };
    area * overlap_x * overlap_y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spillway_lowering_edits_the_sill_and_preserves_the_ocean() {
        let heights = vec![-40.0, 8.0, 3.0, 2.0, 9.0, 15.0];
        let ocean = [true, false, false, false, false, false];
        let drainage = Drainage {
            outlets: vec![NO_OUTLET, 0, 1, 2, 3, 4],
            spill: vec![WATER_SURFACE_ELEVATION, 8.0, 8.0, 8.0, 9.0, 15.0],
            order: vec![1, 2, 3, 4, 5],
            accumulation: vec![0.0; heights.len()],
        };
        let lakes = || Lakes {
            ids: vec![NO_LAKE, NO_LAKE, 0, 0, NO_LAKE, NO_LAKE],
            basins: vec![Basin {
                cells: vec![2, 3],
                outlet: 2,
                level: 8.0,
                area_km2: 2.0,
                max_depth: 6.0,
            }],
        };
        let mut settings = HydrologySettings {
            spillway_min_area_km2: 1.0,
            spillway_drawdown_m: 3.0,
            spillway_max_cells: 0,
            ..Default::default()
        };
        let mut terrain = heights.clone();
        assert_eq!(
            lower_spillways(&mut terrain, &ocean, &drainage, lakes(), &settings),
            (0, 0)
        );
        assert_eq!(terrain, heights);
        settings.spillway_max_cells = 1;
        assert_eq!(
            lower_spillways(&mut terrain, &ocean, &drainage, lakes(), &settings),
            (1, 1)
        );
        assert_eq!(terrain, [-40.0, 5.0, 3.0, 2.0, 9.0, 15.0]);
    }

    #[test]
    fn highest_cells_route_to_ocean_through_closed_depression() {
        // Build a valley across one face; the uphill sill is the only exit.
        // Original D8 routing would stop at the 2 m local minimum.
        let heights = vec![0.0, 2.0, 8.0, 3.0, 2.0, 9.0, 15.0];
        let (outlets, spill) = route_line(&heights);
        assert_eq!(outlets, [NO_OUTLET, 0, 1, 2, 3, 4, 5]);
        assert_eq!(spill, [0.0, 2.0, 8.0, 8.0, 8.0, 9.0, 15.0]);
        for i in 1..heights.len() {
            assert!(spill[i] >= spill[outlets[i] as usize]);
        }
    }

    // A synthetic one-dimensional priority flood uses the same heap discipline,
    // without allocating the six-face production grid in a unit test.
    fn route_line(heights: &[f64]) -> (Vec<u32>, Vec<f64>) {
        let mut outlets = vec![NO_OUTLET; heights.len()];
        let mut spill = vec![f64::INFINITY; heights.len()];
        let mut done = vec![false; heights.len()];
        let mut heap = BinaryHeap::new();
        spill[0] = 0.0;
        done[0] = true;
        heap.push(Front {
            elevation: 0.0,
            cell: 0,
            queued: 0,
        });
        while let Some(Front {
            elevation, cell, ..
        }) = heap.pop()
        {
            let c = cell as usize;
            for n in [c.checked_sub(1), (c + 1 < heights.len()).then_some(c + 1)]
                .into_iter()
                .flatten()
            {
                if done[n] {
                    continue;
                }
                done[n] = true;
                outlets[n] = cell;
                spill[n] = heights[n].max(elevation);
                heap.push(Front {
                    cell: n as u32,
                    elevation: spill[n],
                    queued: n,
                });
            }
        }
        (outlets, spill)
    }

    #[test]
    fn downhill_prefers_grade_and_keeps_flat_flood_routes() {
        let middle = SIZE / 2;
        let source = index(0, middle, middle);
        let east = index(0, middle + 1, middle);
        let diagonal = index(0, middle + 1, middle + 1);
        let mut spill = vec![10.0; diagonal + 1];
        spill[east] = 4.0;
        spill[diagonal] = 3.0;
        assert_eq!(
            downhill_outlet(source, &[diagonal, east], &spill, diagonal as u32),
            east as u32
        );
        spill[east] = 10.0;
        spill[diagonal] = 10.0;
        assert_eq!(
            downhill_outlet(source, &[east, diagonal], &spill, diagonal as u32),
            diagonal as u32
        );
    }

    #[test]
    fn cube_sphere_neighbors_connect_all_six_faces() {
        let mut neighboring = [0usize; 8];
        for face in 0..6 {
            for &(x, y) in &[
                (0, SIZE / 2),
                (SIZE - 1, SIZE / 2),
                (SIZE / 2, 0),
                (SIZE / 2, SIZE - 1),
            ] {
                let count = adjacent(index(face, x, y), &mut neighboring);
                assert!((3..=8).contains(&count));
                assert!(neighboring[..count]
                    .iter()
                    .any(|&n| decode_index(n).0 != face));
            }
        }
    }
}
