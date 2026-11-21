//! Global priority-flood drainage on the planet's cube-sphere height grid.
//!
//! All land cells receive a downstream route to the connected ocean, even
//! when the raw procedural height field contains closed basins. The minimum
//! spill elevation through a basin is the surface elevation of its lake.
//! This is an offline bake; no new per-frame work is required by the client.

use std::{cmp::Ordering, collections::{BinaryHeap, VecDeque}};

use super::{decode_index, index, neighbour, SIZE, SEA_LEVEL};

/// No downstream neighbor (connected ocean).
pub(super) const NO_OUTLET: u32 = u32::MAX;
const MIN_LAKE_DEPTH: f64 = 2.0;
const MIN_LAKE_CELLS: usize = 12;
const NEIGHBORS: [(i32, i32); 8] = [
    (-1, 0), (1, 0), (0, -1), (0, 1),
    (-1, -1), (-1, 1), (1, -1), (1, 1),
];

/// Downstream graph ordered from ocean towards the watershed divides.
pub(super) struct Drainage {
    pub outlets: Vec<u32>,
    pub accumulation: Vec<f32>,
    /// Minimum elevation at which a cell can drain to the connected ocean.
    pub spill: Vec<f64>,
    /// Land cells in downstream-first order. Reverse iteration is upstream-first.
    pub order: Vec<u32>,
    /// Lakes with a minimum area, excluding temporary single-cell depressions.
    pub lakes: Vec<bool>,
}

#[derive(Clone, Copy)]
struct Front {
    elevation: f64,
    cell: u32,
}
impl PartialEq for Front {
    fn eq(&self, other: &Self) -> bool {
        self.cell == other.cell && self.elevation.total_cmp(&other.elevation) == Ordering::Equal
    }
}
impl Eq for Front {}
impl PartialOrd for Front {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> { Some(self.cmp(other)) }
}
impl Ord for Front {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: pop the lowest spill elevation first.
        other.elevation.total_cmp(&self.elevation).then_with(|| other.cell.cmp(&self.cell))
    }
}

fn adjacent(cell: usize, result: &mut [usize; 8]) -> usize {
    let (face, x, y) = decode_index(cell);
    let mut count = 0;
    for (dx, dy) in NEIGHBORS {
        // The face transform supports cardinal seam edges. Diagonals at cube
        // seams are ambiguous, so route across the seam using cardinals.
        if dx != 0 && dy != 0 &&
            (x == 0 || y == 0 || x + 1 == SIZE || y + 1 == SIZE) {
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

/// Route runoff through every basin to the largest connected sea/ocean.
///
/// The returned lake mask distinguishes natural filled depressions from
/// disconnected seawater and small terrain raster artifacts.
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

    // Seed the priority queue with the *shoreline*, not millions of sea
    // cells. Inland sub-sea-level basins are not outlets: they must spill.
    let mut sea_count = 0usize;
    for (cell, &sea) in ocean.iter().enumerate() {
        if sea {
            sea_count += 1;
            visited[cell] = true;
            spill[cell] = SEA_LEVEL;
        }
    }
    if sea_count == 0 {
        // A completely land-covered planet still needs one global outlet.
        let lowest = heights.iter().enumerate()
            .min_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap();
        visited[lowest] = true;
        spill[lowest] = heights[lowest];
        queue.push(Front { elevation: spill[lowest], cell: lowest as u32 });
    } else {
        for (cell, &sea) in ocean.iter().enumerate() {
            if !sea { continue; }
            let count = adjacent(cell, &mut neighbors);
            for &next in &neighbors[..count] {
                if visited[next] { continue; }
                visited[next] = true;
                outlets[next] = cell as u32;
                spill[next] = heights[next].max(SEA_LEVEL);
                queue.push(Front { elevation: spill[next], cell: next as u32 });
            }
        }
    }

    while let Some(Front { elevation, cell }) = queue.pop() {
        let cell = cell as usize;
        order.push(cell as u32);
        let count = adjacent(cell, &mut neighbors);
        for &next in &neighbors[..count] {
            if visited[next] { continue; }
            visited[next] = true;
            outlets[next] = cell as u32;
            spill[next] = elevation.max(heights[next]);
            queue.push(Front { elevation: spill[next], cell: next as u32 });
        }
    }
    assert!(visited.iter().all(|&v| v), "cube-sphere drainage disconnected");

    // Accumulate precipitation/catchment in true topological order.
    // Sorting by raw elevation was incorrect after carving and in basins.
    let mut accumulation = vec![0.0_f32; heights.len()];
    for &node in order.iter().rev() {
        let cell = node as usize;
        let (_, x, y) = decode_index(cell);
        accumulation[cell] += cell_area(x, y);
        let next = outlets[cell];
        if next != NO_OUTLET {
            accumulation[next as usize] += accumulation[cell];
        }
    }

    let mut lakes: Vec<bool> = spill.iter().enumerate()
        .map(|(i, &level)| !ocean[i] && level > SEA_LEVEL + 0.5
            && level - heights[i] >= MIN_LAKE_DEPTH)
        .collect();
    // Reject puddles and one-pixel pits: river water can still traverse them.
    // Connected pixels of the same spill level constitute one lake surface.
    let mut inspected = vec![false; lakes.len()];
    let mut component = Vec::new();
    let mut todo = VecDeque::new();
    for start in 0..lakes.len() {
        if !lakes[start] || inspected[start] { continue; }
        inspected[start] = true;
        todo.push_back(start);
        component.clear();
        let level = spill[start];
        while let Some(cell) = todo.pop_front() {
            component.push(cell);
            let count = adjacent(cell, &mut neighbors);
            for &next in &neighbors[..count] {
                if lakes[next] && !inspected[next] && (spill[next] - level).abs() < 0.2 {
                    inspected[next] = true;
                    todo.push_back(next);
                }
            }
        }
        if component.len() < MIN_LAKE_CELLS {
            for &cell in &component { lakes[cell] = false; }
        }
    }

    Drainage { outlets, accumulation, spill, order, lakes }
}

fn cell_area(x: usize, y: usize) -> f32 {
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
        heap.push(Front { elevation: 0.0, cell: 0 });
        while let Some(Front { elevation, cell }) = heap.pop() {
            let c = cell as usize;
            for n in [c.checked_sub(1), (c + 1 < heights.len()).then_some(c + 1)].into_iter().flatten() {
                if done[n] { continue; }
                done[n] = true;
                outlets[n] = cell;
                spill[n] = heights[n].max(elevation);
                heap.push(Front { cell: n as u32, elevation: spill[n] });
            }
        }
        (outlets, spill)
    }

    #[test]
    fn cube_sphere_neighbors_connect_all_six_faces() {
        let mut neighboring = [0usize; 8];
        for face in 0..6 {
            for &(x, y) in &[(0, SIZE / 2), (SIZE - 1, SIZE / 2),
                (SIZE / 2, 0), (SIZE / 2, SIZE - 1)] {
                let count = adjacent(index(face, x, y), &mut neighboring);
                assert!((3..=8).contains(&count));
                assert!(neighboring[..count].iter().any(|&n| decode_index(n).0 != face));
            }
        }
    }
}
